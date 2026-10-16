use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let sequence = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "traversiq-test-{}-{stamp}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        for project in ["web", "ui"] {
            fs::create_dir(root.join(project)).unwrap();
        }
        fs::write(
            root.join("workspace.json"),
            r#"{"version":1,"projects":{"ui":{"depends_on":[]},"web":{"depends_on":["ui"]}}}"#,
        )
        .unwrap();
        fs::write(
            root.join("local.json"),
            r#"{"paths":{"ui":"ui","web":"web"}}"#,
        )
        .unwrap();
        Self(root)
    }

    fn run(&self, command: &str, extra: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_traversiq"))
            .current_dir(&self.0)
            .args([command, "workspace.json", "local.json"])
            .args(extra)
            .output()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn cli_resolves_local_paths_and_reports_transitive_candidates() {
    let fixture = Fixture::new();
    assert!(fixture.run("validate", &[]).status.success());
    let result = fixture.run("impact", &["ui"]);
    assert!(result.status.success());
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["coverage"], "declared_project_dependencies_only");
    assert_eq!(
        report["affected"][0]["dependency_chain"],
        serde_json::json!(["web", "ui"])
    );
    assert_eq!(report["declaration_source"], "workspace.json");
    assert!(!fixture.run("impact", &["missing"]).status.success());
}

#[test]
fn bad_local_paths_never_produce_a_successful_report() {
    let fixture = Fixture::new();
    for json in [
        r#"{"paths":{"ui":"ui"}}"#,
        r#"{"paths":{"ui":"ui","web":"missing"}}"#,
        r#"{"paths":{"ui":"ui","web":"ui"}}"#,
        r#"{"paths":{"ui":"ui","web":"web","unknown":"web"}}"#,
        r#"{"paths":{"ui":"ui","web":"workspace.json"}}"#,
        r#"{"paths":{"ui":"ui","web":""}}"#,
        "invalid JSON",
    ] {
        fs::write(fixture.0.join("local.json"), json).unwrap();
        let result = fixture.run("impact", &["ui"]);
        assert!(!result.status.success(), "accepted {json}");
        assert!(result.stdout.is_empty());
        assert!(!result.stderr.is_empty());
    }
}

