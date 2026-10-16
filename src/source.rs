use crate::cache::{Cache, CacheStats};
use crate::{discovery::excluded, load_project_paths, load_workspace};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tree_sitter::{Node, Parser};

const MAX_FILES: usize = 10_000;
const MAX_BYTES: u64 = 1_048_576;
const MAX_RESULTS: usize = 100;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Location {
    pub project: String,
    pub path: PathBuf,
    pub line: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub definition: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Binding {
    imported: String,
    local: String,
}
#[derive(Debug, Serialize, Deserialize)]
struct Link {
    reexport: bool,
    source: String,
    bindings: Vec<Binding>,
    line: usize,
}
#[derive(Debug, Serialize, Deserialize)]
struct Export {
    name: String,
    original: String,
    source: Option<String>,
    line: usize,
}
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Module {
    pub(crate) partial_error: Option<String>,
    pub(crate) location: Location,
    pub(crate) gql_templates: Vec<GraphqlTemplate>,
    links: Vec<Link>,
    exports: Vec<Export>,
    declarations: BTreeMap<String, usize>,
}

impl Module {
    pub(crate) fn definition(&self, name: &str) -> Option<Location> {
        self.declarations.get(name).map(|line| Location {
            line: *line,
            definition: Some(name.to_owned()),
            ..self.location.clone()
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct GraphqlTemplate {
    pub document: String,
    pub line: usize,
    pub binding: Option<String>,
    pub interpolated: bool,
}

#[derive(Serialize)]
pub struct Reference {
    pub imported_symbol: String,
    pub location: Location,
    pub local_name: String,
    pub kind: &'static str,
}
#[derive(Serialize)]
pub struct Wrapper {
    pub exported_symbol: String,
    pub declaration: Location,
    pub dependency: Location,
}
#[derive(Serialize)]
pub struct Candidate {
    pub location: Location,
    pub kind: &'static str,
    pub evidence_chain: Vec<Location>,
}
#[derive(Serialize)]
pub struct Trace {
    pub cache: CacheStats,
    pub provider: String,
    pub package: String,
    pub symbol: String,
    pub coverage: &'static str,
    pub entry_to_definition: Vec<Location>,
    pub provider_references: Vec<Reference>,
    pub wrapper_candidates: Vec<Wrapper>,
    pub direct_references: Vec<Reference>,
    pub downstream_files: Vec<Candidate>,
    pub parsed_files: usize,
    pub omitted_source_files: usize,
    pub partial_source_files: usize,
    pub unresolved_local_modules: usize,
    pub elapsed_ms: u128,
    pub omitted_results: usize,
    pub warnings: Vec<String>,
    pub omitted_warnings: usize,
    pub limitations: Vec<&'static str>,
}

pub(crate) fn text(node: Node<'_>, source: &str) -> String {
    source[node.byte_range()].to_owned()
}
pub(crate) fn children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}
fn string(node: Node<'_>, source: &str) -> Result<String, String> {
    let raw = text(node, source);
    if raw.contains('\\') || raw.len() < 2 {
        return Err("escaped module strings are not resolved".into());
    }
    Ok(raw[1..raw.len() - 1].to_owned())
}
fn bindings(node: Node<'_>, source: &str, output: &mut Vec<Binding>) {
    match node.kind() {
        "import_specifier" | "export_specifier" => {
            if let Some(name) = node.child_by_field_name("name") {
                output.push(Binding {
                    imported: text(name, source),
                    local: text(node.child_by_field_name("alias").unwrap_or(name), source),
                });
            }
        }
        "identifier" => output.push(Binding {
            imported: "default".into(),
            local: text(node, source),
        }),
        "namespace_import" | "namespace_export" => {
            output.push(Binding {
                imported: "*".into(),
                local: children(node)
                    .first()
                    .map(|n| text(*n, source))
                    .unwrap_or_default(),
            });
        }
        "named_imports" | "import_clause" | "export_clause" => {
            for child in children(node) {
                bindings(child, source, output);
            }
        }
        _ => {}
    }
}
fn declarations(node: Node<'_>, source: &str) -> Vec<(String, usize)> {
    if let Some(name) = node.child_by_field_name("name")
        && matches!(name.kind(), "identifier" | "type_identifier")
    {
        return vec![(text(name, source), name.start_position().row + 1)];
    }
    if matches!(node.kind(), "lexical_declaration" | "variable_declaration") {
        return children(node)
            .into_iter()
            .filter(|n| n.kind() == "variable_declarator")
            .flat_map(|n| declarations(n, source))
            .collect();
    }
    Vec::new()
}
fn parse(
    parser: &mut Parser,
    location: Location,
    source: &str,
    extension: &str,
) -> Result<Module, String> {
    let language = match extension {
        "ts" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "tsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
        _ => tree_sitter_javascript::LANGUAGE.into(),
    };
    parser.set_language(&language).map_err(|e| e.to_string())?;
    let tree = parser
        .parse(source, None)
        .ok_or("parser returned no tree")?;
    let mut partial_error = None;
    if tree.root_node().has_error() {
        let mut pending = vec![tree.root_node()];
        while let Some(node) = pending.pop() {
            if node.is_error() || node.is_missing() {
                let at = node.start_position();
                partial_error = Some(format!(
                    "parser error at {}:{} ({}); affected top-level statements skipped",
                    at.row + 1,
                    at.column + 1,
                    node.kind()
                ));
                break;
            }
            let mut cursor = node.walk();
            let descendants: Vec<_> = node.children(&mut cursor).collect();
            pending.extend(
                descendants
                    .into_iter()
                    .rev()
                    .filter(|n| n.has_error() || n.is_missing()),
            );
        }
        if partial_error.is_none() {
            partial_error = Some("parser error; affected top-level statements skipped".into());
        }
    }
    let mut module = Module {
        partial_error,
        location,
        gql_templates: Vec::new(),
        links: Vec::new(),
        exports: Vec::new(),
        declarations: BTreeMap::new(),
    };
    for node in children(tree.root_node()) {
        // shortcut: grammar errors leave partial coverage; only error-free top-level statements are indexed until the grammar supports the rejected syntax.
        if node.has_error() || node.is_missing() {
            continue;
        }
        let line = node.start_position().row + 1;
        let source_path = node
            .child_by_field_name("source")
            .map(|n| string(n, source))
            .transpose()?;
        if matches!(node.kind(), "import_statement" | "export_statement") {
            let mut names = Vec::new();
            for child in children(node) {
                if matches!(
                    child.kind(),
                    "import_clause" | "export_clause" | "namespace_export"
                ) {
                    bindings(child, source, &mut names);
                }
            }
            if node.kind() == "export_statement" {
                let mut cursor = node.walk();
                let is_default = node.children(&mut cursor).any(|n| n.kind() == "default");
                if let Some(declaration) = node.child_by_field_name("declaration") {
                    for (name, at) in declarations(declaration, source) {
                        module.declarations.insert(name.clone(), at);
                        if !is_default {
                            module.exports.push(Export {
                                name: name.clone(),
                                original: name,
                                source: None,
                                line,
                            });
                        }
                    }
                }
                for binding in &names {
                    module.exports.push(Export {
                        name: binding.local.clone(),
                        original: binding.imported.clone(),
                        source: source_path.clone(),
                        line,
                    });
                }
                if source_path.is_some() && names.is_empty() {
                    names.push(Binding {
                        imported: "*".into(),
                        local: "*".into(),
                    });
                    module.exports.push(Export {
                        name: "*".into(),
                        original: "*".into(),
                        source: source_path.clone(),
                        line,
                    });
                }
                // shortcut: default exports are represented as locations, not resolved expressions; use type-aware resolution when default-expression internals matter.
                if is_default {
                    module.declarations.insert("default".into(), line);
                    module.exports.push(Export {
                        name: "default".into(),
                        original: "default".into(),
                        source: None,
                        line,
                    });
                }
            }
            if let Some(source) = source_path {
                module.links.push(Link {
                    reexport: node.kind() == "export_statement",
                    source,
                    bindings: names,
                    line,
                });
            }
        } else {
            module.declarations.extend(declarations(node, source));
        }
    }
    let mut pending = vec![tree.root_node()];
    while let Some(node) = pending.pop() {
        if node.is_error() || node.is_missing() {
            continue;
        }
        if node.kind() == "call_expression"
            && !node.has_error()
            && let Some(tag) = node.child_by_field_name("function")
            && text(tag, source) == "gql"
            && let Some(template) = node.child_by_field_name("arguments")
            && template.kind() == "template_string"
        {
            let raw = text(template, source);
            let binding = node
                .parent()
                .filter(|p| p.kind() == "variable_declarator")
                .filter(|p| {
                    p.parent()
                        .and_then(|declaration| declaration.parent())
                        .is_some_and(|scope| {
                            scope.kind() == "program"
                                || scope.kind() == "export_statement"
                                    && scope.parent().is_some_and(|p| p.kind() == "program")
                        })
                })
                .and_then(|p| p.child_by_field_name("name"))
                .filter(|n| n.kind() == "identifier")
                .map(|n| text(n, source));
            module.gql_templates.push(GraphqlTemplate {
                document: raw[1..raw.len() - 1].to_owned(),
                line: template.start_position().row + 1,
                binding,
                interpolated: children(template)
                    .iter()
                    .any(|n| n.kind() == "template_substitution"),
            });
        }
        pending.extend(children(node).into_iter().rev());
    }
    if let Some(error) = &module.partial_error
        && module.links.is_empty()
        && module.exports.is_empty()
        && module.declarations.is_empty()
        && module.gql_templates.is_empty()
    {
        return Err(format!("{}; no usable top-level facts", error));
    }
    Ok(module)
}

pub(crate) fn read_text(path: &Path) -> Result<String, String> {
    if !fs::symlink_metadata(path)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_file()
    {
        return Err("only regular files are read; symbolic links are skipped".into());
    }
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("file exceeds 1 MiB".into());
    }
    String::from_utf8(bytes).map_err(|e| e.to_string())
}
pub(crate) fn walk(
    root: &Path,
    subdir: &str,
    extensions: &[&str],
    warnings: &mut Vec<String>,
) -> Vec<PathBuf> {
    let source_root = root.join(subdir);
    if !fs::symlink_metadata(&source_root).is_ok_and(|m| m.file_type().is_dir()) {
        warnings.push(format!(
            "No regular {} directory for {}; project not source-indexed",
            subdir,
            root.display()
        ));
        return Vec::new();
    }
    let mut queue = VecDeque::from([(source_root, 0)]);
    let mut files = Vec::new();
    let mut dirs = 0;
    while let Some((dir, depth)) = queue.pop_front() {
        dirs += 1;
        if dirs > MAX_FILES {
            warnings.push(format!("Directory limit reached for {}", root.display()));
            break;
        }
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) => {
                warnings.push(format!("Cannot scan {}: {e}", dir.display()));
                continue;
            }
        };
        let mut entries = entries
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string());
        let Ok(ref mut entries) = entries else {
            warnings.push(format!("Cannot enumerate {}", dir.display()));
            continue;
        };
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            if excluded(&entry.file_name().to_string_lossy()) {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                warnings.push(format!("Cannot inspect {}", entry.path().display()));
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                if depth < 20 {
                    queue.push_back((entry.path(), depth + 1));
                } else {
                    warnings.push(format!("Depth limit at {}", entry.path().display()));
                }
            } else if kind.is_file()
                && entry
                    .path()
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| extensions.contains(&e))
            {
                if files.len() == MAX_FILES {
                    warnings.push(format!("Source file limit reached for {}", root.display()));
                    return files;
                }
                files.push(entry.path());
            }
        }
    }
    files
}

