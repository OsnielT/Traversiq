use crate::{Project, Workspace};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_DEPTH: usize = 4;
const MAX_DIRECTORIES: usize = 10_000;
const MAX_MANIFEST_BYTES: u64 = 1_048_576;
const SECTIONS: [&str; 4] = [
    "dependencies",
    "devDependencies",
    "peerDependencies",
    "optionalDependencies",
];

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    name: Option<String>,
    version: Option<String>,
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
    #[serde(default)]
    dev_dependencies: BTreeMap<String, String>,
    #[serde(default)]
    peer_dependencies: BTreeMap<String, String>,
    #[serde(default)]
    optional_dependencies: BTreeMap<String, String>,
}

impl Manifest {
    fn sections(&self) -> [&BTreeMap<String, String>; 4] {
        [
            &self.dependencies,
            &self.dev_dependencies,
            &self.peer_dependencies,
            &self.optional_dependencies,
        ]
    }
}

#[derive(Serialize)]
pub struct Discovery {
    pub status: &'static str,
    pub root: PathBuf,
    pub workspace: Workspace,
    pub local: LocalProposal,
    pub projects: Vec<DiscoveredProject>,
    pub relationships: Vec<Relationship>,
    pub warnings: Vec<String>,
    pub limitations: Vec<&'static str>,
}

#[derive(Serialize)]
pub struct LocalProposal {
    pub paths: BTreeMap<String, PathBuf>,
}

#[derive(Serialize)]
pub struct DiscoveredProject {
    pub id: String,
    pub package_name: Option<String>,
    pub version: Option<String>,
    pub manifest: PathBuf,
}

#[derive(Serialize)]
pub struct Relationship {
    pub consumer: String,
    pub provider: String,
    pub kind: &'static str,
    pub source: PathBuf,
    pub json_pointer: String,
    pub dependency: String,
    pub requirement: String,
    pub provider_version: Option<String>,
    pub needs_confirmation: bool,
}

pub(crate) fn excluded(name: &str) -> bool {
    name.starts_with('.')
        || matches!(
            name,
            "node_modules"
                | "target"
                | "dist"
                | "build"
                | "vendor"
                | "coverage"
                | "venv"
                | "__pycache__"
        )
}

fn scan(
    dir: &Path,
    depth: usize,
    visited: &mut usize,
    files: &mut Vec<PathBuf>,
    warnings: &mut Vec<String>,
) {
    *visited += 1;
    if *visited > MAX_DIRECTORIES {
        return;
    }
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            warnings.push(format!("Cannot inspect {}: {error}", dir.display()));
            return;
        }
    };
    let mut entries: Vec<_> = entries
        .filter_map(|entry| match entry {
            Ok(entry) => Some(entry),
            Err(error) => {
                warnings.push(format!(
                    "Cannot inspect entry in {}: {error}",
                    dir.display()
                ));
                None
            }
        })
        .collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if excluded(&name) {
            continue;
        }
        let kind = match entry.file_type() {
            Ok(kind) => kind,
            Err(error) => {
                warnings.push(format!(
                    "Cannot inspect {}: {error}",
                    entry.path().display()
                ));
                continue;
            }
        };
        if kind.is_symlink() {
            continue;
        }
        if kind.is_file() && name == "package.json" {
            files.push(entry.path());
        }
        if kind.is_file()
            && matches!(
                name.as_ref(),
                "Cargo.toml" | "go.mod" | "pyproject.toml" | "pom.xml"
            )
        {
            warnings.push(format!("Unsupported manifest: {}", entry.path().display()));
        }
        if kind.is_dir() {
            if depth < MAX_DEPTH && *visited < MAX_DIRECTORIES {
                scan(&entry.path(), depth + 1, visited, files, warnings);
            } else {
                warnings.push(format!(
                    "Scan limit: directory not inspected: {}",
                    entry.path().display()
                ));
            }
        }
    }
}

