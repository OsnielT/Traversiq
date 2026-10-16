use crate::cache::{Cache, CacheStats};
use crate::source::{self, Candidate, Location};
use crate::{load_project_paths, load_workspace};
use graphql_parser::{query as q, schema as s};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tree_sitter::Parser;

const LIMIT: usize = 100;

#[derive(Clone, Serialize)]
pub struct SchemaField {
    pub owner: String,
    pub name: String,
    pub output_type: String,
    pub location: Location,
    #[serde(skip)]
    named_type: String,
}
struct Schema {
    fields: BTreeMap<String, BTreeMap<String, SchemaField>>,
    roots: [String; 3],
}
#[derive(Serialize)]
pub struct QueryMatch {
    pub location: Location,
    pub operation: Option<String>,
    pub binding: Option<String>,
    pub response_path: Vec<String>,
    pub kind: &'static str,
}
#[derive(Serialize, Deserialize)]
pub struct GoCall {
    pub location: Location,
    pub expression: String,
    pub method: String,
}
#[derive(Serialize, Deserialize)]
pub struct GoMethod {
    pub location: Location,
    pub receiver: String,
    pub name: String,
    pub kind: String,
    pub selector_calls: Vec<GoCall>,
}
#[derive(Serialize)]
pub struct ApiTrace {
    pub source_cache: CacheStats,
    pub go_cache: CacheStats,
    pub provider: String,
    pub requested_path: String,
    pub coverage: &'static str,
    pub endpoint_binding: &'static str,
    pub schema_chain: Vec<SchemaField>,
    pub query_matches: Vec<QueryMatch>,
    pub query_imports: Vec<source::Reference>,
    pub downstream_files: Vec<Candidate>,
    pub resolver_candidates: Vec<GoMethod>,
    pub indexed_js_ts_files: usize,
    pub partial_js_ts_files: usize,
    pub parsed_graphql_documents: usize,
    pub skipped_graphql_documents: usize,
    pub parsed_go_files: usize,
    pub elapsed_ms: u128,
    pub omitted_results: usize,
    pub warnings: Vec<String>,
    pub omitted_warnings: usize,
    pub limitations: Vec<&'static str>,
}

fn type_names(ty: &s::Type<'_, String>) -> (String, String) {
    match ty {
        s::Type::NamedType(name) => (name.clone(), name.clone()),
        s::Type::ListType(inner) => {
            let (name, rendered) = type_names(inner);
            (name, format!("[{rendered}]"))
        }
        s::Type::NonNullType(inner) => {
            let (name, rendered) = type_names(inner);
            (name, format!("{rendered}!"))
        }
    }
}
fn schema(source: &str, location: Location) -> Result<Schema, String> {
    let document = s::parse_schema::<String>(source).map_err(|e| format!("Invalid schema: {e}"))?;
    let mut fields: BTreeMap<String, BTreeMap<String, SchemaField>> = BTreeMap::new();
    let mut roots = ["Query".into(), "Mutation".into(), "Subscription".into()];
    for definition in &document.definitions {
        let object = match definition {
            s::Definition::SchemaDefinition(root) => {
                for (index, name) in [
                    root.query.as_ref(),
                    root.mutation.as_ref(),
                    root.subscription.as_ref(),
                ]
                .into_iter()
                .enumerate()
                {
                    if let Some(name) = name {
                        roots[index] = name.clone();
                    }
                }
                None
            }
            s::Definition::TypeDefinition(s::TypeDefinition::Object(object)) => {
                Some((&object.name, &object.fields))
            }
            s::Definition::TypeDefinition(s::TypeDefinition::Interface(object)) => {
                Some((&object.name, &object.fields))
            }
            s::Definition::TypeExtension(s::TypeExtension::Object(object)) => {
                Some((&object.name, &object.fields))
            }
            s::Definition::TypeExtension(s::TypeExtension::Interface(object)) => {
                Some((&object.name, &object.fields))
            }
            _ => None,
        };
        if let Some((owner, declarations)) = object {
            for field in declarations {
                let (named_type, output_type) = type_names(&field.field_type);
                let entry = SchemaField {
                    owner: owner.clone(),
                    name: field.name.clone(),
                    output_type,
                    named_type,
                    location: Location {
                        line: field.position.line,
                        ..location.clone()
                    },
                };
                if fields
                    .entry(owner.clone())
                    .or_default()
                    .insert(field.name.clone(), entry)
                    .is_some()
                {
                    return Err(format!(
                        "Ambiguous duplicate schema field {owner}.{}",
                        field.name
                    ));
                }
            }
        }
    }
    Ok(Schema { fields, roots })
}
fn selected_fields(schema: &Schema, path: &str) -> Result<Vec<SchemaField>, String> {
    let names: Vec<_> = path.split('.').collect();
    if !(2..=32).contains(&names.len())
        || names.iter().any(|n| {
            n.is_empty()
                || !n.bytes().enumerate().all(|(i, b)| {
                    b == b'_' || b.is_ascii_alphabetic() || i > 0 && b.is_ascii_digit()
                })
        })
    {
        return Err("Use a schema type followed by field names, for example Query.tenant.sitemapXml (at most 32 parts)".into());
    }
    let mut owner = names[0].to_owned();
    let mut chain = Vec::new();
    for name in &names[1..] {
        let field = schema
            .fields
            .get(&owner)
            .and_then(|f| f.get(*name))
            .ok_or_else(|| format!("Unknown schema field {owner}.{name}"))?;
        chain.push(field.clone());
        owner = field.named_type.clone();
    }
    Ok(chain)
}