fn discover(fixture: &Fixture) -> serde_json::Value {
    let output = Command::new(env!("CARGO_BIN_EXE_traversiq"))
        .args(["discover", fixture.0.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn discovery_proposes_links_with_evidence_and_loadable_configs() {
    let fixture = Fixture::new();
    let web = r#"{"name":"web","scripts":{"postinstall":"touch SHOULD_NOT_EXIST"},"dependencies":{"@team/ui":"file:../ui"}}"#;
    fs::write(fixture.0.join("web/package.json"), web).unwrap();
    fs::write(
        fixture.0.join("ui/package.json"),
        r#"{"name":"@team/ui","version":"2.0.0"}"#,
    )
    .unwrap();
    let report = discover(&fixture);
    assert_eq!(report["status"], "needs_review");
    assert_eq!(
        report["workspace"]["projects"]["web"]["depends_on"],
        serde_json::json!(["ui"])
    );
    assert_eq!(report["relationships"][0]["kind"], "local_path");
    assert_eq!(
        report["relationships"][0]["json_pointer"],
        "/dependencies/@team~1ui"
    );
    assert_eq!(report["relationships"][0]["needs_confirmation"], true);
    assert!(!fixture.0.join("web/SHOULD_NOT_EXIST").exists());
    assert_eq!(
        fs::read_to_string(fixture.0.join("web/package.json")).unwrap(),
        web
    );
    fs::write(
        fixture.0.join("workspace.json"),
        report["workspace"].to_string(),
    )
    .unwrap();
    fs::write(fixture.0.join("local.json"), report["local"].to_string()).unwrap();
    assert!(fixture.run("validate", &[]).status.success());
}

#[test]
fn discovery_does_not_guess_between_same_named_packages() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.0.join("other-ui")).unwrap();
    for dir in ["ui", "other-ui"] {
        fs::write(fixture.0.join(dir).join("package.json"), r#"{"name":"ui"}"#).unwrap();
    }
    fs::write(
        fixture.0.join("web/package.json"),
        r#"{"name":"web","dependencies":{"ui":"^1.0.0"}}"#,
    )
    .unwrap();
    let report = discover(&fixture);
    assert_eq!(report["relationships"], serde_json::json!([]));
    assert!(
        report["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("2 matching projects"))
    );
}

#[test]
fn discovery_reports_registry_matches_as_unverified_candidates() {
    let fixture = Fixture::new();
    fs::write(
        fixture.0.join("ui/package.json"),
        r#"{"name":"ui","version":"9.0.0"}"#,
    )
    .unwrap();
    fs::write(
        fixture.0.join("web/package.json"),
        r#"{"devDependencies":{"ui":"^1.0.0"},"peerDependencies":{"ui":"^2.0.0"}}"#,
    )
    .unwrap();
    let report = discover(&fixture);
    assert_eq!(report["relationships"].as_array().unwrap().len(), 2);
    assert_eq!(report["relationships"][0]["kind"], "package_name_candidate");
    assert_eq!(report["relationships"][0]["provider_version"], "9.0.0");
    assert_eq!(
        report["workspace"]["projects"]["web"]["depends_on"],
        serde_json::json!(["ui"])
    );
}

#[test]
fn discovery_bounds_scan_and_surfaces_invalid_and_unsupported_manifests() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("ui/package.json"), "malformed").unwrap();
    fs::write(
        fixture.0.join("web/package.json"),
        r#"{"name":"web","dependencies":{"ui":"file:../missing"}}"#,
    )
    .unwrap();
    fs::write(fixture.0.join("Cargo.toml"), "[package]").unwrap();
    for dir in ["node_modules/fake", ".hidden", "target/fake", "a/b/c/d/e"] {
        let path = fixture.0.join(dir);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("package.json"), r#"{"name":"ignored"}"#).unwrap();
    }
    for index in 0..6 {
        fs::create_dir_all(fixture.0.join(format!("a/b/c/d/extra-{index}"))).unwrap();
    }
    let report = discover(&fixture);
    assert_eq!(report["projects"].as_array().unwrap().len(), 1);
    let warnings = report["warnings"].to_string();
    assert!(warnings.contains("7 directories not inspected"));
    assert_eq!(
        report["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|w| w.as_str().unwrap().starts_with("Scan limit:"))
            .count(),
        1
    );
    for expected in [
        "Skipped",
        "Unsupported manifest",
        "Scan limit",
        "Unresolved local dependency",
    ] {
        assert!(
            warnings.contains(expected),
            "missing {expected}: {warnings}"
        );
    }
}

#[cfg(unix)]
#[test]
fn discovery_skips_symlinked_directories_and_manifests() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    fs::write(fixture.0.join("ui/package.json"), r#"{"name":"ui"}"#).unwrap();
    symlink(
        fixture.0.join("ui/package.json"),
        fixture.0.join("web/package.json"),
    )
    .unwrap();
    symlink(&fixture.0, fixture.0.join("cycle")).unwrap();
    let report = discover(&fixture);
    assert_eq!(report["projects"].as_array().unwrap().len(), 1);
}

#[test]
fn discovery_rejects_empty_or_invalid_roots() {
    let fixture = Fixture::new();
    for root in [
        &fixture.0,
        &fixture.0.join("missing"),
        &fixture.0.join("workspace.json"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_traversiq"))
            .arg("discover")
            .arg(root)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
}

fn source_fixture() -> Fixture {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.0.join("ui/src")).unwrap();
    fs::create_dir_all(fixture.0.join("web/src")).unwrap();
    for (file, content) in [
        ("ui/package.json", r#"{"name":"ui","version":"1.0.0"}"#),
        (
            "ui/src/index.ts",
            "export { Button as Form } from './form';\nexport * from './cycle';",
        ),
        ("ui/src/cycle.ts", "export * from './index';"),
        (
            "ui/src/form.tsx",
            "export function Button() { return <form/>; }",
        ),
        (
            "web/tsconfig.json",
            r#"{"compilerOptions":{"paths":{"@/*":["./src/*"]}}}"#,
        ),
        (
            "web/src/use.tsx",
            "import { Form as UIForm } from 'ui';\nexport const Wrapper = () => <UIForm/>;",
        ),
        ("web/src/barrel.ts", "export { Wrapper } from './use';"),
        (
            "web/src/page.tsx",
            "import { Wrapper } from '@/barrel';\nexport default () => <Wrapper/>;",
        ),
        (
            "web/src/unrelated.ts",
            "// import { Form } from 'ui';\nimport { Other } from 'ui';\nconst text = \"import { Form } from 'ui'\";",
        ),
        ("web/src/broken.ts", "export function broken("),
    ] {
        fs::write(fixture.0.join(file), content).unwrap();
    }
    fixture
}

fn run_trace(fixture: &Fixture, entry: &str, symbol: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_traversiq"))
        .current_dir(&fixture.0)
        .args(["trace", "workspace.json", "local.json", "ui", entry, symbol])
        .output()
        .unwrap()
}

#[test]
fn source_trace_follows_export_aliases_and_downstream_file_imports() {
    let fixture = source_fixture();
    let output = run_trace(&fixture, "src/index.ts", "Form");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["entry_to_definition"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["path"],
        "src/form.tsx"
    );
    assert_eq!(report["direct_references"].as_array().unwrap().len(), 1);
    assert_eq!(report["direct_references"][0]["local_name"], "UIForm");
    assert_eq!(report["direct_references"][0]["location"]["line"], 1);
    assert_eq!(report["downstream_files"].as_array().unwrap().len(), 2);
    assert_eq!(
        report["downstream_files"][1]["location"]["path"],
        "src/page.tsx"
    );
    assert_eq!(
        report["downstream_files"][1]["evidence_chain"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert!(report["warnings"].to_string().contains("broken.ts"));
    assert_eq!(report["omitted_source_files"], 1);
    assert_eq!(report["coverage"], "partial_static_import_graph");
}

#[test]
fn source_trace_rejects_ambiguous_missing_and_out_of_root_exports() {
    let fixture = source_fixture();
    fs::write(fixture.0.join("ui/src/other.ts"), "export const Form = 1;").unwrap();
    fs::write(fixture.0.join("ui/src/first.ts"), "export const Form = 2;").unwrap();
    fs::write(
        fixture.0.join("ui/src/ambiguous.ts"),
        "export * from './first'; export * from './other';",
    )
    .unwrap();
    for (entry, symbol) in [
        ("src/ambiguous.ts", "Form"),
        ("src/index.ts", "Missing"),
        ("../web/src/use.tsx", "Wrapper"),
    ] {
        let output = run_trace(&fixture, entry, symbol);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn namespace_imports_are_candidates_not_named_symbol_references() {
    let fixture = source_fixture();
    fs::write(
        fixture.0.join("web/src/namespace.ts"),
        "import * as UI from 'ui'; export const unused = UI.Other;",
    )
    .unwrap();
    let output = run_trace(&fixture, "src/index.ts", "Form");
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let reference = report["direct_references"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["local_name"] == "UI")
        .unwrap();
    assert_eq!(reference["kind"], "namespace_or_star_candidate");
}

#[test]
fn source_trace_includes_one_hop_public_wrapper_candidates() {
    let fixture = source_fixture();
    fs::write(
        fixture.0.join("ui/src/wrapper.tsx"),
        "import { Button } from './form'; export const Subscribe = () => <Button/>;",
    )
    .unwrap();
    fs::write(
        fixture.0.join("ui/src/index.ts"),
        "export { Button as Form } from './form'; export { Subscribe } from './wrapper';",
    )
    .unwrap();
    fs::write(
        fixture.0.join("web/src/subscribe.tsx"),
        "import { Subscribe } from 'ui'; export default () => <Subscribe/>;",
    )
    .unwrap();
    let output = run_trace(&fixture, "src/index.ts", "Form");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["provider_references"][0]["location"]["path"],
        "src/wrapper.tsx"
    );
    assert_eq!(
        report["wrapper_candidates"][0]["exported_symbol"],
        "Subscribe"
    );
    let reference = report["direct_references"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["imported_symbol"] == "Subscribe")
        .unwrap();
    assert_eq!(reference["kind"], "public_wrapper_candidate");
}

#[test]
fn default_function_names_are_not_named_exports() {
    let fixture = source_fixture();
    fs::write(
        fixture.0.join("ui/src/default.tsx"),
        "export default function Form() { return <form/>; }",
    )
    .unwrap();
    assert!(
        !run_trace(&fixture, "src/default.tsx", "Form")
            .status
            .success()
    );
    assert!(
        run_trace(&fixture, "src/default.tsx", "default")
            .status
            .success()
    );
}

#[test]
fn explicit_exports_take_precedence_over_star_exports() {
    let fixture = source_fixture();
    fs::write(fixture.0.join("ui/src/other.ts"), "export const Form = 1;").unwrap();
    fs::write(
        fixture.0.join("ui/src/index.ts"),
        "export { Button as Form } from './form'; export * from './other';",
    )
    .unwrap();
    let output = run_trace(&fixture, "src/index.ts", "Form");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["entry_to_definition"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["path"],
        "src/form.tsx"
    );
}

#[test]
fn dotted_module_names_resolve_without_losing_the_suffix() {
    let fixture = source_fixture();
    fs::write(
        fixture.0.join("web/src/action.styles.ts"),
        "import { Form } from 'ui'; export const StyledForm = Form;",
    )
    .unwrap();
    fs::write(
        fixture.0.join("web/src/styled-consumer.ts"),
        "import { StyledForm } from './action.styles';",
    )
    .unwrap();
    let output = run_trace(&fixture, "src/index.ts", "Form");
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        report["downstream_files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["location"]["path"] == "src/styled-consumer.ts")
    );
    assert!(!report["warnings"].to_string().contains("action.styles"));
}

#[test]
fn grammar_errors_do_not_discard_unaffected_top_level_imports_or_exports() {
    let fixture = source_fixture();
    fs::write(fixture.0.join("web/src/partial.tsx"), "import { Form } from 'ui';\nconst Styled = styled.div<{color:string}>`color:red;`;\nexport const Wrapper = () => <Form/>;").unwrap();
    fs::write(
        fixture.0.join("web/src/partial-consumer.ts"),
        "import { Wrapper } from './partial';",
    )
    .unwrap();
    let output = run_trace(&fixture, "src/index.ts", "Form");
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["partial_source_files"], 1);
    assert!(report["warnings"].to_string().contains("Partial "));
    assert!(
        report["direct_references"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["location"]["path"] == "src/partial.tsx")
    );
    assert!(
        report["downstream_files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["location"]["path"] == "src/partial-consumer.ts")
    );
}

#[test]
fn complete_diagnostics_are_explicit_and_default_output_stays_bounded() {
    let fixture = source_fixture();
    let imports: String = (0..60)
        .map(|i| format!("import './missing-{i}';\n"))
        .collect();
    fs::write(fixture.0.join("web/src/missing-imports.ts"), imports).unwrap();
    let compact = run_trace(&fixture, "src/index.ts", "Form");
    let compact: serde_json::Value = serde_json::from_slice(&compact.stdout).unwrap();
    assert_eq!(compact["warnings"].as_array().unwrap().len(), 50);
    assert!(compact["omitted_warnings"].as_u64().unwrap() > 0);
    let full = Command::new(env!("CARGO_BIN_EXE_traversiq"))
        .current_dir(&fixture.0)
        .args([
            "trace",
            "workspace.json",
            "local.json",
            "ui",
            "src/index.ts",
            "Form",
            "--all-warnings",
        ])
        .output()
        .unwrap();
    assert!(full.status.success());
    let full: serde_json::Value = serde_json::from_slice(&full.stdout).unwrap();
    assert_eq!(full["omitted_warnings"], 0);
    assert!(full["warnings"].as_array().unwrap().len() > 50);
    assert_eq!(full["unresolved_local_modules"], 60);
}

fn api_fixture() -> Fixture {
    let fixture = source_fixture();
    fs::create_dir_all(fixture.0.join("ui/internal")).unwrap();
    fs::write(
        fixture.0.join("workspace.json"),
        r#"{"version":1,"projects":{"ui":{"depends_on":[]},"web":{"depends_on":[]}}}"#,
    )
    .unwrap();
    fs::write(fixture.0.join("ui/schema.graphql"), "type Query { tenant: TenantQuery!, other: Other! }\ntype TenantQuery { sitemapXml: String! }\ntype Other { sitemapXml: String! }").unwrap();
    fs::write(fixture.0.join("web/src/queries.ts"), "import { gql } from '@apollo/client';\nexport const GET_XML = gql`\nquery GetXml {\n t: tenant { ...Xml }\n}\nfragment Xml on TenantQuery {\n xml: sitemapXml\n}\n`;").unwrap();
    fs::write(
        fixture.0.join("web/src/xml-service.ts"),
        "import { GET_XML } from './queries'; export const query = GET_XML;",
    )
    .unwrap();
    fs::write(
        fixture.0.join("web/src/xml-route.ts"),
        "import { query } from './xml-service'; export const GET = query;",
    )
    .unwrap();
    fs::write(
        fixture.0.join("ui/internal/resolver.go"),
        r#"package api
// func (r *Fake) SitemapXml() string { return "fake" }
const example = "func (r *Fake) SitemapXml() string"
func (r *TenantResolver) SitemapXml() string { return r.routes.SitemapXml() }
func (r *Routes) SitemapXml() string { return r.service.GetSitemapXML() }
func (r *Service) GetSitemapXML() string { return "" }
"#,
    )
    .unwrap();
    fs::write(
        fixture.0.join("ui/internal/resolver_test.go"),
        "package api\nfunc (r *TestOnly) SitemapXml() string { return \"\" }",
    )
    .unwrap();
    fixture
}

fn run_api(fixture: &Fixture, schema: &str, field: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_traversiq"))
        .current_dir(&fixture.0)
        .args([
            "api-trace",
            "workspace.json",
            "local.json",
            "ui",
            schema,
            field,
        ])
        .output()
        .unwrap()
}

#[test]
fn api_trace_links_schema_scoped_fragments_aliases_and_go_candidates() {
    let fixture = api_fixture();
    let output = run_api(&fixture, "schema.graphql", "Query.tenant.sitemapXml");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["endpoint_binding"], "unverified");
    assert_eq!(report["schema_chain"][1]["owner"], "TenantQuery");
    assert_eq!(report["schema_chain"][1]["output_type"], "String!");
    assert_eq!(report["schema_chain"][1]["location"]["line"], 2);
    assert_eq!(report["query_matches"].as_array().unwrap().len(), 1);
    assert_eq!(report["query_matches"][0]["operation"], "GetXml");
    assert_eq!(report["query_matches"][0]["binding"], "GET_XML");
    assert_eq!(
        report["query_matches"][0]["response_path"],
        serde_json::json!(["t", "xml"])
    );
    assert_eq!(report["query_matches"][0]["location"]["line"], 7);
    assert!(
        report["query_imports"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["location"]["path"] == "src/xml-service.ts")
    );
    assert_eq!(report["resolver_candidates"].as_array().unwrap().len(), 3);
    assert_eq!(report["parsed_go_files"], 1);
    assert!(!report["resolver_candidates"].to_string().contains("Fake"));
    assert!(
        !report["resolver_candidates"]
            .to_string()
            .contains("TestOnly")
    );
    assert!(
        report["resolver_candidates"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["name"] == "GetSitemapXML" && r["kind"] == "selector_call_name_candidate")
    );
}

#[test]
fn api_trace_does_not_match_same_named_fields_in_other_schema_types_or_strings() {
    let fixture = api_fixture();
    fs::write(fixture.0.join("web/src/queries.ts"), "// gql`query { tenant { sitemapXml } }`\nconst fake = \"gql`query { tenant { sitemapXml } }`\";\nexport const OTHER = gql`query GetOther { other { sitemapXml } }`;").unwrap();
    let output = run_api(&fixture, "schema.graphql", "Query.tenant.sitemapXml");
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["query_matches"], serde_json::json!([]));
    assert_eq!(report["parsed_graphql_documents"], 1);
}

#[test]
fn api_trace_skips_dynamic_documents_and_reports_missing_or_cyclic_fragments() {
    let fixture = api_fixture();
    fs::write(fixture.0.join("web/src/queries.ts"), "export const A = gql`query { tenant { ${fragment} } }`;\nexport const B = gql`query { tenant { ...Missing } }`;\nexport const C = gql`query { tenant { ...Cycle } } fragment Cycle on TenantQuery { ...Cycle }`;\nexport const D = gql`query {`;").unwrap();
    let output = run_api(&fixture, "schema.graphql", "Query.tenant.sitemapXml");
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["skipped_graphql_documents"], 2);
    assert_eq!(report["query_matches"], serde_json::json!([]));
    let warnings = report["warnings"].to_string();
    assert!(warnings.contains("interpolation"));
    assert!(warnings.contains("Missing fragment"));
    assert!(warnings.contains("Cyclic fragment"));
}

#[test]
fn api_trace_rejects_unknown_ambiguous_and_out_of_scope_schema_entries() {
    let fixture = api_fixture();
    for (schema, path) in [
        ("schema.graphql", "Query.missing"),
        ("schema.graphql", "Query"),
        ("../web/tsconfig.json", "Query.tenant"),
    ] {
        let output = run_api(&fixture, schema, path);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
    fs::write(
        fixture.0.join("ui/schema.graphql"),
        "type Query { tenant: String tenant: String }",
    )
    .unwrap();
    assert!(
        !run_api(&fixture, "schema.graphql", "Query.tenant")
            .status
            .success()
    );
    fs::write(fixture.0.join("ui/schema.graphql"), "type Query {").unwrap();
    assert!(
        !run_api(&fixture, "schema.graphql", "Query.tenant")
            .status
            .success()
    );
}

#[test]
fn api_trace_respects_custom_query_roots_and_inline_fragments() {
    let fixture = api_fixture();
    fs::write(fixture.0.join("ui/schema.graphql"), "schema { query: Root } type Root { tenant: TenantQuery } type TenantQuery { sitemapXml: String }").unwrap();
    fs::write(
        fixture.0.join("web/src/queries.ts"),
        "export const GET_XML = gql`query { tenant { ... on TenantQuery { sitemapXml } } }`;",
    )
    .unwrap();
    let output = run_api(&fixture, "schema.graphql", "Root.tenant.sitemapXml");
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["query_matches"].as_array().unwrap().len(), 1);
}

#[test]
fn api_query_bindings_do_not_expand_unrelated_exports_from_the_same_line_or_barrel() {
    let fixture = api_fixture();
    fs::write(fixture.0.join("web/src/queries.ts"), "export const GET_XML = gql`query { tenant { sitemapXml } }`; export const OTHER = gql`query { other { sitemapXml } }`;").unwrap();
    fs::write(
        fixture.0.join("web/src/query-barrel.ts"),
        "export * from './queries';",
    )
    .unwrap();
    fs::write(
        fixture.0.join("web/src/xml-service.ts"),
        "import { GET_XML as Xml } from './query-barrel'; export const query = Xml;",
    )
    .unwrap();
    fs::write(
        fixture.0.join("web/src/unrelated-query-user.ts"),
        "import { OTHER } from './query-barrel'; export const data = OTHER;",
    )
    .unwrap();
    let output = run_api(&fixture, "schema.graphql", "Query.tenant.sitemapXml");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["query_imports"].as_array().unwrap().len(), 1);
    assert_eq!(report["query_imports"][0]["local_name"], "Xml");
    assert_eq!(
        report["query_imports"][0]["location"]["path"],
        "src/xml-service.ts"
    );
    assert_eq!(report["downstream_files"].as_array().unwrap().len(), 1);
    assert_eq!(
        report["downstream_files"][0]["location"]["path"],
        "src/xml-route.ts"
    );
}

#[test]
fn nested_query_bindings_are_not_confused_with_same_named_module_exports() {
    let fixture = api_fixture();
    fs::write(fixture.0.join("web/src/queries.ts"), "export const GET_XML = gql`query { other { sitemapXml } }`; function nested() { const GET_XML = gql`query { tenant { sitemapXml } }`; return GET_XML; }").unwrap();
    let output = run_api(&fixture, "schema.graphql", "Query.tenant.sitemapXml");
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["query_matches"].as_array().unwrap().len(), 1);
    assert!(report["query_matches"][0]["binding"].is_null());
    assert_eq!(report["query_imports"], serde_json::json!([]));
    assert!(
        report["warnings"]
            .to_string()
            .contains("no top-level query binding")
    );
}

fn report(output: Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn without_run_metrics(mut value: serde_json::Value) -> serde_json::Value {
    for key in ["elapsed_ms", "cache", "source_cache", "go_cache"] {
        value.as_object_mut().unwrap().remove(key);
    }
    value
}

#[test]
fn source_cache_reuses_partial_and_failed_parses_without_changing_evidence() {
    let fixture = source_fixture();
    fs::write(fixture.0.join("web/src/partial.tsx"), "import { Form } from 'ui';\nconst Styled = styled.div<{color:string}>`color:red;`;\nexport const Wrapper = () => <Form/>;").unwrap();
    let cold = report(run_trace(&fixture, "src/index.ts", "Form"));
    let warm = report(run_trace(&fixture, "src/index.ts", "Form"));
    assert_eq!(cold["cache"]["reused_files"], 0);
    assert_eq!(warm["cache"]["reparsed_files"], 0);
    assert_eq!(
        cold["cache"]["reparsed_files"],
        warm["cache"]["reused_files"]
    );
    assert_eq!(warm["partial_source_files"], 1);
    assert_eq!(warm["omitted_source_files"], 1);
    assert_eq!(without_run_metrics(cold), without_run_metrics(warm));
}

#[test]
fn source_cache_detects_same_size_same_timestamp_edits_and_deletions() {
    let fixture = source_fixture();
    report(run_trace(&fixture, "src/index.ts", "Form"));
    let path = fixture.0.join("web/src/use.tsx");
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    let source = fs::read_to_string(&path).unwrap();
    fs::write(&path, source.replace("from 'ui'", "from 'xx'")).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
    let changed = report(run_trace(&fixture, "src/index.ts", "Form"));
    assert_eq!(changed["cache"]["reparsed_files"], 1);
    assert!(changed["direct_references"].as_array().unwrap().is_empty());
    assert!(changed["downstream_files"].as_array().unwrap().is_empty());
    fs::remove_file(&path).unwrap();
    let deleted = report(run_trace(&fixture, "src/index.ts", "Form"));
    assert_eq!(deleted["cache"]["removed_files"], 1);
    assert_eq!(deleted["cache"]["reparsed_files"], 0);
    assert!(
        deleted["warnings"]
            .to_string()
            .contains("Unresolved local module")
    );
    let db =
        rusqlite::Connection::open(fixture.0.join("local.json.traversiq-cache.sqlite3")).unwrap();
    let rows: i64 = db
        .query_row(
            "SELECT count(*) FROM facts WHERE path=?1",
            [serde_json::to_string(&("web", &path)).unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, 0);
}

#[test]
fn cached_facts_use_current_aliases_dependencies_and_approved_roots() {
    let fixture = source_fixture();
    report(run_trace(&fixture, "src/index.ts", "Form"));
    fs::write(
        fixture.0.join("web/tsconfig.json"),
        r#"{"compilerOptions":{"paths":{"@/*":["./absent/*"]}}}"#,
    )
    .unwrap();
    let changed = report(run_trace(&fixture, "src/index.ts", "Form"));
    assert_eq!(changed["cache"]["reparsed_files"], 0);
    assert_eq!(changed["downstream_files"].as_array().unwrap().len(), 1);
    fs::write(
        fixture.0.join("workspace.json"),
        r#"{"version":1,"projects":{"ui":{"depends_on":[]},"web":{"depends_on":[]}}}"#,
    )
    .unwrap();
    let changed = report(run_trace(&fixture, "src/index.ts", "Form"));
    assert!(changed["direct_references"].as_array().unwrap().is_empty());
    fs::create_dir_all(fixture.0.join("other-web/src")).unwrap();
    fs::write(
        fixture.0.join("local.json"),
        r#"{"paths":{"ui":"ui","web":"other-web"}}"#,
    )
    .unwrap();
    let moved = report(run_trace(&fixture, "src/index.ts", "Form"));
    assert!(!moved["warnings"].to_string().contains("broken.ts"));
    assert!(moved["direct_references"].as_array().unwrap().is_empty());
}

#[test]
fn incompatible_or_malformed_cached_facts_are_reparsed() {
    let fixture = source_fixture();
    let cold = report(run_trace(&fixture, "src/index.ts", "Form"));
    let db =
        rusqlite::Connection::open(fixture.0.join("local.json.traversiq-cache.sqlite3")).unwrap();
    db.execute("UPDATE facts SET profile='old-parser'", [])
        .unwrap();
    let changed = report(run_trace(&fixture, "src/index.ts", "Form"));
    assert_eq!(changed["cache"]["reused_files"], 0);
    assert_eq!(
        without_run_metrics(cold.clone()),
        without_run_metrics(changed)
    );
    db.execute("UPDATE facts SET payload='invalid-json'", [])
        .unwrap();
    let changed = report(run_trace(&fixture, "src/index.ts", "Form"));
    assert_eq!(changed["cache"]["reused_files"], 0);
    assert_eq!(without_run_metrics(cold), without_run_metrics(changed));
}

#[test]
fn unavailable_cache_fails_without_returning_stale_evidence() {
    let fixture = source_fixture();
    report(run_trace(&fixture, "src/index.ts", "Form"));
    let path = fixture.0.join("local.json.traversiq-cache.sqlite3");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    let locked = run_trace(&fixture, "src/index.ts", "Form");
    assert!(!locked.status.success());
    assert!(locked.stdout.is_empty());
    assert!(String::from_utf8_lossy(&locked.stderr).contains("Cache unavailable"));
    db.execute_batch("ROLLBACK").unwrap();
    drop(db);
    fs::write(&path, "not a sqlite database").unwrap();
    let corrupt = run_trace(&fixture, "src/index.ts", "Form");
    assert!(!corrupt.status.success());
    assert!(corrupt.stdout.is_empty());
    assert_eq!(fs::read_to_string(path).unwrap(), "not a sqlite database");
}

#[test]
fn api_cache_preserves_evidence_and_refreshes_queries_go_and_schema() {
    let fixture = api_fixture();
    let cold = report(run_api(
        &fixture,
        "schema.graphql",
        "Query.tenant.sitemapXml",
    ));
    let warm = report(run_api(
        &fixture,
        "schema.graphql",
        "Query.tenant.sitemapXml",
    ));
    assert_eq!(warm["source_cache"]["reparsed_files"], 0);
    assert_eq!(warm["go_cache"]["reparsed_files"], 0);
    assert_eq!(warm["go_cache"]["reused_files"], 1);
    assert_eq!(without_run_metrics(cold), without_run_metrics(warm));
    let go = fixture.0.join("ui/internal/resolver.go");
    let source = fs::read_to_string(&go).unwrap();
    fs::write(&go, source.replace("SitemapXml", "DifferentX")).unwrap();
    let changed = report(run_api(
        &fixture,
        "schema.graphql",
        "Query.tenant.sitemapXml",
    ));
    assert_eq!(changed["go_cache"]["reparsed_files"], 1);
    assert!(
        changed["resolver_candidates"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    fs::remove_file(go).unwrap();
    let query = fixture.0.join("web/src/queries.ts");
    fs::write(
        &query,
        fs::read_to_string(&query)
            .unwrap()
            .replace("sitemapXml", "missingXml"),
    )
    .unwrap();
    let changed = report(run_api(
        &fixture,
        "schema.graphql",
        "Query.tenant.sitemapXml",
    ));
    assert_eq!(changed["go_cache"]["removed_files"], 1);
    assert_eq!(changed["source_cache"]["reparsed_files"], 1);
    assert!(changed["query_matches"].as_array().unwrap().is_empty());
    assert!(changed["query_imports"].as_array().unwrap().is_empty());
    assert!(changed["downstream_files"].as_array().unwrap().is_empty());
    fs::write(
        fixture.0.join("ui/schema.graphql"),
        "type Query { removed: String }",
    )
    .unwrap();
    let changed = run_api(&fixture, "schema.graphql", "Query.tenant.sitemapXml");
    assert!(!changed.status.success());
    assert!(changed.stdout.is_empty());
}

#[test]
fn overlapping_project_roots_keep_each_files_project_identity() {
    let fixture = source_fixture();
    fs::create_dir_all(fixture.0.join("ui/src/nested/src")).unwrap();
    fs::write(
        fixture.0.join("ui/src/nested/src/use.tsx"),
        "import { Form } from 'ui'; export const Page = () => <Form/>;",
    )
    .unwrap();
    fs::write(
        fixture.0.join("local.json"),
        r#"{"paths":{"ui":"ui","web":"ui/src/nested"}}"#,
    )
    .unwrap();
    let cold = report(run_trace(&fixture, "src/index.ts", "Form"));
    assert_eq!(cold["direct_references"].as_array().unwrap().len(), 1);
    assert_eq!(cold["direct_references"][0]["location"]["project"], "web");
    assert_eq!(
        cold["direct_references"][0]["location"]["path"],
        "src/use.tsx"
    );
    let warm = report(run_trace(&fixture, "src/index.ts", "Form"));
    assert_eq!(without_run_metrics(cold), without_run_metrics(warm));
}

struct McpClient {
    child: std::process::Child,
    input: std::process::ChildStdin,
    responses: std::sync::mpsc::Receiver<Result<serde_json::Value, String>>,
    next_id: u64,
}

impl McpClient {
    fn new(fixture: &Fixture) -> Self {
        Self::with_args(fixture, &["mcp", "workspace.json", "local.json"])
    }

    fn with_args(fixture: &Fixture, args: &[&str]) -> Self {
        use std::io::{BufRead, BufReader};
        use std::process::Stdio;
        let mut child = Command::new(env!("CARGO_BIN_EXE_traversiq"))
            .current_dir(&fixture.0)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let (send, responses) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let value = line
                    .map_err(|e| e.to_string())
                    .and_then(|line| serde_json::from_str(&line).map_err(|e| e.to_string()));
                if send.send(value).is_err() {
                    break;
                }
            }
        });
        let mut client = Self {
            child,
            input,
            responses,
            next_id: 0,
        };
        let initialized = client.request(
            "initialize",
            serde_json::json!({
                "protocolVersion":"2025-11-25", "capabilities":{},
                "clientInfo":{"name":"traversiq-test","version":"1"}
            }),
        );
        assert_eq!(initialized["result"]["serverInfo"]["name"], "traversiq");
        assert!(initialized["result"]["capabilities"]["tools"].is_object());
        client.send(serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        client
    }

    fn send(&mut self, value: serde_json::Value) {
        use std::io::Write;
        writeln!(self.input, "{value}").unwrap();
        self.input.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        self.next_id += 1;
        self.send(
            serde_json::json!({"jsonrpc":"2.0","id":self.next_id,"method":method,"params":params}),
        );
        let response = self
            .responses
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("MCP response timeout")
            .unwrap();
        assert_eq!(response["id"], self.next_id);
        response
    }

    fn call(&mut self, tool: &str, args: serde_json::Value) -> serde_json::Value {
        self.request(
            "tools/call",
            serde_json::json!({"name":tool,"arguments":args}),
        )
    }

    fn successful(&mut self, tool: &str, args: serde_json::Value) -> serde_json::Value {
        let response = self.call(tool, args);
        assert_eq!(response["result"]["isError"], false, "{response}");
        let result = &response["result"];
        let text: serde_json::Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text, result["structuredContent"]);
        text
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn mcp_negotiates_lists_tools_and_matches_cli_evidence() {
    let fixture = api_fixture();
    let mut client = McpClient::new(&fixture);
    let listed = client.request("tools/list", serde_json::json!({}));
    let tools = listed["result"]["tools"].as_array().unwrap();
    let names: std::collections::BTreeSet<_> =
        tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        std::collections::BTreeSet::from([
            "traversiq_workspace",
            "traversiq_impact",
            "traversiq_trace",
            "traversiq_api_trace"
        ])
    );
    for tool in tools {
        assert_eq!(tool["annotations"]["readOnlyHint"], true);
        assert_eq!(tool["inputSchema"]["type"], "object");
    }
    let workspace = client.successful("traversiq_workspace", serde_json::json!({}));
    assert!(workspace["projects"]["ui"].is_object());
    let mut cli = report(fixture.run("impact", &["ui"]));
    cli["declaration_source"] =
        serde_json::json!(fixture.0.join("workspace.json").canonicalize().unwrap());
    assert_eq!(
        cli,
        client.successful("traversiq_impact", serde_json::json!({"project":"ui"}))
    );
    let trace = client.successful(
        "traversiq_trace",
        serde_json::json!({"provider":"ui","entry":"src/index.ts","symbol":"Form"}),
    );
    assert_eq!(
        without_run_metrics(trace),
        without_run_metrics(report(run_trace(&fixture, "src/index.ts", "Form")))
    );
    let api = client.successful("traversiq_api_trace", serde_json::json!({"provider":"ui","schema_entry":"schema.graphql","field_path":"Query.tenant.sitemapXml"}));
    assert_eq!(
        without_run_metrics(api),
        without_run_metrics(report(run_api(
            &fixture,
            "schema.graphql",
            "Query.tenant.sitemapXml"
        )))
    );
    let ping = client.request("ping", serde_json::json!({}));
    assert!(ping["result"].is_object());
}

#[test]
fn mcp_rejects_bad_arguments_and_paths_then_recovers() {
    let fixture = api_fixture();
    let mut client = McpClient::new(&fixture);
    for (tool, args) in [
        ("unknown_tool", serde_json::json!({})),
        ("traversiq_impact", serde_json::json!({"project":42})),
        (
            "traversiq_trace",
            serde_json::json!({"provider":"ui","entry":"src/index.ts"}),
        ),
        (
            "traversiq_trace",
            serde_json::json!({"provider":"ui","entry":"src/index.ts","symbol":"Form","local":"another-workspace.json"}),
        ),
    ] {
        let response = client.call(tool, args);
        assert!(
            response["error"].is_object() || response["result"]["isError"] == true,
            "{response}"
        );
    }
    for (tool, args) in [
        (
            "traversiq_impact",
            serde_json::json!({"project":"unapproved-project"}),
        ),
        (
            "traversiq_trace",
            serde_json::json!({"provider":"ui","entry":"../web/src/use.tsx","symbol":"Form"}),
        ),
        (
            "traversiq_api_trace",
            serde_json::json!({"provider":"ui","schema_entry":"../local.json","field_path":"Query.tenant.sitemapXml"}),
        ),
    ] {
        let response = client.call(tool, args);
        assert_eq!(response["result"]["isError"], true, "{response}");
        assert!(response["result"]["structuredContent"].is_null());
    }
    assert!(
        client.successful("traversiq_workspace", serde_json::json!({}))["projects"].is_object()
    );
}

#[test]
fn mcp_reads_current_files_and_config_between_requests() {
    let fixture = source_fixture();
    let mut client = McpClient::new(&fixture);
    let args = serde_json::json!({"provider":"ui","entry":"src/index.ts","symbol":"Form"});
    let first = client.successful("traversiq_trace", args.clone());
    assert_eq!(first["direct_references"].as_array().unwrap().len(), 1);
    fs::remove_file(fixture.0.join("web/src/use.tsx")).unwrap();
    let second = client.successful("traversiq_trace", args);
    assert!(second["direct_references"].as_array().unwrap().is_empty());
    assert_eq!(second["cache"]["removed_files"], 1);
    fs::write(fixture.0.join("workspace.json"), "{}").unwrap();
    let response = client.call("traversiq_impact", serde_json::json!({"project":"ui"}));
    assert_eq!(response["result"]["isError"], true);
}

#[test]
fn mcp_invalid_startup_config_fails_without_protocol_output() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("workspace.json"), "{}").unwrap();
    let output = fixture.run("mcp", &[]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("missing field"));
}

#[test]
fn public_demo_is_self_contained_and_reports_documented_consumers() {
    fn copy_demo(source: &std::path::Path, target: &std::path::Path) {
        fs::create_dir_all(target).unwrap();
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                copy_demo(&path, &target.join(entry.file_name()));
            } else if matches!(
                path.extension().and_then(|s| s.to_str()),
                Some("json" | "ts" | "tsx" | "go" | "graphql")
            ) {
                fs::copy(path, target.join(entry.file_name())).unwrap();
            }
        }
    }
    let fixture = Fixture::new();
    copy_demo(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/demo"),
        &fixture.0,
    );
    assert!(fixture.run("validate", &[]).status.success());
    let impact = report(fixture.run("impact", &["ui"]));
    assert_eq!(impact["affected"].as_array().unwrap().len(), 2);
    let source = report(run_trace(&fixture, "src/index.ts", "Button"));
    assert_eq!(source["direct_references"].as_array().unwrap().len(), 2);
    assert_eq!(source["downstream_files"].as_array().unwrap().len(), 1);
    let api = report(fixture.run(
        "api-trace",
        &["api", "schema.graphql", "Query.catalog.summary"],
    ));
    assert_eq!(api["query_matches"].as_array().unwrap().len(), 1);
    assert_eq!(api["query_imports"].as_array().unwrap().len(), 1);
    assert_eq!(api["downstream_files"].as_array().unwrap().len(), 1);
    assert_eq!(api["resolver_candidates"].as_array().unwrap().len(), 2);
}