type Aliases = BTreeMap<String, Vec<String>>;
fn aliases(root: &Path, warnings: &mut Vec<String>) -> Aliases {
    let path = root.join("tsconfig.json");
    if !path.exists() {
        return BTreeMap::new();
    }
    match read_text(&path)
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).map_err(|e| e.to_string()))
    {
        Ok(value) => {
            if value.get("extends").is_some() || value.pointer("/compilerOptions/baseUrl").is_some()
            {
                warnings.push(format!(
                    "{}: extends/baseUrl not resolved; aliases omitted",
                    path.display()
                ));
                return BTreeMap::new();
            }
            match value.pointer("/compilerOptions/paths") {
                Some(paths) => serde_json::from_value(paths.clone()).unwrap_or_else(|_| {
                    warnings.push(format!("Invalid paths in {}", path.display()));
                    BTreeMap::new()
                }),
                None => BTreeMap::new(),
            }
        }
        Err(e) => {
            warnings.push(format!("Aliases unavailable for {}: {e}", path.display()));
            BTreeMap::new()
        }
    }
}
fn resolve(
    index: &BTreeMap<PathBuf, Module>,
    roots: &BTreeMap<String, PathBuf>,
    aliases: &BTreeMap<String, Aliases>,
    file: &Path,
    spec: &str,
) -> Option<PathBuf> {
    let module = index.get(file)?;
    let mut bases = Vec::new();
    if spec.starts_with('.') {
        bases.push(file.parent()?.join(spec));
    } else {
        for (pattern, targets) in &aliases[&module.location.project] {
            let middle = if let Some(prefix) = pattern.strip_suffix('*') {
                spec.strip_prefix(prefix)
            } else if pattern == spec {
                Some("")
            } else {
                None
            };
            if let Some(middle) = middle {
                for target in targets {
                    if let Some(prefix) = target.strip_suffix('*') {
                        bases.push(
                            roots[&module.location.project].join(format!("{prefix}{middle}")),
                        );
                    } else if middle.is_empty() {
                        bases.push(roots[&module.location.project].join(target));
                    }
                }
            }
        }
    }
    let mut matches = BTreeSet::new();
    for base in bases {
        let mut candidates = vec![base.clone()];
        if !matches!(
            base.extension().and_then(|s| s.to_str()),
            Some("ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs")
        ) {
            for extension in ["ts", "tsx", "js", "jsx", "mjs", "cjs"] {
                let mut filename = base.as_os_str().to_owned();
                filename.push(format!(".{extension}"));
                candidates.push(PathBuf::from(filename));
                candidates.push(base.join(format!("index.{extension}")));
            }
        }
        for candidate in candidates {
            if let Ok(path) = candidate.canonicalize()
                && index.contains_key(&path)
            {
                matches.insert(path);
            }
        }
    }
    if matches.len() == 1 {
        matches.pop_first()
    } else {
        None
    }
}

