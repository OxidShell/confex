#![allow(clippy::doc_markdown)]

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use redb::{
    Builder, ConcurrencyMode, Database, ReadableDatabase as _, ReadableTable as _, ReadableTableMetadata as _,
    TableDefinition,
};

use crate::{codec, error::SourceError, source::Source, value::ConfigValue};

const KV: TableDefinition<(&str, &str), &str> = TableDefinition::new("confex_kv");

fn open_db(path: &Path) -> Result<Database, redb::DatabaseError> {
    let mut b = Builder::new();
    b.set_concurrency_mode(ConcurrencyMode::MultiWriter);
    b.create(path)
}

/// Config source backed by a redb database in multi-writer mode.
///
/// Keys use dot notation for nesting: `"window.width"` expands to
/// `{ "window": { "width": … } }`. Values are codec-encoded `ConfigValue`s.
///
/// Safe for concurrent access from multiple processes; redb's multi-writer
/// concurrency mode handles isolation via file byte-range locks. Requires the
/// `experimental-multiprocess` feature on the `redb` crate (enabled by default).
///
/// The `app_name` scope is injected automatically when the source is added to a
/// [`crate::config::ConfigLoader`] via `with_source` or `with_watched_source`.
/// When using `watch` directly (without `ConfigLoader`), the app name is inferred
/// from the schema type's `APP_NAME` constant.
#[derive(Clone)]
pub struct RedbSource {
    db_path: PathBuf,
    app_name: Option<String>,
}

impl RedbSource {
    /// Create a source pointing at `db_path`.
    ///
    /// The `app_name` scope is filled in automatically by `ConfigLoader` or inferred
    /// from the schema type when calling `watch`. Pass the source to
    /// [`crate::config::ConfigLoader::with_source`] rather than setting the name
    /// manually in most cases.
    #[must_use]
    pub fn new(db_path: impl Into<PathBuf>) -> Self {
        Self {
            db_path: db_path.into(),
            app_name: None,
        }
    }

    /// Return a write handle for the same database and app scope.
    ///
    /// Valid only after the source has been given an app name (via `ConfigLoader`
    /// or an explicit `Source::with_app_name` call).
    #[must_use]
    pub fn store(&self) -> RedbStore {
        RedbStore {
            db_path: self.db_path.clone(),
            app_name: self.app_name.clone().unwrap_or_default(),
        }
    }
}

/// Hot-reload support for redb-backed config.
///
/// Watches the directory containing the database file so that writes from
/// other processes are detected without polling.
#[cfg(feature = "hot-reload")]
impl RedbSource {
    /// Watch this database for changes and reload automatically.
    ///
    /// Loads the initial config from the database, then watches the containing
    /// directory so cross-process writes trigger a reload.
    ///
    /// Writes should go through [`RedbStore`]; [`crate::handle::WriteGuard`]
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

        let resolved = Self {
            app_name: Some(
                self.app_name
                    .clone()
                    .unwrap_or_else(|| T::APP_NAME.to_owned()),
            ),
            ..self.clone()
        };

        let load = |src: &Self| -> Result<T, crate::error::LoadError> {
            let cv = src.collect()?;
            serde::Deserialize::deserialize(crate::deserialize::ConfigValueDeserializer(cv))
                .map_err(Into::into)
        };

        let initial = load(&resolved)?;

        let reload: Arc<dyn Fn() -> Result<T, crate::error::LoadError> + Send + Sync> =
            Arc::new(move || load(&resolved));

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
impl crate::source::WatchableSource for RedbSource {
    fn watch_paths(&self) -> Vec<std::path::PathBuf> {
        vec![self.db_path.parent().unwrap_or(&self.db_path).to_path_buf()]
    }
}

impl Source for RedbSource {
    fn with_app_name(mut self, name: &str) -> Self {
        if self.app_name.is_none() {
            self.app_name = Some(name.to_owned());
        }
        self
    }