fn hook(
    fixture: &Fixture,
    patch: &str,
    session: &str,
    turn: &str,
    call: &str,
) -> serde_json::Value {
    let event = serde_json::json!({"hook_event_name":"PreToolUse", "session_id":session,
        "turn_id":turn, "tool_use_id":call, "cwd":fixture.0,
        "tool_name":"apply_patch", "tool_input":{"command":patch}});
    checkpoint_event(fixture, event)
}

fn checkpoint_event(fixture: &Fixture, event: serde_json::Value) -> serde_json::Value {
    checkpoint_command(
        fixture,
        event,
        &["pre-edit", "workspace.json", "local.json"],
    )
}

fn checkpoint_command(
    fixture: &Fixture,
    event: serde_json::Value,
    args: &[&str],
) -> serde_json::Value {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::new(env!("CARGO_BIN_EXE_traversiq"))
        .current_dir(&fixture.0)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(event.to_string().as_bytes())
        .unwrap();
    report(child.wait_with_output().unwrap())
}

const FORM_PATCH: &str = "*** Begin Patch\n*** Update File: ui/src/form.tsx\n@@\n-export function Button() { return <form/>; }\n+export function Button() { return null; }\n*** End Patch";

fn denied(value: &serde_json::Value) -> bool {
    value["hookSpecificOutput"]["permissionDecision"] == "deny"
}