fn export_paths(
    index: &BTreeMap<PathBuf, Module>,
    resolved: &BTreeMap<(PathBuf, String), PathBuf>,
    file: &Path,
    symbol: &str,
    visited: &mut BTreeSet<(PathBuf, String)>,
) -> Vec<Vec<Location>> {
    let key = (file.to_owned(), symbol.to_owned());
    if visited.len() >= 64 || !visited.insert(key.clone()) {
        return Vec::new();
    }
    let Some(module) = index.get(file) else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    let has_explicit = module.exports.iter().any(|export| export.name == symbol);
    for export in &module.exports {
        if export.name != symbol && (has_explicit || export.name != "*" || symbol == "default") {
            continue;
        }
        let original = if export.name == "*" {
            symbol
        } else {
            &export.original
        };
        let at = Location {
            line: export.line,
            ..module.location.clone()
        };
        let mut target = export
            .source
            .as_ref()
            .and_then(|spec| resolved.get(&(file.to_owned(), spec.clone())))
            .map(|p| (p.clone(), original.to_owned()));
        if export.source.is_none() {
            if let Some(definition) = module.definition(original) {
                paths.push(if at.line == definition.line {
                    vec![definition]
                } else {
                    vec![at, definition]
                });
                continue;
            }
            for link in &module.links {
                for binding in &link.bindings {
                    if binding.local == original
                        && let Some(path) = resolved.get(&(file.to_owned(), link.source.clone()))
                    {
                        target = Some((path.clone(), binding.imported.clone()));
                    }
                }
            }
        }
        if let Some((path, name)) = target {
            if name == "*" {
                continue;
            }
            for mut chain in export_paths(index, resolved, &path, &name, visited) {
                chain.insert(0, at.clone());
                paths.push(chain);
            }
        }
    }
    visited.remove(&key);
    paths
}