struct FieldHit {
    line: usize,
    response_path: Vec<String>,
}
struct SelectionSearch<'a, 'query> {
    schema: &'a Schema,
    fragments: BTreeMap<&'a str, &'a q::FragmentDefinition<'query, String>>,
    target: (&'a str, &'a str),
    visits: usize,
}
impl<'query> SelectionSearch<'_, 'query> {
    fn visit(
        &mut self,
        set: &q::SelectionSet<'query, String>,
        owner: &str,
        path: &mut Vec<String>,
        active: &mut BTreeSet<String>,
        hits: &mut Vec<FieldHit>,
        warnings: &mut Vec<String>,
    ) {
        if path.len() >= 32 || active.len() >= 32 {
            warnings.push("GraphQL traversal depth exceeded".into());
            return;
        }
        for selection in &set.items {
            self.visits += 1;
            if self.visits > 10_000 {
                if self.visits == 10_001 {
                    warnings.push("GraphQL selection budget exceeded".into());
                }
                return;
            }
            match selection {
                q::Selection::Field(field) => {
                    if field.name == "__typename" {
                        continue;
                    }
                    let Some(definition) = self
                        .schema
                        .fields
                        .get(owner)
                        .and_then(|f| f.get(&field.name))
                    else {
                        warnings.push(format!("Unknown schema field {owner}.{}", field.name));
                        continue;
                    };
                    path.push(field.alias.as_ref().unwrap_or(&field.name).clone());
                    if owner == self.target.0 && field.name == self.target.1 {
                        hits.push(FieldHit {
                            line: field.position.line,
                            response_path: path.clone(),
                        });
                    }
                    let next = definition.named_type.clone();
                    self.visit(&field.selection_set, &next, path, active, hits, warnings);
                    path.pop();
                }
                q::Selection::FragmentSpread(spread) => {
                    let Some(fragment) = self.fragments.get(spread.fragment_name.as_str()).copied()
                    else {
                        warnings.push(format!("Missing fragment {}", spread.fragment_name));
                        continue;
                    };
                    let q::TypeCondition::On(condition) = &fragment.type_condition;
                    if condition != owner {
                        warnings.push(format!(
                            "Unresolved fragment type condition {condition} on {owner}"
                        ));
                        continue;
                    }
                    if !active.insert(fragment.name.clone()) {
                        warnings.push(format!("Cyclic fragment {}", fragment.name));
                        continue;
                    }
                    self.visit(&fragment.selection_set, owner, path, active, hits, warnings);
                    active.remove(&fragment.name);
                }
                q::Selection::InlineFragment(fragment) => {
                    if let Some(q::TypeCondition::On(condition)) = &fragment.type_condition
                        && condition != owner
                    {
                        warnings.push(format!(
                            "Unresolved inline type condition {condition} on {owner}"
                        ));
                        continue;
                    }
                    self.visit(&fragment.selection_set, owner, path, active, hits, warnings);
                }
            }
        }
    }
}