#[test]
fn checkpoint_briefs_before_an_identical_retry_and_consumes_the_receipt() {
    let fixture = source_fixture();
    fs::write(
        fixture.0.join("web/src/subpath.ts"),
        "export * from 'ui/components';",
    )
    .unwrap();
    fs::write(
        fixture.0.join("web/src/other-package.ts"),
        "import * as Other from 'ui-extra';",
    )
    .unwrap();
    let before = fs::read(fixture.0.join("ui/src/form.tsx")).unwrap();
    let first = hook(&fixture, FORM_PATCH, "session", "turn", "one");
    assert!(denied(&first));
    let reason = first["hookSpecificOutput"]["permissionDecisionReason"]
        .as_str()
        .unwrap();
    assert!(reason.contains("web") && reason.contains("src/page.tsx"));
    assert!(reason.contains("partial") && reason.contains("broken.ts"));
    assert!(reason.contains("src/subpath.ts") && !reason.contains("src/other-package.ts"));
    assert!(
        denied(&hook(&fixture, FORM_PATCH, "session", "turn", "one")),
        "same invocation cannot acknowledge its own briefing"
    );
    let retry = hook(&fixture, FORM_PATCH, "session", "turn", "two");
    assert!(!denied(&retry));
    assert!(
        retry["hookSpecificOutput"]
            .get("permissionDecision")
            .is_none(),
        "never override normal authorization with allow"
    );
    assert!(denied(&hook(
        &fixture, FORM_PATCH, "session", "turn", "three"
    )));
    assert_eq!(fs::read(fixture.0.join("ui/src/form.tsx")).unwrap(), before);
}

