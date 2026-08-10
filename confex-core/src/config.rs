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

/// Builds and executes a layered configuration load.
///
/// Sources are merged in registration order (first = lowest priority).
/// Because sources are stored as `Arc`, the loader is cheaply cloneable and
/// its sources can be captured in hot-reload closures without duplicating state.
pub struct ConfigLoader<S: ConfigSchema> {
    sources: Vec<std::sync::Arc<dyn Source + Send + Sync>>,
    migrations: Option<std::sync::Arc<MigrationChain>>,
    encryptor: Option<std::sync::Arc<dyn Encryptor>>,
    _phantom: std::marker::PhantomData<S>,
}

impl<S: ConfigSchema> Clone for ConfigLoader<S> {
    fn clone(&self) -> Self {
        Self {
            sources: self.sources.clone(),
            migrations: self.migrations.clone(),
            encryptor: self.encryptor.clone(),
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<S: ConfigSchema> Default for ConfigLoader<S> {
    fn default() -> Self {
        Self {
            sources: Vec::new(),
            migrations: None,
            encryptor: None,
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<S: ConfigSchema> ConfigLoader<S> {
    #[must_use]
    pub fn with_source(mut self, source: impl Source + Send + Sync + 'static) -> Self {
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

    /// Merge all sources, run migrations if needed, decrypt fields, and deserialize.
    ///
    /// # Errors
    /// Returns [`LoadError`] on source, migration, crypto, or deserialization failure.
    pub fn load(&self) -> Result<S, LoadError> {
        let strategies = S::field_strategies();

        let mut merged = ConfigValue::Table(BTreeMap::new());
        for source in &self.sources {
            let collected = source.collect()?;
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

// ── hot-reload ────────────────────────────────────────────────────────────────

#[cfg(feature = "hot-reload")]
impl<S: ConfigSchema + Clone + Send + Sync + 'static> ConfigLoader<S> {
    /// Watch `watch_path` for changes and reload the full source stack automatically.
    ///
    /// On every detected change (after the debounce window) all sources are
    /// re-collected, merged, migrated, decrypted, and deserialized — exactly
    /// the same pipeline as [`load`](ConfigLoader::load). The resulting
    /// [`crate::handle::ConfigHandle`] is updated atomically so all readers
    /// see the new value lock-free.
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
}

// ── async loader ──────────────────────────────────────────────────────────────

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
