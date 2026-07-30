// Product names (SQLite, WAL, JSON) are not Rust identifiers; suppress doc_markdown for this module.
#![allow(clippy::doc_markdown)]

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

use rusqlite::{Connection, params};

use crate::{error::SourceError, source::Source, value::ConfigValue};

// ── connection helpers ────────────────────────────────────────────────────────

fn open_wal(path: &Path) -> Result<Connection, rusqlite::Error> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    Ok(conn)
}

fn ensure_schema(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS kv (
            app_name TEXT NOT NULL,
            key      TEXT NOT NULL,
            value    TEXT NOT NULL,
            PRIMARY KEY (app_name, key)
        );",
    )
}

// ── source (read) ─────────────────────────────────────────────────────────────

/// Config source backed by a SQLite database in WAL mode.
///
/// Keys use dot notation for nesting: `"window.width"` expands to
/// `{ "window": { "width": … } }`. Values are JSON-encoded `ConfigValue`s.
///
/// Safe for concurrent access from multiple processes; WAL handles the isolation.
pub struct SqliteSource {
    db_path: PathBuf,
    app_name: String,
}

impl SqliteSource {
    /// Create a source pointing at `db_path` scoped to `app_name`.
    #[must_use]
    pub fn new(db_path: impl Into<PathBuf>, app_name: impl Into<String>) -> Self {
        Self {
            db_path: db_path.into(),
            app_name: app_name.into(),
        }
    }

    /// Return a write handle for the same database and app scope.
    #[must_use]
    pub fn store(&self) -> SqliteStore {
        SqliteStore {
            db_path: self.db_path.clone(),
            app_name: self.app_name.clone(),
        }
    }
}

impl Source for SqliteSource {
    fn collect(&self) -> Result<ConfigValue, SourceError> {
        if !self.db_path.exists() {
            return Ok(ConfigValue::Table(BTreeMap::new()));
        }

        let conn = open_wal(&self.db_path).map_err(|e| be(&e))?;
        ensure_schema(&conn).map_err(|e| be(&e))?;

        let mut stmt = conn
            .prepare("SELECT key, value FROM kv WHERE app_name = ?1 ORDER BY key")
            .map_err(|e| be(&e))?;

        let pairs: Vec<(String, String)> = stmt
            .query_map(params![self.app_name], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|e| be(&e))?
            .collect::<rusqlite::Result<_>>()
            .map_err(|e| be(&e))?;

        let mut table: BTreeMap<String, ConfigValue> = BTreeMap::new();
        for (key, json) in pairs {
            let json_val: serde_json::Value =
                serde_json::from_str(&json).map_err(|e| SourceError::Backend(e.to_string()))?;
            insert_dot_path(&mut table, &key, from_json(json_val));
        }

        Ok(ConfigValue::Table(table))
    }
}

// ── store (write) ─────────────────────────────────────────────────────────────

/// Write handle for inserting or updating values in the SQLite store.
pub struct SqliteStore {
    db_path: PathBuf,
    app_name: String,
}

impl SqliteStore {
    /// Create a write handle pointing at `db_path` scoped to `app_name`.
    #[must_use]
    pub fn new(db_path: impl Into<PathBuf>, app_name: impl Into<String>) -> Self {
        Self {
            db_path: db_path.into(),
            app_name: app_name.into(),
        }
    }