#[test]
fn checkpoint_rebriefs_for_changed_bodies_proposals_sessions_turns_and_expiry() {
    let fixture = source_fixture();
    assert!(denied(&hook(&fixture, FORM_PATCH, "a", "a", "1")));
    fs::write(
        fixture.0.join("web/src/page.tsx"),
        "// changed consumer body\nexport const page = 1;",
    )
    .unwrap();
    assert!(denied(&hook(&fixture, FORM_PATCH, "a", "a", "2")));
    assert!(denied(&hook(
        &fixture,
        &FORM_PATCH.replace("return null", "return false"),
        "a",
        "a",
        "3"
    )));
    assert!(denied(&hook(&fixture, FORM_PATCH, "b", "a", "4")));
    assert!(denied(&hook(&fixture, FORM_PATCH, "b", "b", "5")));
    let path = fixture.0.join("local.json.traversiq-checkpoint.json");
    let mut receipt: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    receipt["issued"] = serde_json::json!(0);
    fs::write(&path, receipt.to_string()).unwrap();
    assert!(denied(&hook(&fixture, FORM_PATCH, "b", "b", "6")));
    fs::write(
        fixture.0.join("ui/package.json"),
        r#"{"name":"renamed-ui"}"#,
    )
    .unwrap();
    assert!(denied(&hook(&fixture, FORM_PATCH, "b", "b", "7")));
}