fn queries(
    index: &source::SourceIndex,
    schema: &Schema,
    target: &SchemaField,
    warnings: &mut Vec<String>,
) -> (Vec<QueryMatch>, Vec<(PathBuf, Location)>, usize, usize) {
    let mut matches = Vec::new();
    let mut seeds = Vec::new();
    let mut parsed = 0;
    let mut skipped = 0;
    for (file, module) in index {
        for template in &module.gql_templates {
            let label = format!(
                "{}/{}:{}",
                module.location.project,
                module.location.path.display(),
                template.line
            );
            if template.interpolated || template.document.contains('\\') {
                skipped += 1;
                warnings.push(format!(
                    "GraphQL {label}: interpolation or JavaScript escapes not evaluated"
                ));
                continue;
            }
            let document = match q::parse_query::<String>(&template.document) {
                Ok(document) => document,
                Err(error) => {
                    skipped += 1;
                    warnings.push(format!("GraphQL {label}: {error}"));
                    continue;
                }
            };
            let mut fragments = BTreeMap::new();
            let mut duplicate = false;
            for definition in &document.definitions {
                if let q::Definition::Fragment(fragment) = definition
                    && fragments.insert(fragment.name.as_str(), fragment).is_some()
                {
                    duplicate = true;
                }
            }
            if duplicate {
                skipped += 1;
                warnings.push(format!(
                    "GraphQL {label}: duplicate fragments; document skipped"
                ));
                continue;
            }
            parsed += 1;
            let mut search = SelectionSearch {
                schema,
                fragments,
                target: (&target.owner, &target.name),
                visits: 0,
            };
            for definition in &document.definitions {
                let q::Definition::Operation(operation) = definition else {
                    continue;
                };
                let (name, owner, set) = match operation {
                    q::OperationDefinition::SelectionSet(set) => (None, &schema.roots[0], set),
                    q::OperationDefinition::Query(operation) => (
                        operation.name.clone(),
                        &schema.roots[0],
                        &operation.selection_set,
                    ),
                    q::OperationDefinition::Mutation(operation) => (
                        operation.name.clone(),
                        &schema.roots[1],
                        &operation.selection_set,
                    ),
                    q::OperationDefinition::Subscription(operation) => (
                        operation.name.clone(),
                        &schema.roots[2],
                        &operation.selection_set,
                    ),
                };
                let mut hits = Vec::new();
                let mut unknowns = Vec::new();
                search.visit(
                    set,
                    owner,
                    &mut Vec::new(),
                    &mut BTreeSet::new(),
                    &mut hits,
                    &mut unknowns,
                );
                if !unknowns.is_empty() {
                    warnings.push(format!(
                        "GraphQL {label}: {} unresolved traversal items; first: {}",
                        unknowns.len(),
                        unknowns[0]
                    ));
                }
                for hit in hits {
                    let location = Location {
                        line: template.line + hit.line - 1,
                        ..module.location.clone()
                    };
                    matches.push(QueryMatch {
                        location: location.clone(),
                        operation: name.clone(),
                        binding: template.binding.clone(),
                        response_path: hit.response_path,
                        kind: "schema_field_match_candidate",
                    });
                    seeds.push((file.clone(), location));
                }
            }
        }
    }
    (matches, seeds, parsed, skipped)
}

fn go_methods(
    root: &Path,
    provider: &str,
    local: &Path,
    warnings: &mut Vec<String>,
) -> Result<(Vec<GoMethod>, usize, CacheStats), String> {
    let roots = BTreeMap::from([(provider.to_owned(), root.to_owned())]);
    let mut cache = Cache::open(local, "go", &roots)?;
    let mut parser = Parser::new();
    // This bundled grammar is fixed at build time; failure is reported rather than running Go.
    if let Err(error) = parser.set_language(&tree_sitter_go::LANGUAGE.into()) {
        warnings.push(error.to_string());
        return Err(error.to_string());
    }
    let mut methods = Vec::new();
    let mut parsed = 0;
    for file in source::walk(root, "", &["go"], warnings) {
        if file
            .file_name()
            .is_some_and(|s| s.to_string_lossy().ends_with("_test.go"))
        {
            continue;
        }
        let text = match source::read_text(&file) {
            Ok(text) => text,
            Err(error) => {
                warnings.push(format!("Go {}: {error}", file.display()));
                continue;
            }
        };
        let facts = cache.parse(&file, provider, &text, warnings, |warnings| {
            parse_go_file(&mut parser, &file, root, provider, &text, warnings)
        })?;
        if let Some(facts) = facts {
            parsed += 1;
            methods.extend(facts);
        }
    }
    Ok((methods, parsed, cache.finish()?))
}

