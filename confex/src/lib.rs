#![warn(clippy::pedantic)]

pub use confex_core::config::{ConfigLoader, ConfigSchema};
pub use confex_core::crypto::{
    Aes256GcmEncryptor, EncryptedFormat, Encryptor, decrypt_fields, encrypt_fields,
};
pub use confex_core::error::{CryptoError, KeyError, LoadError, MigrationError, SourceError};
pub use confex_core::format::{Format, TomlFormat};
pub use confex_core::key::{FileKeyProvider, KeyProvider, SecretKey};
pub use confex_core::merge::{FieldStrategies, MergeStrategy};
pub use confex_core::migrate::{Migration, MigrationChain, rename_key};
pub use confex_core::paths::{app_config_dir, app_data_dir, config_base_dir, data_base_dir};
pub use confex_core::source::{CliSource, EnvSource, FileSource, Source};
pub use confex_core::value::ConfigValue;
// Re-export the derive macro under the same name as the trait (two namespaces, no conflict)
pub use confex_macros::ConfigSchema;