fn public_names(
    index: &BTreeMap<PathBuf, Module>,
    resolved: &BTreeMap<(PathBuf, String), PathBuf>,
    file: &Path,
    visited: &mut BTreeSet<PathBuf>,
) -> BTreeSet<String> {
    if visited.len() >= 64 || !visited.insert(file.to_owned()) {
        return BTreeSet::new();
    }
    let mut names = BTreeSet::new();
    if let Some(module) = index.get(file) {
        for export in &module.exports {
            if export.name != "*" {
                names.insert(export.name.clone());
            } else if let Some(source) = &export.source
                && let Some(target) = resolved.get(&(file.to_owned(), source.clone()))
            {
                let mut inherited = public_names(index, resolved, target, visited);
                inherited.remove("default");
                names.extend(inherited);
            }
        }
    }
    visited.remove(file);
    names
}

pub(crate) type SourceIndex = BTreeMap<PathBuf, Module>;
pub(crate) type ResolvedModules = BTreeMap<(PathBuf, String), PathBuf>;
pub(crate) type ReverseModules = BTreeMap<PathBuf, Vec<(PathBuf, Location)>>;

pub(crate) fn index_sources(
    roots: &BTreeMap<String, PathBuf>,
    local: &Path,
    warnings: &mut Vec<String>,
) -> Result<(SourceIndex, CacheStats), String> {
    let mut cache = Cache::open(local, "js-ts", roots)?;
    let mut parser = Parser::new();
    let mut index = BTreeMap::new();
    for (project, root) in roots {
        for path in walk(
            root,
            "src",
            &["ts", "tsx", "js", "jsx", "mjs", "cjs"],
            warnings,
        ) {
            let location = Location {
                project: project.clone(),
                path: path.strip_prefix(root).unwrap().to_owned(),
                line: 1,
                definition: None,
            };
            let parsed = match read_text(&path) {
                Ok(s) => cache.parse(&path, project, &s, warnings, |_| {
                    parse(
                        &mut parser,
                        location,
                        &s,
                        path.extension().and_then(|e| e.to_str()).unwrap_or("js"),
                    )
                })?,
                Err(error) => Err(error),
            };
            match parsed {
                Ok(module) => {
                    if let Some(error) = &module.partial_error {
                        warnings.push(format!("Partial {}: {error}", path.display()));
                    }
                    index.insert(path, module);
                }
                Err(e) => warnings.push(format!("Omitted {}: {e}", path.display())),
            }
        }
    }
    Ok((index, cache.finish()?))
}

