use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Default, Serialize)]
pub struct CacheStats {
    pub reused_files: usize,
    pub reparsed_files: usize,
    pub removed_files: usize,
}

pub(crate) struct Cache {
    db: Connection,
    scope: String,
    profile: String,
    stats: CacheStats,
}

#[derive(Serialize, Deserialize)]
struct Facts<T> {
    value: T,
    warnings: Vec<String>,
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

impl Cache {
    pub(crate) fn open(
        local: &Path,
        kind: &str,
        roots: &BTreeMap<String, PathBuf>,
    ) -> Result<Self, String> {
        let local = local.canonicalize().map_err(|e| e.to_string())?;
        let path = local.with_file_name(format!(
            "{}.traversiq-cache.sqlite3",
            local.file_name().unwrap().to_string_lossy()
        ));
        if let Ok(meta) = std::fs::symlink_metadata(&path)
            && !meta.file_type().is_file()
        {
            return Err("Cache must be a regular file, not a directory or symlink".into());
        }
        let db = Connection::open(&path).map_err(|e| format!("Cache {}: {e}", path.display()))?;
        db.busy_timeout(Duration::from_millis(100))
            .map_err(|e| e.to_string())?;
        db.execute_batch(
            "BEGIN IMMEDIATE;
            CREATE TABLE IF NOT EXISTS facts (
                scope TEXT NOT NULL, path TEXT NOT NULL, profile TEXT NOT NULL,
                hash TEXT NOT NULL, payload TEXT NOT NULL, PRIMARY KEY(scope, path)
            );
            CREATE TEMP TABLE seen (path TEXT PRIMARY KEY);",
        )
        .map_err(|e| format!("Cache unavailable: {e}"))?;
        let scope = format!(
            "{kind}:{}",
            serde_json::to_string(roots).map_err(|e| e.to_string())?
        );
        // Invalidate facts whenever parser logic or locked dependency versions change.
        let profile = hash(
            concat!(
                include_str!("source.rs"),
                include_str!("api.rs"),
                include_str!("cache.rs"),
                include_str!("../Cargo.lock")
            )
            .as_bytes(),
        );
        Ok(Self {
            db,
            scope,
            profile,
            stats: CacheStats::default(),
        })
    }

    pub(crate) fn parse<T: Serialize + DeserializeOwned>(
        &mut self,
        path: &Path,
        project: &str,
        text: &str,
        warnings: &mut Vec<String>,
        parse: impl FnOnce(&mut Vec<String>) -> T,
    ) -> Result<T, String> {
        let path = serde_json::to_string(&(project, path)).map_err(|e| e.to_string())?;
        let hash = hash(text.as_bytes());
        self.db
            .execute("INSERT OR IGNORE INTO seen VALUES (?1)", [&path])
            .map_err(|e| e.to_string())?;
        let payload: Option<String> = self
            .db
            .query_row(
                "SELECT payload FROM facts WHERE scope=?1 AND path=?2 AND profile=?3 AND hash=?4",
                params![self.scope, path, self.profile, hash],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Cache read: {e}"))?;
        if let Some(facts) = payload.and_then(|s| serde_json::from_str::<Facts<T>>(&s).ok()) {
            self.stats.reused_files += 1;
            warnings.extend(facts.warnings);
            return Ok(facts.value);
        }
        let mut diagnostics = Vec::new();
        let value = parse(&mut diagnostics);
        let payload = serde_json::to_string(&Facts {
            value: &value,
            warnings: diagnostics.clone(),
        })
        .map_err(|e| e.to_string())?;
        self.db
            .execute(
                "INSERT OR REPLACE INTO facts VALUES (?1,?2,?3,?4,?5)",
                params![self.scope, path, self.profile, hash, payload],
            )
            .map_err(|e| format!("Cache write: {e}"))?;
        self.stats.reparsed_files += 1;
        warnings.extend(diagnostics);
        Ok(value)
    }

    pub(crate) fn finish(mut self) -> Result<CacheStats, String> {
        self.stats.removed_files = self
            .db
            .execute(
                "DELETE FROM facts WHERE scope=?1 AND path NOT IN (SELECT path FROM seen)",
                [&self.scope],
            )
            .map_err(|e| e.to_string())?;
        self.db
            .execute_batch("COMMIT")
            .map_err(|e| format!("Cache commit: {e}"))?;
        Ok(self.stats)
    }
}