    fn collect(&self) -> Result<ConfigValue, SourceError> {
        let app_name = match self.app_name.as_deref() {
            Some(n) if !n.is_empty() => n,
            _ => return Ok(ConfigValue::Table(BTreeMap::new())),
        };

        if !self.db_path.exists() {
            return Ok(ConfigValue::Table(BTreeMap::new()));
        }

        let db = open_db(&self.db_path)?;
        let txn = db.begin_read()?;
        let table = match txn.open_table(KV) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => {
                return Ok(ConfigValue::Table(BTreeMap::new()));
            }
            Err(e) => return Err(e.into()),
        };

        let mut out: BTreeMap<String, ConfigValue> = BTreeMap::new();
        if table.is_empty()? {
            return Ok(ConfigValue::Table(out));
        }
        let start: (&str, &str) = (app_name, "");
        let iter = table.range(start..)?;
        for entry in iter {
            let (k_guard, v_guard) = entry?;
            let (a, key) = k_guard.value();
            if a != app_name {
                break;
            }
            insert_dot_path(&mut out, key, codec::decode(v_guard.value())?);
        }
        Ok(ConfigValue::Table(out))
    }
}

/// Write handle for inserting or updating values in the redb store.
pub struct RedbStore {
    db_path: PathBuf,
    app_name: String,
}

impl RedbStore {
    /// Create a write handle pointing at `db_path` scoped to `app_name`.
    #[must_use]
    pub fn new(db_path: impl Into<PathBuf>, app_name: impl Into<String>) -> Self {
        Self {
            db_path: db_path.into(),
            app_name: app_name.into(),
        }
    }

