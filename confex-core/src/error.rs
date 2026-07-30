use thiserror::Error;

#[derive(Debug, Error)]
pub enum SourceError {
    #[error("I/O error reading {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parse error in {path}: {message}")]
    Parse {
        path: std::path::PathBuf,
        message: String,
    },
    #[error("serialization error: {0}")]
    Serialize(String),
    #[error("environment variable error: {0}")]
    Env(String),
    #[error("crypto error: {0}")]
    Crypto(#[from] CryptoError),
}

#[derive(Debug, Error)]
pub enum LoadError {
    #[error(transparent)]
    Source(#[from] SourceError),
    #[error(transparent)]
    Migration(#[from] MigrationError),
    #[error(transparent)]
    Crypto(#[from] CryptoError),
    #[error("deserialization failed: {0}")]
    Deserialize(#[from] serde_json::Error),
}

#[derive(Debug, Error)]
pub enum MigrationError {
    #[error("no migration path from version {from} to {to}")]
    NoPath { from: u32, to: u32 },
    #[error("migration step failed: {0}")]
    Step(String),
}

#[derive(Debug, Error)]
pub enum KeyError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid key length: expected 32 bytes, got {0}")]
    InvalidLength(usize),
}

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("encryption failed")]
    Encrypt,
    #[error("decryption failed")]
    Decrypt,
    #[error("invalid ciphertext format")]
    InvalidFormat,
    #[error("value at field '{0}' is not a string")]
    NotAString(String),
}