fn parse_go_file(
    parser: &mut Parser,
    file: &Path,
    root: &Path,
    provider: &str,
    text: &str,
    warnings: &mut Vec<String>,
) -> Option<Vec<GoMethod>> {
    let mut methods = Vec::new();
    let Some(tree) = parser.parse(text, None) else {
        warnings.push(format!("Go {}: no parse tree", file.display()));
        return None;
    };
    if tree.root_node().has_error() {
        warnings.push(format!(
            "Go {}: parser errors; affected methods skipped",
            file.display()
        ));
    }
    let location = Location {
        project: provider.into(),
        path: file.strip_prefix(root).unwrap().to_owned(),
        line: 1,
        definition: None,
    };
    for node in source::children(tree.root_node()) {
        if node.kind() != "method_declaration" || node.has_error() {
            continue;
        }
        let (Some(name), Some(receiver)) = (
            node.child_by_field_name("name"),
            node.child_by_field_name("receiver"),
        ) else {
            continue;
        };
        let mut calls = Vec::new();
        let mut pending = node
            .child_by_field_name("body")
            .into_iter()
            .collect::<Vec<_>>();
        while let Some(child) = pending.pop() {
            if child.kind() == "call_expression"
                && let Some(function) = child.child_by_field_name("function")
                && function.kind() == "selector_expression"
                && let Some(method) = function.child_by_field_name("field")
            {
                if calls.len() == LIMIT {
                    warnings.push(format!("Go {}: call-output cap reached", file.display()));
                    break;
                }
                calls.push(GoCall {
                    location: Location {
                        line: child.start_position().row + 1,
                        ..location.clone()
                    },
                    expression: source::text(function, text),
                    method: source::text(method, text),
                });
            }
            pending.extend(source::children(child).into_iter().rev());
        }
        methods.push(GoMethod {
            location: Location {
                line: node.start_position().row + 1,
                ..location.clone()
            },
            receiver: source::text(receiver, text),
            name: source::text(name, text),
            kind: "unselected".into(),
            selector_calls: calls,
        });
    }
    Some(methods)
}

