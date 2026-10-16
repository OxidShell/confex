#![warn(clippy::pedantic)]

pub use confex_core::config::{ConfigLoader, ConfigSchema};
pub use confex_core::crypto::{
    Aes256GcmEncryptor, EncryptedFormat, Encryptor, decrypt_fields, encrypt_fields,
};
pub use confex_core::error::{CryptoError, KeyError, LoadError, MigrationError, SourceError};
pub use confex_core::format::Format;
#[cfg(feature = "toml")]
pub use confex_core::format::TomlFormat;
pub use confex_core::key::{FileKeyProvider, KeyProvider, SecretKey};

#[cfg(feature = "native-keyring")]
pub use confex_core::key::NativeKeyring;

#[cfg(all(windows, feature = "native-keyring"))]
pub use confex_core::key::WindowsCredentialManager;

#[cfg(feature = "sealed-key")]
pub use confex_core::key::{PassphraseUnsealer, SealedFileKeyProvider, Unsealer};
pub use confex_core::merge::{FieldStrategies, MergeStrategy};
pub use confex_core::migrate::{Migration, MigrationChain, rename_key};
pub use confex_core::paths::{app_config_dir, app_data_dir, config_base_dir, data_base_dir};
#[cfg(feature = "hot-reload")]
pub use confex_core::source::WatchableSource;
pub use confex_core::source::{CliSource, EnvSource, FileSource, Source};

#[cfg(feature = "async")]
pub use confex_core::config::AsyncConfigLoader;
#[cfg(feature = "async")]
pub use confex_core::source::{AsyncSource, CollectFuture};
pub use confex_core::value::ConfigValue;
// Re-export the derive macro under the same name as the trait (two namespaces, no conflict)
#[cfg(feature = "macros")]
pub use confex_macros::ConfigSchema;

#[cfg(feature = "redb-backend")]
pub use confex_core::redb::{RedbSource, RedbStore};

#[cfg(feature = "hot-reload")]
pub use confex_core::handle::WatcherHandle;
pub use confex_core::handle::{ConfigHandle, HotReloadError, WatchOptions, WriteGuard};
