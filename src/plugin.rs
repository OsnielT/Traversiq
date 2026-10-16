//! Workspace-local configuration for an installed, relocatable plugin.
use crate::{checkpoint, load_workspace, source};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const SETTINGS: &str = ".traversiq/config.json";

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub workspace: PathBuf,
    pub local: PathBuf,
}

pub fn settings(cwd: &Path) -> Result<Option<Settings>, String> {
    let cwd = cwd.canonicalize().map_err(|e| e.to_string())?;
    if !cwd.is_dir() {
        return Err("plugin working directory must be a directory".into());
    }
    for ancestor in cwd.ancestors() {
        let path = ancestor.join(SETTINGS);
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                if fs::symlink_metadata(path.parent().unwrap())
                    .map_err(|e| e.to_string())?
                    .file_type()
                    .is_symlink()
                {
                    return Err("symlink configuration directories are unsupported".into());
                }
                let mut settings: Settings = serde_json::from_str(&source::read_text(&path)?)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                let base = path.parent().unwrap();
                settings.workspace = base.join(&settings.workspace);
                settings.local = base.join(&settings.local);
                return Ok(Some(settings));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
    }
    Ok(None)
}

pub fn configure(cwd: &Path, workspace: &Path, local: &Path) -> Result<Value, String> {
    let cwd = cwd.canonicalize().map_err(|e| e.to_string())?;
    let settings = Settings {
        workspace: cwd
            .join(workspace)
            .canonicalize()
            .map_err(|e| e.to_string())?,
        local: cwd.join(local).canonicalize().map_err(|e| e.to_string())?,
    };
    let graph = load_workspace(&settings.workspace, &settings.local)?;
    let path = cwd.join(SETTINGS);
    let bytes = serde_json::to_vec_pretty(&settings).map_err(|e| e.to_string())?;
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    if fs::symlink_metadata(path.parent().unwrap())
        .map_err(|e| e.to_string())?
        .file_type()
        .is_symlink()
    {
        return Err("symlink configuration directories are unsupported".into());
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path).map_err(|e| {
        format!(
            "{}: {e}; existing configuration is never overwritten",
            path.display()
        )
    })?;
    if let Err(error) = file.write_all(&bytes) {
        let _ = fs::remove_file(&path);
        return Err(error.to_string());
    }
    Ok(json!({"configured":true, "config":path, "project_count":graph.projects.len()}))
}

pub fn pre_edit() -> Value {
    let result = (|| {
        let input = checkpoint::read_input()?;
        let event: Value = serde_json::from_str(&input).map_err(|e| e.to_string())?;
        let cwd = event
            .get("cwd")
            .and_then(Value::as_str)
            .filter(|s| Path::new(s).is_absolute())
            .ok_or("hook cwd must be absolute")?;
        match settings(Path::new(cwd))? {
            Some(settings) => Ok(checkpoint::with_input(
                &settings.workspace,
                &settings.local,
                input,
            )),
            None => Ok(json!({"hookSpecificOutput": {"hookEventName":"PreToolUse",
                "additionalContext":"Traversiq is not configured for this directory; change impact is unchecked. Use the Traversiq setup skill to confirm project mappings and create .traversiq/config.json. Normal host permissions still apply."}})),
        }
    })();
    result.unwrap_or_else(checkpoint::incomplete)
}