pub(crate) fn module_graph(
    index: &SourceIndex,
    roots: &BTreeMap<String, PathBuf>,
    warnings: &mut Vec<String>,
) -> (ResolvedModules, ReverseModules) {
    let project_aliases = roots
        .iter()
        .map(|(id, root)| (id.clone(), aliases(root, warnings)))
        .collect();
    let mut resolved = BTreeMap::new();
    let mut reverse: BTreeMap<PathBuf, Vec<(PathBuf, Location)>> = BTreeMap::new();
    for (path, module) in index {
        for link in &module.links {
            if let Some(target) = resolve(index, roots, &project_aliases, path, &link.source) {
                resolved.insert((path.clone(), link.source.clone()), target.clone());
                reverse.entry(target).or_default().push((
                    path.clone(),
                    Location {
                        line: link.line,
                        ..module.location.clone()
                    },
                ));
            } else if (link.source.starts_with('.') || link.source.starts_with("@/"))
                && !matches!(
                    Path::new(&link.source).extension().and_then(|s| s.to_str()),
                    Some("css" | "scss" | "sass" | "svg" | "png" | "jpg" | "jpeg" | "json")
                )
            {
                warnings.push(format!(
                    "Unresolved local module: {}/{}:{} {}",
                    module.location.project,
                    module.location.path.display(),
                    link.line,
                    link.source
                ));
            }
        }
    }
    (resolved, reverse)
}

