use std::{fmt, path::PathBuf};

#[derive(Debug)]
pub enum SourceError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Parse {
        path: PathBuf,
        message: String,
    },
    Serialize(String),
    Env(String),
    Crypto(CryptoError),
    Backend(String),
    #[cfg(feature = "sqlite-backend")]
    Sqlite(rusqlite::Error),
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "I/O error reading {}: {source}", path.display())
            }
            Self::Parse { path, message } => {
                write!(f, "parse error in {}: {message}", path.display())
            }
            Self::Serialize(msg) => write!(f, "serialization error: {msg}"),
            Self::Env(msg) => write!(f, "environment variable error: {msg}"),
            Self::Crypto(e) => write!(f, "crypto error: {e}"),
            Self::Backend(msg) => write!(f, "backend error: {msg}"),
            #[cfg(feature = "sqlite-backend")]
            Self::Sqlite(e) => write!(f, "SQLite error: {e}"),
        }
    }
}

impl std::error::Error for SourceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Crypto(e) => Some(e),
            #[cfg(feature = "sqlite-backend")]
            Self::Sqlite(e) => Some(e),
            _ => None,
        }
    }
}

impl From<CryptoError> for SourceError {
    fn from(e: CryptoError) -> Self {
        Self::Crypto(e)
    }
}

#[cfg(feature = "sqlite-backend")]
impl From<rusqlite::Error> for SourceError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}

#[derive(Debug)]
pub enum LoadError {
    Source(SourceError),
    Migration(MigrationError),
    Crypto(CryptoError),
    Deserialize(crate::deserialize::DeserializeError),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(e) => fmt::Display::fmt(e, f),
            Self::Migration(e) => fmt::Display::fmt(e, f),
            Self::Crypto(e) => fmt::Display::fmt(e, f),
            Self::Deserialize(e) => write!(f, "deserialization failed: {e}"),
        }
    }
}

impl std::error::Error for LoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Source(e) => Some(e),
            Self::Migration(e) => Some(e),
            Self::Crypto(e) => Some(e),
            Self::Deserialize(e) => Some(e),
        }
    }
}

impl From<SourceError> for LoadError {
    fn from(e: SourceError) -> Self {
        Self::Source(e)
    }
}

impl From<MigrationError> for LoadError {
    fn from(e: MigrationError) -> Self {
        Self::Migration(e)
    }
}

impl From<CryptoError> for LoadError {
    fn from(e: CryptoError) -> Self {
        Self::Crypto(e)
    }
}

impl From<crate::deserialize::DeserializeError> for LoadError {
    fn from(e: crate::deserialize::DeserializeError) -> Self {
        Self::Deserialize(e)
    }
}

#[derive(Debug)]
pub enum MigrationError {
    NoPath { from: u32, to: u32 },
    Step(String),
}

impl fmt::Display for MigrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoPath { from, to } => {
                write!(f, "no migration path from version {from} to {to}")
            }
            Self::Step(msg) => write!(f, "migration step failed: {msg}"),
        }
    }
}

impl std::error::Error for MigrationError {}

#[derive(Debug)]
pub enum KeyError {
    Io(std::io::Error),
    InvalidLength(usize),
    /// No Secret Service daemon available (headless or server environment).
    #[cfg(all(target_os = "linux", feature = "native-keyring"))]
    NoDaemon,
    #[cfg(all(target_os = "linux", feature = "native-keyring"))]
    SecretService(String),
    #[cfg(all(target_os = "macos", feature = "native-keyring"))]
    Keychain(security_framework::base::Error),
    #[cfg(all(windows, feature = "native-keyring"))]
    CredentialManager(String),
    #[cfg(feature = "sealed-key")]
    Kdf(argon2::Error),
    InvalidBlob,
    Passphrase,
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::InvalidLength(n) => {
                write!(f, "invalid key length: expected 32 bytes, got {n}")
            }
            #[cfg(all(target_os = "linux", feature = "native-keyring"))]
            Self::NoDaemon => write!(f, "Secret Service daemon not available"),
            #[cfg(all(target_os = "linux", feature = "native-keyring"))]
            Self::SecretService(e) => write!(f, "Secret Service error: {e}"),
            #[cfg(all(target_os = "macos", feature = "native-keyring"))]
            Self::Keychain(e) => write!(f, "keychain error: {e}"),
            #[cfg(all(windows, feature = "native-keyring"))]
            Self::CredentialManager(msg) => write!(f, "Credential Manager error: {msg}"),
            #[cfg(feature = "sealed-key")]
            Self::Kdf(e) => write!(f, "KDF error: {e}"),
            Self::InvalidBlob => write!(f, "sealed key blob is invalid"),
            Self::Passphrase => write!(f, "passphrase not available"),
        }
    }
}

impl std::error::Error for KeyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            #[cfg(all(target_os = "linux", feature = "native-keyring"))]
            Self::NoDaemon | Self::SecretService(_) => None,
            #[cfg(all(target_os = "macos", feature = "native-keyring"))]
            Self::Keychain(e) => Some(e),
            #[cfg(feature = "sealed-key")]
            Self::Kdf(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for KeyError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

#[cfg(all(target_os = "macos", feature = "native-keyring"))]
impl From<security_framework::base::Error> for KeyError {
    fn from(e: security_framework::base::Error) -> Self {
        Self::Keychain(e)
    }
}

#[cfg(feature = "sealed-key")]
impl From<argon2::Error> for KeyError {
    fn from(e: argon2::Error) -> Self {
        Self::Kdf(e)
    }
}

#[derive(Debug)]
pub enum CryptoError {
    Encrypt,
    Decrypt,
    InvalidFormat,
    NotAString(String),
}

impl fmt::Display for CryptoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Encrypt => write!(f, "encryption failed"),
            Self::Decrypt => write!(f, "decryption failed"),
            Self::InvalidFormat => write!(f, "invalid ciphertext format"),
            Self::NotAString(field) => write!(f, "value at field '{field}' is not a string"),
        }
    }
}

impl std::error::Error for CryptoError {}