    /// Upsert `key` with JSON-encoded `value`.
    ///
    /// # Errors
    /// Returns [`SourceError::Backend`] on I/O, SQLite, or serialization failure.
    pub fn set(&self, key: &str, value: &ConfigValue) -> Result<(), SourceError> {
        let json = serde_json::to_string(&serde_json::Value::from(value.clone()))
            .map_err(|e| SourceError::Backend(e.to_string()))?;
        if let Some(parent) = self.db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| SourceError::Io {
                path: self.db_path.clone(),
                source: e,
            })?;
        }
        let conn = open_wal(&self.db_path).map_err(|e| be(&e))?;
        ensure_schema(&conn).map_err(|e| be(&e))?;
        conn.execute(
            "INSERT OR REPLACE INTO kv (app_name, key, value) VALUES (?1, ?2, ?3)",
            params![self.app_name, key, json],
        )
        .map_err(|e| be(&e))?;
        Ok(())
    }

    /// Read a single key's value, or `None` if absent.
    ///
    /// # Errors
    /// Returns [`SourceError::Backend`] on I/O, SQLite, or deserialization failure.
    pub fn get(&self, key: &str) -> Result<Option<ConfigValue>, SourceError> {
        if !self.db_path.exists() {
            return Ok(None);
        }
        let conn = open_wal(&self.db_path).map_err(|e| be(&e))?;
        let result: Option<String> = match conn.query_row(
            "SELECT value FROM kv WHERE app_name = ?1 AND key = ?2",
            params![self.app_name, key],
            |row| row.get(0),
        ) {
            Ok(v) => Some(v),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(be(&e)),
        };
        match result {
            None => Ok(None),
            Some(json) => {
                let val: serde_json::Value =
                    serde_json::from_str(&json).map_err(|e| SourceError::Backend(e.to_string()))?;
                Ok(Some(from_json(val)))
            }
        }
    }

    /// Delete `key` from the store. No-op if absent.
    ///
    /// # Errors
    /// Returns [`SourceError::Backend`] on SQLite failure.
    pub fn delete(&self, key: &str) -> Result<(), SourceError> {
        if !self.db_path.exists() {
            return Ok(());
        }
        let conn = open_wal(&self.db_path).map_err(|e| be(&e))?;
        conn.execute(
            "DELETE FROM kv WHERE app_name = ?1 AND key = ?2",
            params![self.app_name, key],
        )
        .map_err(|e| be(&e))?;
        Ok(())
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Convert a rusqlite error to a `SourceError::Backend`.
fn be(e: &rusqlite::Error) -> SourceError {
    SourceError::Backend(e.to_string())
}

/// Expand a dot-notation key into nested tables and insert the value.
fn insert_dot_path(table: &mut BTreeMap<String, ConfigValue>, key: &str, value: ConfigValue) {
    match key.split_once('.') {
        None => {
            table.insert(key.to_owned(), value);
        }
        Some((head, rest)) => {
            let entry = table
                .entry(head.to_owned())
                .or_insert_with(|| ConfigValue::Table(BTreeMap::new()));
            if let ConfigValue::Table(inner) = entry {
                insert_dot_path(inner, rest, value);
            }
        }
    }
}

fn from_json(val: serde_json::Value) -> ConfigValue {
    match val {
        serde_json::Value::Null => ConfigValue::Null,
        serde_json::Value::Bool(b) => ConfigValue::Bool(b),
        serde_json::Value::Number(n) => n
            .as_i64()
            .map(ConfigValue::Integer)
            .or_else(|| n.as_f64().map(ConfigValue::Float))
            .unwrap_or(ConfigValue::Null),
        serde_json::Value::String(s) => ConfigValue::String(s),
        serde_json::Value::Array(arr) => {
            ConfigValue::Array(arr.into_iter().map(from_json).collect())
        }
        serde_json::Value::Object(map) => {
            ConfigValue::Table(map.into_iter().map(|(k, v)| (k, from_json(v))).collect())
        }
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn set_and_collect_flat_keys() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");
        let store = SqliteStore::new(&db, "myapp");

        store.set("port", &ConfigValue::Integer(8080)).unwrap();
        store.set("host", &ConfigValue::String("localhost".into())).unwrap();

        let source = SqliteSource::new(&db, "myapp");
        let config = source.collect().unwrap();
        let table = config.as_table().unwrap();

        assert_eq!(table["port"], ConfigValue::Integer(8080));
        assert_eq!(table["host"], ConfigValue::String("localhost".into()));
    }

    #[test]
    fn dot_notation_expands_to_nested_tables() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");
        let store = SqliteStore::new(&db, "myapp");

        store.set("window.width", &ConfigValue::Integer(1920)).unwrap();
        store.set("window.height", &ConfigValue::Integer(1080)).unwrap();
        store.set("port", &ConfigValue::Integer(3000)).unwrap();

        let source = SqliteSource::new(&db, "myapp");
        let table = source.collect().unwrap();
        let table = table.as_table().unwrap();

        let window = table["window"].as_table().unwrap();
        assert_eq!(window["width"], ConfigValue::Integer(1920));
        assert_eq!(window["height"], ConfigValue::Integer(1080));
        assert_eq!(table["port"], ConfigValue::Integer(3000));
    }

    #[test]
    fn get_returns_none_for_missing_key() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");
        let store = SqliteStore::new(&db, "myapp");
        store.set("a", &ConfigValue::Integer(1)).unwrap();

        assert!(store.get("missing").unwrap().is_none());
        assert_eq!(store.get("a").unwrap(), Some(ConfigValue::Integer(1)));
    }

    #[test]
    fn delete_removes_key() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");
        let store = SqliteStore::new(&db, "myapp");
        store.set("key", &ConfigValue::Integer(42)).unwrap();
        store.delete("key").unwrap();

        let source = SqliteSource::new(&db, "myapp");
        let table = source.collect().unwrap();
        assert!(table.as_table().unwrap().is_empty());
    }

    #[test]
    fn app_name_isolation() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");

        SqliteStore::new(&db, "app_a")
            .set("val", &ConfigValue::Integer(1))
            .unwrap();
        SqliteStore::new(&db, "app_b")
            .set("val", &ConfigValue::Integer(2))
            .unwrap();

        let a = SqliteSource::new(&db, "app_a").collect().unwrap();
        let b = SqliteSource::new(&db, "app_b").collect().unwrap();

        assert_eq!(a.as_table().unwrap()["val"], ConfigValue::Integer(1));
        assert_eq!(b.as_table().unwrap()["val"], ConfigValue::Integer(2));
    }

    /// Concurrent writes from 8 threads to the same database must not corrupt data.
    ///
    /// This validates WAL-mode isolation: SQLite serializes writers while still
    /// allowing readers to proceed without blocking.
    #[test]
    fn concurrent_writes_no_corruption() {
        const THREADS: i64 = 8;
        const OPS_PER_THREAD: i64 = 50;

        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(dir.path().join("concurrent.db"));

        // Seed the schema before spawning threads.
        SqliteStore::new(db.as_ref(), "myapp")
            .set("seed", &ConfigValue::Bool(true))
            .unwrap();

        let handles: Vec<_> = (0..THREADS)
            .map(|t| {
                let db = Arc::clone(&db);
                std::thread::spawn(move || {
                    let store = SqliteStore::new(db.as_ref(), "myapp");
                    for j in 0..OPS_PER_THREAD {
                        store
                            .set(&format!("t{t}_k{j}"), &ConfigValue::Integer(t * 1000 + j))
                            .unwrap();
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().expect("writer thread panicked");
        }

        // All keys must be present and hold correct values.
        let source = SqliteSource::new(db.as_ref(), "myapp");
        let table = source.collect().unwrap();
        let table = table.as_table().unwrap();

        // 1 seed + THREADS * OPS_PER_THREAD entries
        let expected = usize::try_from(1 + THREADS * OPS_PER_THREAD).unwrap();
        assert_eq!(table.len(), expected);

        for t in 0..THREADS {
            for j in 0..OPS_PER_THREAD {
                let key = format!("t{t}_k{j}");
                assert_eq!(
                    table[&key],
                    ConfigValue::Integer(t * 1000 + j),
                    "wrong value for {key}"
                );
            }
        }
    }
}