#[test]
fn checkpoint_handles_add_delete_move_and_rejects_unapproved_or_invalid_paths() {
    let fixture = source_fixture();
    let patch = "*** Begin Patch\n*** Add File: ui/src/new/added.ts\n+export const newValue = 1;\n*** Update File: ui/src/form.tsx\n*** Move to: web/src/moved.tsx\n@@\n-old\n+new\n*** Delete File: web/src/page.tsx\n*** End Patch";
    assert!(denied(&hook(&fixture, patch, "a", "a", "1")));
    assert!(!denied(&hook(&fixture, patch, "a", "a", "2")));
    assert!(!fixture.0.join("ui/src/new").exists());
    for patch in [
        "*** Begin Patch\n*** Add File: outside.ts\n+hi\n*** End Patch",
        "*** Begin Patch\n*** Add File: ui/../outside.ts\n+hi\n*** End Patch",
        "*** Begin Patch\n*** Update File: ui/missing.ts\n@@\n-old\n+new\n*** End Patch",
        "not a patch",
    ] {
        for call in ["3", "4"] {
            let result = hook(&fixture, patch, "a", "a", call);
            assert!(denied(&result));
            assert!(result.to_string().contains("incomplete"));
        }
    }
    fs::write(fixture.0.join("workspace.json"), "malformed").unwrap();
    assert!(
        hook(&fixture, FORM_PATCH, "a", "a", "5")
            .to_string()
            .contains("incomplete")
    );
}