    /// Upsert `key` with codec-encoded `value`.
    ///
    /// # Errors
    /// Returns [`SourceError::Redb`] on redb failure, [`SourceError::Io`] if the
    /// parent directory cannot be created.
    pub fn set(&self, key: &str, value: &ConfigValue) -> Result<(), SourceError> {
        let encoded = codec::encode(value);
        if let Some(parent) = self.db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| SourceError::Io {
                path: self.db_path.clone(),
                source: e,
            })?;
        }
        let db = open_db(&self.db_path)?;
        let txn = db.begin_write()?;
        {
            let mut table = txn.open_table(KV)?;
            table.insert((self.app_name.as_str(), key), encoded.as_str())?;
        }
        txn.commit()?;
        Ok(())
    }

    /// Read a single key's value, or `None` if absent.
    ///
    /// # Errors
    /// Returns [`SourceError::Redb`] on redb failure.
    pub fn get(&self, key: &str) -> Result<Option<ConfigValue>, SourceError> {
        if !self.db_path.exists() {
            return Ok(None);
        }
        let db = open_db(&self.db_path)?;
        let txn = db.begin_read()?;
        let table = match txn.open_table(KV) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        match table.get((self.app_name.as_str(), key))? {
            None => Ok(None),
            Some(v) => Ok(Some(codec::decode(v.value())?)),
        }
    }

    /// Delete `key` from the store. No-op if absent.
    ///
    /// # Errors
    /// Returns [`SourceError::Redb`] on redb failure.
    pub fn delete(&self, key: &str) -> Result<(), SourceError> {
        if !self.db_path.exists() {
            return Ok(());
        }
        let db = open_db(&self.db_path)?;
        let txn = db.begin_write()?;
        {
            let mut table = match txn.open_table(KV) {
                Ok(t) => t,
                Err(redb::TableError::TableDoesNotExist(_)) => return Ok(()),
                Err(e) => return Err(e.into()),
            };
            table.remove((self.app_name.as_str(), key))?;
        }
        txn.commit()?;
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
        let db = dir.path().join("test.redb");
        let store = RedbStore::new(&db, "myapp");

        store.set("port", &ConfigValue::Integer(8080)).unwrap();
        store
            .set("host", &ConfigValue::String("localhost".into()))
            .unwrap();

        let source = RedbSource::new(&db).with_app_name("myapp");
        let config = source.collect().unwrap();
        let table = config.as_table().unwrap();

        assert_eq!(table["port"], ConfigValue::Integer(8080));
        assert_eq!(table["host"], ConfigValue::String("localhost".into()));
    }

    #[test]
    fn dot_notation_expands_to_nested_tables() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.redb");
        let store = RedbStore::new(&db, "myapp");

        store
            .set("window.width", &ConfigValue::Integer(1920))
            .unwrap();
        store
            .set("window.height", &ConfigValue::Integer(1080))
            .unwrap();
        store.set("port", &ConfigValue::Integer(3000)).unwrap();

        let source = RedbSource::new(&db).with_app_name("myapp");
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
        let db = dir.path().join("test.redb");
        let store = RedbStore::new(&db, "myapp");
        store.set("a", &ConfigValue::Integer(1)).unwrap();

        assert!(store.get("missing").unwrap().is_none());
        assert_eq!(store.get("a").unwrap(), Some(ConfigValue::Integer(1)));
    }

    #[test]
    fn delete_removes_key() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.redb");
        let store = RedbStore::new(&db, "myapp");
        store.set("key", &ConfigValue::Integer(42)).unwrap();
        store.delete("key").unwrap();

        let source = RedbSource::new(&db).with_app_name("myapp");
        let table = source.collect().unwrap();
        assert!(table.as_table().unwrap().is_empty());
    }

    #[test]
    fn app_name_isolation() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.redb");

        RedbStore::new(&db, "app_a")
            .set("val", &ConfigValue::Integer(1))
            .unwrap();
        RedbStore::new(&db, "app_b")
            .set("val", &ConfigValue::Integer(2))
            .unwrap();

        let a = RedbSource::new(&db)
            .with_app_name("app_a")
            .collect()
            .unwrap();
        let b = RedbSource::new(&db)
            .with_app_name("app_b")
            .collect()
            .unwrap();

        assert_eq!(a.as_table().unwrap()["val"], ConfigValue::Integer(1));
        assert_eq!(b.as_table().unwrap()["val"], ConfigValue::Integer(2));
    }

    /// Concurrent writes from 8 threads to the same database must not corrupt data.
    ///
    /// Validates redb's multi-writer concurrency mode: writers serialize at the
    /// transaction level while readers in other processes can follow commits.
    #[test]
    fn concurrent_writes_no_corruption() {
        const THREADS: i64 = 8;
        const OPS_PER_THREAD: i64 = 50;

        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(dir.path().join("concurrent.redb"));

        RedbStore::new(db.as_ref(), "myapp")
            .set("seed", &ConfigValue::Bool(true))
            .unwrap();

        let handles: Vec<_> = (0..THREADS)
            .map(|t| {
                let db = Arc::clone(&db);
                std::thread::spawn(move || {
                    let store = RedbStore::new(db.as_ref(), "myapp");
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

        let source = RedbSource::new(db.as_ref()).with_app_name("myapp");
        let table = source.collect().unwrap();
        let table = table.as_table().unwrap();

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
    fn redb_watch_detects_cross_process_write() {
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
            fn field_strategies() -> FieldStrategies {
                &[]
            }
            fn encrypted_fields() -> &'static [&'static str] {
                &[]
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("watch.redb");

        let store = RedbStore::new(&db, "watch_test");
        store.set("port", &ConfigValue::Integer(3000)).unwrap();

        let source = RedbSource::new(&db);
        let (handle, _watcher) = source.watch::<Cfg>(WatchOptions::default()).unwrap();

        assert_eq!(handle.read().port, 3000);

        store.set("port", &ConfigValue::Integer(9090)).unwrap();
        thread::sleep(Duration::from_millis(600));

        assert_eq!(handle.read().port, 9090);
    }
}