fn read_manifest(path: &Path) -> Result<Manifest, String> {
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err("manifest exceeds 1 MiB".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

fn project_id(relative: &Path) -> String {
    // Encode separators and punctuation rather than merging distinct directory names.
    let text = relative.to_string_lossy();
    let mut id = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || byte == b'-' {
            id.push(byte as char);
        } else {
            id.push_str(&format!("_{byte:02x}"));
        }
    }
    if id.is_empty() { "root".into() } else { id }
}

pub fn discover(root: &Path) -> Result<Discovery, String> {
    let root = root
        .canonicalize()
        .map_err(|e| format!("{}: {e}", root.display()))?;
    if !root.is_dir() {
        return Err(format!("{} is not a directory", root.display()));
    }
    let mut files = Vec::new();
    let mut warnings = Vec::new();
    scan(&root, 0, &mut 0, &mut files, &mut warnings);
    let limited: Vec<_> = warnings
        .iter()
        .filter(|w| w.starts_with("Scan limit:"))
        .cloned()
        .collect();
    if limited.len() > 5 {
        warnings.retain(|w| !w.starts_with("Scan limit:"));
        warnings.push(format!(
            "Scan limit: {} directories not inspected; first five examples: {}",
            limited.len(),
            limited[..5].join("; ")
        ));
    }
    let mut manifests = BTreeMap::new();
    let mut paths = BTreeMap::new();
    let mut projects = Vec::new();
    for file in files {
        match read_manifest(&file) {
            Ok(manifest) => {
                let dir = file.parent().unwrap();
                let id = project_id(dir.strip_prefix(&root).unwrap());
                if manifests.contains_key(&id) {
                    return Err(format!("project ID collision: {id}; scan a narrower root"));
                }
                paths.insert(id.clone(), dir.to_owned());
                projects.push(DiscoveredProject {
                    id: id.clone(),
                    package_name: manifest.name.clone(),
                    version: manifest.version.clone(),
                    manifest: file,
                });
                manifests.insert(id, manifest);
            }
            Err(error) => warnings.push(format!("Skipped {}: {error}", file.display())),
        }
    }
    if projects.is_empty() {
        return Err("No readable package.json projects found in the bounded scan".into());
    }
    let mut workspace = Workspace {
        version: 1,
        projects: manifests
            .keys()
            .map(|id| {
                (
                    id.clone(),
                    Project {
                        depends_on: Vec::new(),
                    },
                )
            })
            .collect(),
    };
    let mut relationships = Vec::new();
    for (consumer, manifest) in &manifests {
        for (section, dependencies) in SECTIONS.iter().zip(manifest.sections()) {
            for (dependency, requirement) in dependencies {
                let local_path = requirement
                    .strip_prefix("file:")
                    .or_else(|| requirement.strip_prefix("link:"));
                let candidates: Vec<_> = if let Some(local_path) = local_path {
                    let target = paths[consumer].join(local_path).canonicalize();
                    manifests
                        .keys()
                        .filter(|id| target.as_ref().is_ok_and(|target| target == &paths[*id]))
                        .cloned()
                        .collect()
                } else if requirement.starts_with("npm:")
                    || requirement.contains('/') && !requirement.starts_with("workspace:")
                {
                    // Aliases, git/URL dependencies and tarballs need their own resolver.
                    warnings.push(format!(
                        "Unresolved dependency specification: {consumer} {section}/{dependency}"
                    ));
                    continue;
                } else {
                    manifests
                        .iter()
                        .filter(|(_, provider)| provider.name.as_ref() == Some(dependency))
                        .map(|(id, _)| id.clone())
                        .collect()
                };
                if candidates.len() != 1 {
                    if local_path.is_some()
                        || candidates.len() > 1
                        || requirement.starts_with("workspace:")
                    {
                        warnings.push(format!("Unresolved local dependency: {consumer} {section}/{dependency}: {} matching projects", candidates.len()));
                    }
                    continue;
                }
                let provider = &candidates[0];
                if provider == consumer {
                    warnings.push(format!(
                        "Ignored self dependency: {consumer} {section}/{dependency}"
                    ));
                    continue;
                }
                let depends_on = &mut workspace.projects.get_mut(consumer).unwrap().depends_on;
                if !depends_on.contains(provider) {
                    depends_on.push(provider.clone());
                }
                relationships.push(Relationship {
                    consumer: consumer.clone(),
                    provider: provider.clone(),
                    kind: if local_path.is_some() {
                        "local_path"
                    } else {
                        "package_name_candidate"
                    },
                    source: paths[consumer].join("package.json"),
                    json_pointer: format!(
                        "/{section}/{}",
                        dependency.replace('~', "~0").replace('/', "~1")
                    ),
                    dependency: dependency.clone(),
                    requirement: requirement.clone(),
                    provider_version: manifests[provider].version.clone(),
                    needs_confirmation: true,
                });
            }
        }
    }
    for project in workspace.projects.values_mut() {
        project.depends_on.sort();
    }
    workspace.validate()?;
    Ok(Discovery {
        status: "needs_review",
        root,
        workspace,
        local: LocalProposal { paths },
        projects,
        relationships,
        warnings,
        limitations: vec![
            "package.json only; scans root and four directory levels, up to 10000 directories and 1 MiB per manifest.",
            "Hidden, generated and vendored folders and symbolic links are skipped; no scripts execute and no network requests are made.",
            "Package-name matches are candidates: version compatibility, lockfiles, registry identity and installed resolution are not checked.",
            "Workspace globs and npm aliases are not resolved; nested manifests are discovered only within scan limits.",
            "API calls, source imports, internal callers, project roles and documentation are not analyzed.",
            "All suggested projects and relationships require developer review before use as approved workspace configuration.",
        ],
    })
}