pub(crate) fn downstream_files(
    reverse: &ReverseModules,
    seeds: Vec<(PathBuf, Location)>,
    warnings: &mut Vec<String>,
) -> Vec<Candidate> {
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::new();
    for (path, location) in seeds {
        if seen.insert(path.clone()) {
            queue.push_back((path, vec![location]));
        }
    }
    let mut downstream = Vec::new();
    while let Some((path, chain)) = queue.pop_front() {
        if chain.len() >= 8 {
            if reverse.contains_key(&path) {
                warnings.push(format!(
                    "Downstream depth limit reached at {}",
                    path.display()
                ));
            }
            continue;
        }
        for (consumer, evidence) in reverse.get(&path).into_iter().flatten() {
            if seen.insert(consumer.clone()) {
                let mut next = vec![evidence.clone()];
                next.extend(chain.clone());
                downstream.push(Candidate {
                    location: evidence.clone(),
                    kind: "file_dependency_candidate",
                    evidence_chain: next.clone(),
                });
                queue.push_back((consumer.clone(), next));
            }
        }
    }
    downstream
}

pub(crate) fn importers_of_definition(
    index: &SourceIndex,
    resolved: &ResolvedModules,
    definition: &Location,
) -> Vec<(PathBuf, Reference)> {
    let mut references = Vec::new();
    for (file, module) in index {
        for link in module.links.iter().filter(|link| !link.reexport) {
            let Some(target) = resolved.get(&(file.clone(), link.source.clone())) else {
                continue;
            };
            for binding in &link.bindings {
                if binding.imported == "*" {
                    continue;
                }
                let mut origins = export_paths(
                    index,
                    resolved,
                    target,
                    &binding.imported,
                    &mut BTreeSet::new(),
                );
                origins.sort_by_key(|chain| {
                    chain
                        .last()
                        .map(|l| (l.path.clone(), l.line, l.definition.clone()))
                });
                origins.dedup_by(|a, b| a.last() == b.last());
                if origins.len() == 1 && origins[0].last() == Some(definition) {
                    let location = Location {
                        line: link.line,
                        ..module.location.clone()
                    };
                    references.push((
                        file.clone(),
                        Reference {
                            imported_symbol: binding.imported.clone(),
                            location,
                            local_name: binding.local.clone(),
                            kind: "local_named_import",
                        },
                    ));
                }
            }
        }
    }
    references
}

