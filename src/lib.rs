pub mod api;
mod cache;
pub mod discovery;
pub mod mcp;
pub mod source;

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Workspace {
    pub version: u32,
    pub projects: BTreeMap<String, Project>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub depends_on: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Local {
    paths: BTreeMap<String, PathBuf>,
}

#[derive(Debug, Serialize)]
pub struct Impact {
    pub changed_project: String,
    pub coverage: &'static str,
    pub affected: Vec<Affected>,
    pub limitations: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct Affected {
    pub project: String,
    /// Consumer first, proposed-change project last.
    pub dependency_chain: Vec<String>,
}

impl Workspace {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err(format!(
                "unsupported workspace version {} (expected 1)",
                self.version
            ));
        }
        if self.projects.is_empty() {
            return Err("workspace must contain at least one project".into());
        }
        for (id, project) in &self.projects {
            if id.is_empty()
                || !id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
            {
                return Err(format!(
                    "invalid project ID {id:?}: use letters, digits, _ or -"
                ));
            }
            let mut seen = BTreeSet::new();
            for dependency in &project.depends_on {
                if dependency == id {
                    return Err(format!("project {id} cannot depend on itself"));
                }
                if !self.projects.contains_key(dependency) {
                    return Err(format!(
                        "project {id} depends on unknown project {dependency}"
                    ));
                }
                if !seen.insert(dependency) {
                    return Err(format!("project {id} repeats dependency {dependency}"));
                }
            }
        }
        Ok(())
    }

    pub fn impact(&self, changed: &str) -> Result<Impact, String> {
        self.validate()?;
        if !self.projects.contains_key(changed) {
            return Err(format!("unknown project {changed}"));
        }
        let mut queue = VecDeque::from([vec![changed.to_owned()]]);
        let mut visited = BTreeSet::from([changed.to_owned()]);
        let mut affected = Vec::new();
        // Traverse toward consumers, preserving a shortest declared dependency chain.
        while let Some(chain) = queue.pop_front() {
            for (id, project) in &self.projects {
                if project.depends_on.contains(&chain[0]) && visited.insert(id.clone()) {
                    let mut next = vec![id.clone()];
                    next.extend(chain.iter().cloned());
                    affected.push(Affected {
                        project: id.clone(),
                        dependency_chain: next.clone(),
                    });
                    queue.push_back(next);
                }
            }
        }
        Ok(Impact {
            changed_project: changed.to_owned(),
            coverage: "declared_project_dependencies_only",
            affected,
            limitations: vec![
                "Candidates come from workspace declarations, not source-code analysis.",
                "Symbols, runtime behavior, dependency versions, API contracts and documentation are not checked.",
                "An empty result does not establish that the change is safe.",
            ],
        })
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let bytes = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display()))
}

pub fn load_workspace(config: &Path, local: &Path) -> Result<Workspace, String> {
    let workspace: Workspace = read_json(config)?;
    workspace.validate()?;
    load_project_paths(&workspace, local)?;
    Ok(workspace)
}

pub fn load_project_paths(
    workspace: &Workspace,
    local: &Path,
) -> Result<BTreeMap<String, PathBuf>, String> {
    let local_config: Local = read_json(local)?;
    for id in local_config.paths.keys() {
        if !workspace.projects.contains_key(id) {
            return Err(format!("local paths contain unknown project {id}"));
        }
    }
    let base = local.parent().unwrap_or_else(|| Path::new("."));
    let mut roots = BTreeSet::new();
    let mut paths = BTreeMap::new();
    for id in workspace.projects.keys() {
        let path = local_config
            .paths
            .get(id)
            .ok_or_else(|| format!("missing local path for project {id}"))?;
        if path.as_os_str().is_empty() {
            return Err(format!("empty local path for project {id}"));
        }
        let resolved = base
            .join(path)
            .canonicalize()
            .map_err(|error| format!("project {id}, path {}: {error}", path.display()))?;
        if !resolved.is_dir() {
            return Err(format!(
                "project {id} path is not a directory: {}",
                resolved.display()
            ));
        }
        if !roots.insert(resolved.clone()) {
            return Err(format!(
                "project {id} shares a directory with another project"
            ));
        }
        paths.insert(id.clone(), resolved);
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(json: &str) -> Workspace {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn impact_follows_consumers_not_providers() {
        let graph = workspace(
            r#"{"version":1,"projects":{
            "ui":{"depends_on":[]},"web":{"depends_on":["ui","api"]},
            "api":{"depends_on":[]},"dashboard":{"depends_on":["web"]}}}"#,
        );
        let impact = graph.impact("ui").unwrap();
        assert_eq!(
            impact
                .affected
                .iter()
                .map(|p| p.project.as_str())
                .collect::<Vec<_>>(),
            ["web", "dashboard"]
        );
        assert_eq!(
            impact.affected[1].dependency_chain,
            ["dashboard", "web", "ui"]
        );
        assert!(graph.impact("dashboard").unwrap().affected.is_empty());
    }

    #[test]
    fn cycles_terminate_and_diamonds_have_no_duplicates() {
        let graph = workspace(
            r#"{"version":1,"projects":{
            "a":{"depends_on":["d"]},"b":{"depends_on":["a"]},
            "c":{"depends_on":["a"]},"d":{"depends_on":["b","c"]}}}"#,
        );
        let impact = graph.impact("a").unwrap();
        assert_eq!(impact.affected.len(), 3);
        assert_eq!(impact.affected[2].dependency_chain, ["d", "b", "a"]);
    }

    #[test]
    fn invalid_graphs_fail_before_analysis() {
        for json in [
            r#"{"version":2,"projects":{"a":{"depends_on":[]}}}"#,
            r#"{"version":1,"projects":{}}"#,
            r#"{"version":1,"projects":{"a":{"depends_on":["missing"]}}}"#,
            r#"{"version":1,"projects":{"a":{"depends_on":["a"]}}}"#,
            r#"{"version":1,"projects":{"a":{"depends_on":[]},"b":{"depends_on":["a","a"]}}}"#,
            r#"{"version":1,"projects":{"../escape":{"depends_on":[]}}}"#,
        ] {
            assert!(workspace(json).validate().is_err(), "accepted {json}");
        }
    }

    #[test]
    fn unknown_project_and_unknown_fields_are_errors() {
        let graph = workspace(r#"{"version":1,"projects":{"a":{"depends_on":[]}}}"#);
        assert!(graph.impact("missing").is_err());
        assert!(
            serde_json::from_str::<Workspace>(r#"{"version":1,"projects":{},"typo":true}"#)
                .is_err()
        );
    }
}
