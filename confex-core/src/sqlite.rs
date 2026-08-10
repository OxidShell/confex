// Product names (SQLite, WAL) are not Rust identifiers; suppress doc_markdown for this module.
#![allow(clippy::doc_markdown)]

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

use rusqlite::{Connection, params};

use crate::{codec, error::SourceError, source::Source, value::ConfigValue};

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

/// Config source backed by a `SQLite` database in WAL mode.
///
/// Keys use dot notation for nesting: `"window.width"` expands to
/// `{ "window": { "width": … } }`. Values are JSON-encoded `ConfigValue`s.
///
/// Safe for concurrent access from multiple processes; WAL handles the isolation.
#[derive(Clone)]
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

/// Hot-reload support for `SQLite`-backed config.
///
/// Watches the directory containing the database file so that WAL writes from
/// other processes are detected without waiting for a checkpoint.
#[cfg(feature = "hot-reload")]
impl SqliteSource {
    /// Watch this database for changes and reload automatically.
    ///
    /// Loads the initial config from the database, then watches the containing
    /// directory so WAL writes from other processes trigger a reload without
    /// waiting for a checkpoint.
    ///
    /// Writes should go through [`SqliteStore`]; [`crate::handle::WriteGuard`]
    /// has no write-back path in this mode.
    ///
    /// # Errors
    ///
    /// Returns [`crate::handle::HotReloadError::Load`] if the initial load fails,
    /// or [`crate::handle::HotReloadError::Watch`] if the watcher cannot start.
    pub fn watch<T>(
        &self,
        options: crate::handle::WatchOptions,
    ) -> Result<
        (crate::handle::ConfigHandle<T>, crate::handle::WatcherHandle),
        crate::handle::HotReloadError,
    >
    where
        T: crate::config::ConfigSchema
            + Clone
            + serde::de::DeserializeOwned
            + Send
            + Sync
            + 'static,
    {
        use std::sync::Arc;

        let load = |src: &Self| -> Result<T, crate::error::LoadError> {
            let cv = src.collect()?;
            serde::Deserialize::deserialize(crate::deserialize::ConfigValueDeserializer(cv))
                .map_err(Into::into)
        };

        let initial = load(self)?;

        let source = self.clone();
        let reload: Arc<dyn Fn() -> Result<T, crate::error::LoadError> + Send + Sync> =
            Arc::new(move || load(&source));

        let watch_path = self.db_path.parent().unwrap_or(&self.db_path);
        crate::handle::ConfigHandle::spawn_watcher(
            initial,
            watch_path,
            &reload,
            None,
            options.debounce,
        )
    }
}

#[cfg(feature = "hot-reload")]
impl crate::source::WatchableSource for SqliteSource {
    fn watch_paths(&self) -> Vec<std::path::PathBuf> {
        vec![self
            .db_path
            .parent()
            .unwrap_or(&self.db_path)
            .to_path_buf()]
    }
}

impl Source for SqliteSource {
    fn collect(&self) -> Result<ConfigValue, SourceError> {
        if !self.db_path.exists() {
            return Ok(ConfigValue::Table(BTreeMap::new()));
        }

        let conn = open_wal(&self.db_path)?;
        ensure_schema(&conn)?;

        let mut stmt = conn
            .prepare("SELECT key, value FROM kv WHERE app_name = ?1 ORDER BY key")?;

        let pairs: Vec<(String, String)> = stmt
            .query_map(params![self.app_name], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;

        let mut table: BTreeMap<String, ConfigValue> = BTreeMap::new();
        for (key, encoded) in pairs {
            insert_dot_path(&mut table, &key, codec::decode(&encoded)?);
        }

        Ok(ConfigValue::Table(table))
    }
}

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
    /// Returns [`SourceError::Sqlite`] on SQLite failure, [`SourceError::Backend`] on
    /// Returns [`SourceError::Sqlite`] on SQLite failure.
    pub fn set(&self, key: &str, value: &ConfigValue) -> Result<(), SourceError> {
        let encoded = codec::encode(value);
        if let Some(parent) = self.db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| SourceError::Io {
                path: self.db_path.clone(),
                source: e,
            })?;
        }
        let conn = open_wal(&self.db_path)?;
        ensure_schema(&conn)?;
        conn.execute(
            "INSERT OR REPLACE INTO kv (app_name, key, value) VALUES (?1, ?2, ?3)",
            params![self.app_name, key, encoded],
        )?;
        Ok(())
    }

    /// Read a single key's value, or `None` if absent.
    ///
    /// # Errors
    /// Returns [`SourceError::Sqlite`] on SQLite failure.
    pub fn get(&self, key: &str) -> Result<Option<ConfigValue>, SourceError> {
        if !self.db_path.exists() {
            return Ok(None);
        }
        let conn = open_wal(&self.db_path)?;
        let result: Option<String> = match conn.query_row(
            "SELECT value FROM kv WHERE app_name = ?1 AND key = ?2",
            params![self.app_name, key],
            |row| row.get(0),
        ) {
            Ok(v) => Some(v),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(e.into()),
        };
        match result {
            None => Ok(None),
            Some(encoded) => codec::decode(&encoded).map(Some),
        }
    }

    /// Delete `key` from the store. No-op if absent.
    ///
    /// # Errors
    /// Returns [`SourceError::Sqlite`] on SQLite failure.
    pub fn delete(&self, key: &str) -> Result<(), SourceError> {
        if !self.db_path.exists() {
            return Ok(());
        }
        let conn = open_wal(&self.db_path)?;
        conn.execute(
            "DELETE FROM kv WHERE app_name = ?1 AND key = ?2",
            params![self.app_name, key],
        )?;
        Ok(())
    }
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
        store
            .set("host", &ConfigValue::String("localhost".into()))
            .unwrap();

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

        store
            .set("window.width", &ConfigValue::Integer(1920))
            .unwrap();
        store
            .set("window.height", &ConfigValue::Integer(1080))
            .unwrap();
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

    #[cfg(feature = "hot-reload")]
    #[test]
    fn sqlite_watch_detects_cross_process_write() {
        use crate::{config::ConfigSchema, handle::WatchOptions, merge::FieldStrategies};
        use serde::{Deserialize, Serialize};
        use std::{thread, time::Duration};

        #[derive(Debug, Clone, Serialize, Deserialize)]
        struct Cfg {
            port: i64,
        }
        impl ConfigSchema for Cfg {
            const APP_NAME: &'static str = "watch_test";
            const SCHEMA_VERSION: u32 = 1;
            fn field_strategies() -> FieldStrategies { &[] }
            fn encrypted_fields() -> &'static [&'static str] { &[] }
        }

        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("watch.db");

        let store = SqliteStore::new(&db, "watch_test");
        store.set("port", &ConfigValue::Integer(3000)).unwrap();

        let source = SqliteSource::new(&db, "watch_test");
        let (handle, _watcher) = source.watch::<Cfg>(WatchOptions::default()).unwrap();

        assert_eq!(handle.read().port, 3000);

        // Simulate another process writing to the same DB.
        store.set("port", &ConfigValue::Integer(9090)).unwrap();
        thread::sleep(Duration::from_millis(600));

        assert_eq!(handle.read().port, 9090);
    }
}
