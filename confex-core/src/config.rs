use std::collections::BTreeMap;

use serde::{Serialize, de::DeserializeOwned};

use crate::{
    crypto::{Encryptor, decrypt_fields},
    deserialize::ConfigValueDeserializer,
    error::LoadError,
    merge::{FieldStrategies, merge},
    migrate::MigrationChain,
    source::Source,
    value::ConfigValue,
};

#[cfg(feature = "async")]
use crate::source::AsyncSource;

/// Marker trait for types that can be loaded via [`ConfigLoader`].
///
/// Implement manually or via `#[derive(ConfigSchema)]` (Phase 1).
pub trait ConfigSchema: DeserializeOwned + Serialize {
    const APP_NAME: &'static str;
    const SCHEMA_VERSION: u32;

    /// Per-field merge strategies. Default: all fields use `Replace`.
    #[must_use]
    fn field_strategies() -> FieldStrategies {
        &[]
    }

    /// Field names whose string values are encrypted at rest.
    #[must_use]
    fn encrypted_fields() -> &'static [&'static str] {
        &[]
    }
}

/// One source in the loader's pipeline.
///
/// `watch_paths` is populated only for entries added via
/// [`ConfigLoader::with_watched_source`]; passive sources (env, CLI) leave it
/// empty.
pub(crate) struct LoaderEntry {
    pub(crate) source: std::sync::Arc<dyn Source + Send + Sync>,
    #[cfg(feature = "hot-reload")]
    pub(crate) watch_paths: Vec<std::path::PathBuf>,
}

/// Builds and executes a layered configuration load.
///
/// Sources are merged in registration order (first = lowest priority).
/// Because sources are stored as `Arc`, the loader is cheaply cloneable and
/// its sources can be captured in hot-reload closures without duplicating state.
pub struct ConfigLoader<S: ConfigSchema> {
    entries: Vec<LoaderEntry>,
    migrations: Option<std::sync::Arc<MigrationChain>>,
    encryptor: Option<std::sync::Arc<dyn Encryptor>>,
    _phantom: std::marker::PhantomData<S>,
}