pub fn trace(
    config: &Path,
    local: &Path,
    provider: &str,
    entry: &Path,
    symbol: &str,
    all_warnings: bool,
) -> Result<Trace, String> {
    let started = Instant::now();
    let workspace = load_workspace(config, local)?;
    let roots = load_project_paths(&workspace, local)?;
    let root = roots
        .get(provider)
        .ok_or_else(|| format!("Unknown provider project {provider}"))?;
    if symbol.is_empty() || entry.is_absolute() {
        return Err(
            "Use a nonempty symbol and an entry path relative to the provider project".into(),
        );
    }
    let entry = root
        .join(entry)
        .canonicalize()
        .map_err(|e| format!("Invalid entry: {e}"))?;
    if !entry.starts_with(root) {
        return Err("Entry must stay inside the provider project".into());
    }
    let manifest: serde_json::Value =
        serde_json::from_str(&read_text(&root.join("package.json"))?).map_err(|e| e.to_string())?;
    let package = manifest
        .get("name")
        .and_then(|n| n.as_str())
        .filter(|n| !n.is_empty())
        .ok_or("Provider needs a package.json name")?
        .to_owned();
    let mut warnings = Vec::new();
    let (index, cache) = index_sources(&roots, local, &mut warnings)?;
    let (resolved, reverse) = module_graph(&index, &roots, &mut warnings);
    let mut paths = export_paths(&index, &resolved, &entry, symbol, &mut BTreeSet::new());
    paths.sort_by_key(|p| {
        p.last().map(|l| {
            (
                l.project.clone(),
                l.path.clone(),
                l.line,
                l.definition.clone(),
            )
        })
    });
    paths.dedup_by(|a, b| a.last() == b.last());
    if paths.len() != 1 {
        return Err(format!(
            "Export {symbol} has {} resolved definitions from {}; specify an unambiguous indexed entry",
            paths.len(),
            entry.display()
        ));
    }
    let definition = paths[0].last().unwrap();
    let mut provider_references = Vec::new();
    let mut wrapper_files = BTreeMap::new();
    for (_, reference) in importers_of_definition(&index, &resolved, definition) {
        if reference.location.project != provider {
            continue;
        }
        wrapper_files.insert(reference.location.path.clone(), reference.location.clone());
        provider_references.push(reference);
    }
    let mut wrapper_candidates = Vec::new();
    let mut affected_exports = BTreeSet::from([symbol.to_owned()]);
    for name in public_names(&index, &resolved, &entry, &mut BTreeSet::new()) {
        if name == symbol {
            continue;
        }
        let origins = export_paths(&index, &resolved, &entry, &name, &mut BTreeSet::new());
        if let Some(declaration) = origins.first().and_then(|p| p.last())
            && origins.iter().all(|p| p.last() == Some(declaration))
            && let Some(dependency) = wrapper_files.get(&declaration.path)
        {
            affected_exports.insert(name.clone());
            wrapper_candidates.push(Wrapper {
                exported_symbol: name,
                declaration: declaration.clone(),
                dependency: dependency.clone(),
            });
        }
    }
    let mut direct = Vec::new();
    let mut seeds = Vec::new();
    for (path, module) in &index {
        if module.location.project != provider
            && !workspace.projects[&module.location.project]
                .depends_on
                .iter()
                .any(|p| p == provider)
        {
            continue;
        }
        for link in &module.links {
            if link.source != package {
                continue;
            }
            for binding in &link.bindings {
                if affected_exports.contains(&binding.imported) || binding.imported == "*" {
                    let location = Location {
                        line: link.line,
                        ..module.location.clone()
                    };
                    direct.push(Reference {
                        imported_symbol: binding.imported.clone(),
                        location: location.clone(),
                        local_name: binding.local.clone(),
                        kind: if binding.imported == "*" {
                            "namespace_or_star_candidate"
                        } else if binding.imported == symbol {
                            "named_import_or_reexport"
                        } else {
                            "public_wrapper_candidate"
                        },
                    });
                    seeds.push((path.clone(), location));
                }
            }
        }
    }
    let mut downstream = downstream_files(&reverse, seeds, &mut warnings);
    let omitted_results = direct.len().saturating_sub(MAX_RESULTS)
        + downstream.len().saturating_sub(MAX_RESULTS)
        + provider_references.len().saturating_sub(MAX_RESULTS)
        + wrapper_candidates.len().saturating_sub(MAX_RESULTS);
    provider_references.truncate(MAX_RESULTS);
    wrapper_candidates.truncate(MAX_RESULTS);
    direct.truncate(MAX_RESULTS);
    downstream.truncate(MAX_RESULTS);
    let mut entry_to_definition = paths.remove(0);
    entry_to_definition.dedup();
    let omitted_source_files = warnings
        .iter()
        .filter(|w| w.starts_with("Omitted "))
        .count();
    let partial_source_files = warnings
        .iter()
        .filter(|w| w.starts_with("Partial "))
        .count();
    let unresolved_local_modules = warnings
        .iter()
        .filter(|w| w.starts_with("Unresolved local module:"))
        .count();
    let omitted_warnings = if all_warnings {
        0
    } else {
        warnings.len().saturating_sub(50)
    };
    if !all_warnings {
        warnings.truncate(50);
    }
    Ok(Trace {
        cache,
        provider: provider.into(),
        package,
        symbol: symbol.into(),
        coverage: "partial_static_import_graph",
        entry_to_definition,
        provider_references,
        wrapper_candidates,
        direct_references: direct,
        downstream_files: downstream,
        parsed_files: index.len(),
        omitted_source_files,
        partial_source_files,
        unresolved_local_modules,
        elapsed_ms: started.elapsed().as_millis(),
        omitted_results,
        warnings,
        omitted_warnings,
        limitations: vec![
            "Caller supplies the package-root source entry; build configuration and published/installed package identity are not verified.",
            "Only error-free top-level JavaScript/TypeScript ESM statements under configured src directories are indexed. Files with parser errors retain a partial status; Go/GraphQL, CommonJS, dynamic imports and subpath package exports are not resolved.",
            "Named imports/reexports are observed syntax; namespace/star references and downstream file dependencies are candidates, not resolved symbol usage or proven breakages.",
            "Relative modules and simple tsconfig JSON paths are supported; JSONC, extends/baseUrl, conditional resolution and .js-to-.ts remapping are not supported.",
            "Consumer imports of the selected symbol and one-hop public wrapper candidates are seeded. Wrapper candidates are exports co-located with a provider import of the selected symbol, not proven semantic dependencies. Deeper wrapper chains, type checking, shadowing and runtime execution are not analyzed.",
            "Scans at most 10000 directories/files per project, depth 20, and 1 MiB per file; downstream depth is capped at 8, reference/wrapper/downstream output at 100 each, and warnings at 50 with omitted counts unless --all-warnings is requested.",
            "Content-hashed parsed facts are cached; relationships are rebuilt each request. Scans are not an atomic snapshot. An empty result does not establish safety.",
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_aliases_multiline_exports_and_ignores_comments_and_strings() {
        let source = "// import { Fake } from 'lib';\nconst s = \"import { Nope } from 'lib'\";\nimport type { Form as LocalForm } from 'lib';\nexport { LocalForm as AppForm };\nexport * from './other';\nexport const Component = () => <div/>;";
        let m = parse(
            &mut Parser::new(),
            Location {
                project: "p".into(),
                path: "src/a.tsx".into(),
                line: 1,
                definition: None,
            },
            source,
            "tsx",
        )
        .unwrap();
        assert_eq!(m.links.len(), 2);
        assert_eq!(m.links[0].bindings[0].imported, "Form");
        assert_eq!(m.links[0].bindings[0].local, "LocalForm");
        assert_eq!(m.exports[0].name, "AppForm");
        assert!(m.declarations.contains_key("Component"));
        assert!(!m.exports.iter().any(|e| e.name == "Fake"));
    }
    #[test]
    fn malformed_top_level_facts_are_not_recovered_as_valid_dependencies() {
        let module = parse(
            &mut Parser::new(),
            Location {
                project: "p".into(),
                path: "a.ts".into(),
                line: 1,
                definition: None,
            },
            "import { Fake } from ;\nexport const Safe = 1;\nexport const Broken = ;",
            "ts",
        )
        .unwrap();
        assert!(module.partial_error.is_some());
        assert!(module.links.is_empty());
        assert!(module.declarations.contains_key("Safe"));
        assert!(!module.exports.iter().any(|e| e.name == "Broken"));
    }
    #[test]
    fn syntax_errors_are_not_silently_indexed() {
        assert!(
            parse(
                &mut Parser::new(),
                Location {
                    project: "p".into(),
                    path: "a.ts".into(),
                    line: 1,
                    definition: None,
                },
                "export function broken(",
                "ts"
            )
            .is_err()
        );
    }
}