#[cfg(unix)]
#[test]
fn checkpoint_rejects_symlinks_and_bounds_stuck_analysis() {
    use std::os::unix::fs::symlink;
    let fixture = source_fixture();
    symlink(fixture.0.join("web/src"), fixture.0.join("ui/link")).unwrap();
    let patch = "*** Begin Patch\n*** Add File: ui/link/new.ts\n+hi\n*** End Patch";
    assert!(
        hook(&fixture, patch, "a", "a", "1")
            .to_string()
            .contains("symlink")
    );
    fs::remove_file(fixture.0.join("workspace.json")).unwrap();
    assert!(
        Command::new("mkfifo")
            .arg(fixture.0.join("workspace.json"))
            .status()
            .unwrap()
            .success()
    );
    let start = std::time::Instant::now();
    let value = hook(&fixture, FORM_PATCH, "a", "a", "2");
    assert!(denied(&value) && value.to_string().contains("5 seconds"));
    assert!(
        start.elapsed().as_secs() < 10,
        "worker must not keep the CLI alive after timeout"
    );
}

fn claude_event(
    fixture: &Fixture,
    tool: &str,
    input: serde_json::Value,
    call: &str,
) -> serde_json::Value {
    serde_json::json!({"hook_event_name":"PreToolUse", "session_id":"claude-session",
        "prompt_id":"prompt-1", "tool_use_id":call, "cwd":fixture.0,
        "tool_name":tool, "tool_input":input})
}