pub fn trace_api(
    config: &Path,
    local: &Path,
    provider: &str,
    schema_entry: &Path,
    field_path: &str,
    all_warnings: bool,
) -> Result<ApiTrace, String> {
    let started = Instant::now();
    let workspace = load_workspace(config, local)?;
    let roots = load_project_paths(&workspace, local)?;
    let root = roots
        .get(provider)
        .ok_or_else(|| format!("Unknown provider project {provider}"))?;
    if schema_entry.is_absolute() {
        return Err("Schema entry must be relative to the provider project".into());
    }
    let path = root
        .join(schema_entry)
        .canonicalize()
        .map_err(|e| format!("Invalid schema path: {e}"))?;
    if !path.starts_with(root) {
        return Err("Schema entry must stay inside provider project".into());
    }
    let schema = schema(
        &source::read_text(&path)?,
        Location {
            project: provider.into(),
            path: path.strip_prefix(root).unwrap().to_owned(),
            line: 1,
            definition: None,
        },
    )?;
    let schema_chain = selected_fields(&schema, field_path)?;
    let target = schema_chain.last().unwrap();
    let mut warnings = Vec::new();
    let clients = roots
        .iter()
        .filter(|(id, _)| id.as_str() != provider)
        .map(|(id, root)| (id.clone(), root.clone()))
        .collect();
    let (index, source_cache) = source::index_sources(&clients, local, &mut warnings)?;
    let (resolved, reverse) = source::module_graph(&index, &clients, &mut warnings);
    let (mut matches, seeds, parsed_graphql_documents, skipped_graphql_documents) =
        queries(&index, &schema, target, &mut warnings);
    let mut query_imports = Vec::new();
    let mut focused_seeds = Vec::new();
    let mut visited_definitions = BTreeSet::new();
    for (query, (file, location)) in matches.iter().zip(seeds) {
        if let Some(binding) = &query.binding
            && let Some(definition) = index[&file].definition(binding)
        {
            if !visited_definitions.insert((file.clone(), binding.clone())) {
                continue;
            }
            for (consumer, mut reference) in
                source::importers_of_definition(&index, &resolved, &definition)
            {
                reference.kind = "query_binding_import";
                focused_seeds.push((consumer, reference.location.clone()));
                query_imports.push(reference);
            }
        } else {
            warnings.push(format!(
                "GraphQL {}: no top-level query binding; downstream uses file candidates",
                file.display()
            ));
            focused_seeds.push((file, location));
        }
    }
    let mut downstream = source::downstream_files(&reverse, focused_seeds, &mut warnings);
    let (methods, parsed_go_files, go_cache) = go_methods(root, provider, local, &mut warnings)?;
    let names: BTreeSet<_> = methods
        .iter()
        .filter(|m| m.name.eq_ignore_ascii_case(&target.name))
        .flat_map(|m| m.selector_calls.iter().map(|c| c.method.clone()))
        .collect();
    let mut resolver_candidates: Vec<_> = methods
        .into_iter()
        .filter_map(|mut method| {
            if method.name.eq_ignore_ascii_case(&target.name) {
                method.kind = "field_name_candidate".into();
            } else if names.contains(&method.name) {
                method.kind = "selector_call_name_candidate".into();
            } else {
                return None;
            }
            Some(method)
        })
        .collect();
    let omitted_results = matches.len().saturating_sub(LIMIT)
        + downstream.len().saturating_sub(LIMIT)
        + resolver_candidates.len().saturating_sub(LIMIT)
        + query_imports.len().saturating_sub(LIMIT);
    query_imports.truncate(LIMIT);
    matches.truncate(LIMIT);
    downstream.truncate(LIMIT);
    resolver_candidates.truncate(LIMIT);
    let partial_js_ts_files = index.values().filter(|m| m.partial_error.is_some()).count();
    let omitted_warnings = if all_warnings {
        0
    } else {
        warnings.len().saturating_sub(50)
    };
    if !all_warnings {
        warnings.truncate(50);
    }
    Ok(ApiTrace {
        source_cache,
        go_cache,
        provider: provider.into(),
        requested_path: field_path.into(),
        coverage: "partial_static_graphql_and_go",
        endpoint_binding: "unverified",
        schema_chain,
        query_matches: matches,
        query_imports,
        downstream_files: downstream,
        resolver_candidates,
        indexed_js_ts_files: index.len(),
        partial_js_ts_files,
        parsed_graphql_documents,
        skipped_graphql_documents,
        parsed_go_files,
        elapsed_ms: started.elapsed().as_millis(),
        omitted_results,
        warnings,
        omitted_warnings,
        limitations: vec![
            "Schema-field matches are cross-project candidates; no endpoint configuration is read, and no deployed API dependency is established.",
            "Only static gql-tagged templates in configured src directories are considered; tag bindings/shadowing are not resolved. Interpolation and JavaScript escapes are skipped, not executed.",
            "Same-document fragments and aliases are followed. Conditional directives, interface/union refinements and cross-document fragments are not resolved; this is not full GraphQL validation.",
            "Go methods are matched by field/method names and one hop of selector-call names; receiver types, package identity, runtime schema binding and deeper call chains remain unverified.",
            "Named query bindings are followed through local ESM exports/imports before expanding file candidates. Namespace query imports are not resolved. Downstream files are not proven value flow or breakages. Go test files are excluded from resolver candidates.",
            "Source scans use the existing 10000-file/directory, depth-20 and 1-MiB bounds. GraphQL traversal is capped at 32 levels and 10000 selections per document; result groups cap at 100 and warnings at 50 unless --all-warnings is selected.",
            "The schema entry is explicitly selected; imports/build aggregation, schema freshness, authentication and runtime execution are not checked. No network requests or project code execute.",
        ],
    })
}
