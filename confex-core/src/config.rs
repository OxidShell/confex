use std::collections::BTreeMap;

use serde::de::DeserializeOwned;

use crate::{
    crypto::{Encryptor, decrypt_fields},
    error::LoadError,
    merge::{FieldStrategies, merge},
    migrate::MigrationChain,
    source::Source,
    value::ConfigValue,
};

/// Marker trait for types that can be loaded via [`ConfigLoader`].
///
/// Implement manually or via `#[derive(ConfigSchema)]` (Phase 1).
pub trait ConfigSchema: DeserializeOwned {
    const APP_NAME: &'static str;
    const SCHEMA_VERSION: u32;

    /// Per-field merge strategies. Default: all fields use `Replace`.
    fn field_strategies() -> FieldStrategies {
        &[]
    }

    /// Field names whose string values are encrypted at rest.
    fn encrypted_fields() -> &'static [&'static str] {
        &[]
    }
}

/// Builds and executes a layered configuration load.
///
/// Sources are merged in registration order (first = lowest priority).
pub struct ConfigLoader<S: ConfigSchema> {
    sources: Vec<Box<dyn Source>>,
    migrations: Option<MigrationChain>,
    encryptor: Option<Box<dyn Encryptor>>,
    _phantom: std::marker::PhantomData<S>,
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
    pub fn with_source(mut self, source: impl Source + 'static) -> Self {
        self.sources.push(Box::new(source));
        self
    }

    #[must_use]
    pub fn with_migrations(mut self, migrations: MigrationChain) -> Self {
        self.migrations = Some(migrations);
        self
    }

    #[must_use]
    pub fn with_encryptor(mut self, encryptor: impl Encryptor + 'static) -> Self {
        self.encryptor = Some(Box::new(encryptor));
        self
    }

    /// Merge all sources, run migrations if needed, decrypt fields, and deserialize.
    ///
    /// # Errors
    /// Returns [`LoadError`] on source, migration, crypto, or deserialization failure.
    pub fn load(self) -> Result<S, LoadError> {
        let strategies = S::field_strategies();

        let mut merged = ConfigValue::Table(BTreeMap::new());
        for source in self.sources {
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

        if current_version < S::SCHEMA_VERSION {
            if let Some(chain) = self.migrations {
                merged = chain.run(merged, current_version, S::SCHEMA_VERSION)?;
            }
        }

        if let Some(enc) = &self.encryptor {
            decrypt_fields(&mut merged, S::encrypted_fields(), enc.as_ref())?;
        }

        let json: serde_json::Value = merged.into();
        Ok(serde_json::from_value(json)?)
    }
}