impl<S: ConfigSchema> Clone for ConfigLoader<S> {
    fn clone(&self) -> Self {
        Self {
            entries: self
                .entries
                .iter()
                .map(|e| LoaderEntry {
                    source: std::sync::Arc::clone(&e.source),
                    #[cfg(feature = "hot-reload")]
                    watch_paths: e.watch_paths.clone(),
                })
                .collect(),
            migrations: self.migrations.clone(),
            encryptor: self.encryptor.clone(),
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<S: ConfigSchema> Default for ConfigLoader<S> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            migrations: None,
            encryptor: None,
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<S: ConfigSchema> ConfigLoader<S> {
    /// Add a passive source (env vars, CLI args, etc.).
    ///
    /// Passive sources are collected once at load time. In selective hot-reload
    /// mode ([`watch_auto`](ConfigLoader::watch_auto)) their initial cache is
    /// reused for every re-merge; they never trigger a reload themselves. Use
    /// [`with_watched_source`](ConfigLoader::with_watched_source) for
    /// file-backed or redb sources that should trigger selective reloads.
    #[must_use]
    pub fn with_source(mut self, source: impl Source + Send + Sync + 'static) -> Self {
        let source = source.with_app_name(S::APP_NAME);
        self.entries.push(LoaderEntry {
            source: std::sync::Arc::new(source),
            #[cfg(feature = "hot-reload")]
            watch_paths: Vec::new(),
        });
        self
    }

    /// Add a watchable source whose filesystem changes trigger selective reloads.
    ///
    /// Only meaningful when combined with
    /// [`watch_auto`](ConfigLoader::watch_auto). Using this with
    /// [`watch`](ConfigLoader::watch) or [`load`](ConfigLoader::load) works but
    /// provides no benefit over [`with_source`](ConfigLoader::with_source).
    #[cfg(feature = "hot-reload")]
    #[must_use]
    pub fn with_watched_source(
        mut self,
        source: impl crate::source::WatchableSource + 'static,
    ) -> Self {
        let source = source.with_app_name(S::APP_NAME);
        let watch_paths = source.watch_paths();
        self.entries.push(LoaderEntry {
            source: std::sync::Arc::new(source),
            watch_paths,
        });
        self
    }

    #[must_use]
    pub fn with_migrations(mut self, migrations: MigrationChain) -> Self {
        self.migrations = Some(std::sync::Arc::new(migrations));
        self
    }

    #[must_use]
    pub fn with_encryptor(mut self, encryptor: impl Encryptor + 'static) -> Self {
        self.encryptor = Some(std::sync::Arc::new(encryptor));
        self
    }

    /// Merge all sources, run migrations if needed, decrypt fields, and deserialize.
    ///
    /// # Errors
    /// Returns [`LoadError`] on source, migration, crypto, or deserialization failure.
    pub fn load(&self) -> Result<S, LoadError> {
        let strategies = S::field_strategies();

        let mut merged = ConfigValue::Table(BTreeMap::new());
        for entry in &self.entries {
            let collected = entry.source.collect()?;
            merged = merge(merged, collected, strategies);
        }

        let current_version = match &merged {
            ConfigValue::Table(t) => match t.get("__schema_version") {
                Some(ConfigValue::Integer(v)) => u32::try_from(*v).unwrap_or(0),
                _ => 0,
            },
            _ => 0,
        };

        if current_version < S::SCHEMA_VERSION
            && let Some(chain) = &self.migrations
        {
            merged = chain.run(merged, current_version, S::SCHEMA_VERSION)?;
        }

        if let Some(enc) = &self.encryptor {
            decrypt_fields(&mut merged, S::encrypted_fields(), enc.as_ref())?;
        }

        Ok(S::deserialize(ConfigValueDeserializer(merged))?)
    }
}

#[cfg(feature = "hot-reload")]
impl<S: ConfigSchema + Clone + Send + Sync + 'static> ConfigLoader<S> {
    /// Full-pipeline hot-reload: every change to `watch_path` re-collects all
    /// sources, merges, migrates, decrypts, and deserializes.
    ///
    /// Prefer [`watch_auto`](ConfigLoader::watch_auto) when using
    /// [`with_watched_source`](ConfigLoader::with_watched_source), as it
    /// avoids re-collecting passive sources on every event.
    ///
    /// # Errors
    ///
    /// Returns [`crate::handle::HotReloadError`] if the initial load fails or
    /// the file watcher cannot be initialised.
    pub fn watch(
        self,
        watch_path: &std::path::Path,
        options: crate::handle::WatchOptions,
    ) -> Result<
        (crate::handle::ConfigHandle<S>, crate::handle::WatcherHandle),
        crate::handle::HotReloadError,
    > {
        use std::sync::Arc;

        let initial = self.load()?;
        let loader = Arc::new(self);
        let reload: Arc<dyn Fn() -> Result<S, LoadError> + Send + Sync> =
            Arc::new(move || loader.load());

        crate::handle::ConfigHandle::spawn_watcher(
            initial,
            watch_path,
            &reload,
            None,
            options.debounce,
        )
    }

    /// Selective hot-reload: only re-collects the source whose watch path changed.
    ///
    /// Sources added via [`with_watched_source`](ConfigLoader::with_watched_source)
    /// register their filesystem paths. When a change is detected on one of those
    /// paths, only that source is re-collected; all other sources reuse their cached
    /// values from the last collection. The results are merged in registration order.
    ///
    /// Sources added via [`with_source`](ConfigLoader::with_source) (env vars, CLI
    /// args) are passive — they keep their initial cached value forever and never
    /// trigger a reload.
    ///
    /// Self-write suppression: if the merged result after re-collection equals the
    /// previous merged result, the in-memory swap is skipped. This avoids
    /// redundant deserialization when the current process itself triggered the
    /// filesystem event.
    ///
    /// # Errors
    ///
    /// Returns [`crate::handle::HotReloadError`] if the initial load fails or
    /// the file watcher cannot be initialised.
    pub fn watch_auto(
        self,
        options: crate::handle::WatchOptions,
    ) -> Result<
        (crate::handle::ConfigHandle<S>, crate::handle::WatcherHandle),
        crate::handle::HotReloadError,
    > {
        use std::sync::Arc;

        use crate::handle::SelectiveEntry;

        let strategies = S::field_strategies();
        let migrations = self.migrations.clone();
        let encryptor = self.encryptor.clone();

        let mut selective_entries = Vec::with_capacity(self.entries.len());
        let mut initial_merged = ConfigValue::Table(BTreeMap::new());

        for entry in &self.entries {
            let cache = entry.source.collect().map_err(LoadError::from)?;
            initial_merged = merge(initial_merged, cache.clone(), strategies);
            selective_entries.push(SelectiveEntry {
                source: Arc::clone(&entry.source),
                cache: std::sync::Mutex::new(cache),
                watch_paths: entry.watch_paths.clone(),
            });
        }

        let transform: Arc<dyn Fn(ConfigValue) -> Result<S, LoadError> + Send + Sync> =
            Arc::new(move |mut merged: ConfigValue| {
                let current_version = match &merged {
                    ConfigValue::Table(t) => match t.get("__schema_version") {
                        Some(ConfigValue::Integer(v)) => u32::try_from(*v).unwrap_or(0),
                        _ => 0,
                    },
                    _ => 0,
                };
                if current_version < S::SCHEMA_VERSION
                    && let Some(chain) = &migrations
                {
                    merged = chain.run(merged, current_version, S::SCHEMA_VERSION)?;
                }
                if let Some(enc) = &encryptor {
                    decrypt_fields(&mut merged, S::encrypted_fields(), enc.as_ref())?;
                }
                Ok(S::deserialize(ConfigValueDeserializer(merged))?)
            });

        let initial = transform(initial_merged.clone())?;

        crate::handle::ConfigHandle::spawn_selective_watcher(
            initial,
            initial_merged,
            selective_entries,
            strategies,
            transform,
            options.debounce,
        )
    }
}

/// Async variant of [`ConfigLoader`]. Sources are driven with `.await`; the
/// executor (tokio, async-std, …) is provided by the caller.
///
/// Sync sources automatically implement [`AsyncSource`] via the blanket impl,
/// so they can be mixed with async sources in the same loader.
#[cfg(feature = "async")]
pub struct AsyncConfigLoader<S: ConfigSchema> {
    sources: Vec<std::sync::Arc<dyn AsyncSource + Send + Sync>>,
    migrations: Option<std::sync::Arc<MigrationChain>>,
    encryptor: Option<std::sync::Arc<dyn Encryptor>>,
    _phantom: std::marker::PhantomData<S>,
}

#[cfg(feature = "async")]
impl<S: ConfigSchema> Default for AsyncConfigLoader<S> {
    fn default() -> Self {
        Self {
            sources: Vec::new(),
            migrations: None,
            encryptor: None,
            _phantom: std::marker::PhantomData,
        }
    }
}

#[cfg(feature = "async")]
impl<S: ConfigSchema> AsyncConfigLoader<S> {
    #[must_use]
    pub fn with_source(mut self, source: impl AsyncSource + 'static) -> Self {
        self.sources.push(std::sync::Arc::new(source));
        self
    }

    #[must_use]
    pub fn with_migrations(mut self, migrations: MigrationChain) -> Self {
        self.migrations = Some(std::sync::Arc::new(migrations));
        self
    }

    #[must_use]
    pub fn with_encryptor(mut self, encryptor: impl Encryptor + 'static) -> Self {
        self.encryptor = Some(std::sync::Arc::new(encryptor));
        self
    }

    /// Drive all sources concurrently, merge results, run migrations, decrypt, deserialize.
    ///
    /// # Errors
    /// Returns [`LoadError`] on source, migration, crypto, or deserialization failure.
    pub async fn load(&self) -> Result<S, LoadError> {
        let strategies = S::field_strategies();

        let mut merged = ConfigValue::Table(std::collections::BTreeMap::new());
        for source in &self.sources {
            let collected = source.collect().await?;
            merged = merge(merged, collected, strategies);
        }

        let current_version = match &merged {
            ConfigValue::Table(t) => match t.get("__schema_version") {
                Some(ConfigValue::Integer(v)) => u32::try_from(*v).unwrap_or(0),
                _ => 0,
            },
            _ => 0,
        };

        if current_version < S::SCHEMA_VERSION
            && let Some(chain) = &self.migrations
        {
            merged = chain.run(merged, current_version, S::SCHEMA_VERSION)?;
        }

        if let Some(enc) = &self.encryptor {
            decrypt_fields(&mut merged, S::encrypted_fields(), enc.as_ref())?;
        }

        Ok(S::deserialize(ConfigValueDeserializer(merged))?)
    }
}