#[test]
fn claude_checkpoint_handles_edits_and_writes_without_granting_permission() {
    let fixture = source_fixture();
    let root = fixture.0.canonicalize().unwrap();
    for (tool, input) in [
        (
            "Edit",
            serde_json::json!({"file_path":root.join("ui/src/form.tsx"),
            "old_string":"<form/>", "new_string":"null", "replace_all":false}),
        ),
        (
            "Write",
            serde_json::json!({"file_path":root.join("ui/src/new/value.ts"),
            "content":"export const value = 1;"}),
        ),
        (
            "Edit",
            serde_json::json!({"file_path":root.join("ui/src/new/created.ts"),
            "old_string":"", "new_string":"export const created = 1;"}),
        ),
    ] {
        let first = checkpoint_event(&fixture, claude_event(&fixture, tool, input.clone(), "one"));
        assert!(denied(&first));
        let reason = first["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .unwrap();
        assert!(reason.contains(&format!("identical {tool}")) && reason.contains("web"));
        let retry = checkpoint_event(&fixture, claude_event(&fixture, tool, input.clone(), "two"));
        assert!(
            retry["hookSpecificOutput"]
                .get("permissionDecision")
                .is_none()
        );
        assert!(denied(&checkpoint_event(
            &fixture,
            claude_event(&fixture, tool, input, "three")
        )));
    }
    assert!(!root.join("ui/src/new").exists());
    assert!(
        fs::read_to_string(root.join("ui/src/form.tsx"))
            .unwrap()
            .contains("<form/>")
    );
}

#[test]
fn claude_receipts_are_scoped_to_exact_inputs_prompts_and_agents() {
    let fixture = source_fixture();
    let root = fixture.0.canonicalize().unwrap();
    let input = serde_json::json!({"file_path":root.join("ui/src/form.tsx"),
        "old_string":"<form/>", "new_string":"null", "replace_all":false});
    let initial = claude_event(&fixture, "Edit", input.clone(), "one");
    assert!(denied(&checkpoint_event(&fixture, initial.clone())));
    let mut changed = initial.clone();
    changed["tool_use_id"] = serde_json::json!("two");
    changed["tool_input"]["replace_all"] = serde_json::json!(true);
    assert!(denied(&checkpoint_event(&fixture, changed.clone())));
    changed["tool_use_id"] = serde_json::json!("three");
    changed["prompt_id"] = serde_json::json!("prompt-2");
    assert!(denied(&checkpoint_event(&fixture, changed.clone())));
    changed["tool_use_id"] = serde_json::json!("four");
    changed["agent_id"] = serde_json::json!("agent-1");
    assert!(denied(&checkpoint_event(&fixture, changed.clone())));
    changed["tool_use_id"] = serde_json::json!("five");
    assert!(!denied(&checkpoint_event(&fixture, changed)));
    let write =
        serde_json::json!({"file_path":root.join("ui/src/form.tsx"), "content":"replacement"});
    assert!(denied(&checkpoint_event(
        &fixture,
        claude_event(&fixture, "Write", write.clone(), "six")
    )));
    fs::write(root.join("ui/src/form.tsx"), "export const changed = 1;").unwrap();
    assert!(denied(&checkpoint_event(
        &fixture,
        claude_event(&fixture, "Write", write, "seven")
    )));
}

#[test]
fn claude_checkpoint_rejects_missing_scope_malformed_inputs_and_unapproved_paths() {
    let fixture = source_fixture();
    let root = fixture.0.canonicalize().unwrap();
    let input = serde_json::json!({"file_path":root.join("ui/src/form.tsx"),
        "old_string":"<form/>", "new_string":"null"});
    let mut missing_scope = claude_event(&fixture, "Edit", input, "one");
    missing_scope.as_object_mut().unwrap().remove("prompt_id");
    let result = checkpoint_event(&fixture, missing_scope);
    assert!(denied(&result) && result.to_string().contains("Claude prompt_id"));
    for (tool, input) in [
        (
            "Edit",
            serde_json::json!({"file_path":root.join("ui/src/form.tsx"), "old_string":1,"new_string":"null"}),
        ),
        (
            "Edit",
            serde_json::json!({"file_path":root.join("ui/src/form.tsx"), "old_string":"x","new_string":"y","replace_all":"yes"}),
        ),
        (
            "Write",
            serde_json::json!({"file_path":root.join("ui/src/form.tsx")}),
        ),
        (
            "Write",
            serde_json::json!({"file_path":"ui/new.ts", "content":"x"}),
        ),
        (
            "Write",
            serde_json::json!({"file_path":root.join("outside.ts"), "content":"x"}),
        ),
        (
            "NotebookEdit",
            serde_json::json!({"notebook_path":root.join("ui/book.ipynb")}),
        ),
    ] {
        let result = checkpoint_event(&fixture, claude_event(&fixture, tool, input, "two"));
        assert!(denied(&result) && result.to_string().contains("incomplete"));
    }
}

fn configure_plugin(fixture: &Fixture) -> Output {
    Command::new(env!("CARGO_BIN_EXE_traversiq"))
        .current_dir(&fixture.0)
        .args(["plugin", "configure", "workspace.json", "local.json"])
        .output()
        .unwrap()
}

#[test]
fn plugin_setup_validates_without_overwriting_and_starts_the_shared_mcp() {
    let fixture = source_fixture();
    let configured = report(configure_plugin(&fixture));
    assert_eq!(configured["project_count"], 2);
    let path = fixture.0.join(".traversiq/config.json");
    let before = fs::read(&path).unwrap();
    let pointer: serde_json::Value = serde_json::from_slice(&before).unwrap();
    assert_eq!(
        pointer["workspace"],
        fixture
            .0
            .join("workspace.json")
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap()
    );
    assert!(!configure_plugin(&fixture).status.success());
    assert_eq!(fs::read(&path).unwrap(), before);
    let mut client = McpClient::with_args(&fixture, &["plugin", "mcp"]);
    let workspace = client.successful("traversiq_workspace", serde_json::json!({}));
    assert!(workspace["projects"]["ui"].is_object());
    let broken = Fixture::new();
    fs::write(broken.0.join("local.json"), "invalid").unwrap();
    assert!(!configure_plugin(&broken).status.success());
    assert!(!broken.0.join(".traversiq").exists());
}

#[test]
fn plugin_hooks_route_by_event_cwd_and_reject_a_broken_nearer_pointer() {
    let fixture = source_fixture();
    let input = serde_json::json!({"file_path":fixture.0.canonicalize().unwrap().join("ui/src/form.tsx"),
        "old_string":"<form/>", "new_string":"null"});
    let mut event = claude_event(&fixture, "Edit", input, "one");
    event["cwd"] = serde_json::json!(fixture.0.join("ui/src"));
    let unchecked = checkpoint_command(&fixture, event.clone(), &["plugin", "pre-edit"]);
    assert!(!denied(&unchecked) && unchecked.to_string().contains("unchecked"));
    let unconfigured_mcp = Command::new(env!("CARGO_BIN_EXE_traversiq"))
        .current_dir(&fixture.0)
        .args(["plugin", "mcp"])
        .output()
        .unwrap();
    assert!(!unconfigured_mcp.status.success() && unconfigured_mcp.stdout.is_empty());
    assert!(configure_plugin(&fixture).status.success());
    let first = checkpoint_command(&fixture, event.clone(), &["plugin", "pre-edit"]);
    assert!(denied(&first) && first.to_string().contains("src/page.tsx"));
    event["tool_use_id"] = serde_json::json!("two");
    assert!(!denied(&checkpoint_command(
        &fixture,
        event.clone(),
        &["plugin", "pre-edit"]
    )));
    let nearer = fixture.0.join("ui/.traversiq");
    fs::create_dir_all(&nearer).unwrap();
    fs::write(nearer.join("config.json"), "invalid pointer").unwrap();
    let failure = checkpoint_command(&fixture, event.clone(), &["plugin", "pre-edit"]);
    assert!(denied(&failure) && failure.to_string().contains("incomplete"));
    fs::write(
        nearer.join("config.json"),
        r#"{"workspace":"../../workspace.json","local":"../../local.json"}"#,
    )
    .unwrap();
    let settings = traversiq::plugin::settings(&fixture.0.join("ui/src"))
        .unwrap()
        .unwrap();
    assert_eq!(
        settings.workspace.canonicalize().unwrap(),
        fixture.0.join("workspace.json").canonicalize().unwrap()
    );
    assert!(denied(&checkpoint_command(
        &fixture,
        event,
        &["plugin", "pre-edit"]
    )));
}

#[cfg(unix)]
#[test]
fn plugin_setup_does_not_write_through_a_symlink_configuration_directory() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let other = Fixture::new();
    symlink(&other.0, fixture.0.join(".traversiq")).unwrap();
    assert!(!configure_plugin(&fixture).status.success());
    assert!(!other.0.join("config.json").exists());
    fs::write(
        other.0.join("config.json"),
        r#"{"workspace":"workspace.json","local":"local.json"}"#,
    )
    .unwrap();
    assert!(
        traversiq::plugin::settings(&fixture.0)
            .unwrap_err()
            .contains("symlink")
    );
}
