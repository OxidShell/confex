use std::path::Path;

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, AeadCore, KeyInit},
};
use rand::rngs::OsRng;

use crate::{
    error::{CryptoError, SourceError},
    format::Format,
    key::SecretKey,
    value::ConfigValue,
};

/// Symmetric encrypt/decrypt over raw bytes.
pub trait Encryptor {
    /// # Errors
    /// Returns [`CryptoError::Encrypt`] on failure.
    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, CryptoError>;

    /// # Errors
    /// Returns [`CryptoError::Decrypt`] or [`CryptoError::InvalidFormat`] on failure.
    fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError>;
}

/// AES-256-GCM encryptor. Prepends the 12-byte nonce to the ciphertext.
pub struct Aes256GcmEncryptor {
    cipher: Aes256Gcm,
}

impl From<&SecretKey> for Aes256GcmEncryptor {
    fn from(key: &SecretKey) -> Self {
        Self {
            cipher: Aes256Gcm::new(key.as_bytes().into()),
        }
    }
}

const NONCE_LEN: usize = 12;

impl Encryptor for Aes256GcmEncryptor {
    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let nonce = Aes256Gcm::generate_nonce(OsRng);
        let ct = self
            .cipher
            .encrypt(&nonce, plaintext)
            .map_err(|_| CryptoError::Encrypt)?;

        let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ct);
        Ok(out)
    }

    fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        if ciphertext.len() < NONCE_LEN {
            return Err(CryptoError::InvalidFormat);
        }
        let (nonce_bytes, ct) = ciphertext.split_at(NONCE_LEN);
        let nonce = Nonce::from_slice(nonce_bytes);
        self.cipher
            .decrypt(nonce, ct)
            .map_err(|_| CryptoError::Decrypt)
    }
}

/// Encrypt the string values of named fields in a `ConfigValue::Table`.
///
/// Encrypted bytes are hex-encoded and stored back as strings.
/// Recurses into nested tables.
///
/// # Errors
/// Returns [`CryptoError::NotAString`] if a targeted field is not a string value.
pub fn encrypt_fields(
    value: &mut ConfigValue,
    field_names: &[&str],
    encryptor: &dyn Encryptor,
) -> Result<(), CryptoError> {
    let ConfigValue::Table(table) = value else {
        return Ok(());
    };
    for (key, val) in table.iter_mut() {
        if field_names.contains(&key.as_str()) {
            let ConfigValue::String(s) = val else {
                return Err(CryptoError::NotAString(key.clone()));
            };
            let encrypted = encryptor.encrypt(s.as_bytes())?;
            *val = ConfigValue::String(hex::encode(encrypted));
        } else if matches!(val, ConfigValue::Table(_)) {
            encrypt_fields(val, field_names, encryptor)?;
        }
    }
    Ok(())
}

/// Decrypt hex-encoded string values of named fields in a `ConfigValue::Table`.
///
/// # Errors
/// Returns an error if hex decoding, decryption, or UTF-8 conversion fails.
pub fn decrypt_fields(
    value: &mut ConfigValue,
    field_names: &[&str],
    encryptor: &dyn Encryptor,
) -> Result<(), CryptoError> {
    let ConfigValue::Table(table) = value else {
        return Ok(());
    };
    for (key, val) in table.iter_mut() {
        if field_names.contains(&key.as_str()) {
            let ConfigValue::String(s) = val else {
                return Err(CryptoError::NotAString(key.clone()));
            };
            let bytes = hex::decode(s.as_str()).map_err(|_| CryptoError::InvalidFormat)?;
            let plain = encryptor.decrypt(&bytes)?;
            *val = ConfigValue::String(
                String::from_utf8(plain).map_err(|_| CryptoError::Decrypt)?,
            );
        } else if matches!(val, ConfigValue::Table(_)) {
            decrypt_fields(val, field_names, encryptor)?;
        }
    }
    Ok(())
}

/// Wraps a [`Format`] to transparently decrypt on parse and encrypt on serialize.
///
/// Use this for "app data" files that should be opaque on disk (e.g. session tokens).
pub struct EncryptedFormat<'a, F: Format> {
    inner: F,
    encryptor: &'a dyn Encryptor,
}

impl<'a, F: Format> EncryptedFormat<'a, F> {
    #[must_use]
    pub fn new(inner: F, encryptor: &'a dyn Encryptor) -> Self {
        Self { inner, encryptor }
    }
}

impl<F: Format> Format for EncryptedFormat<'_, F> {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    fn parse(&self, path: &Path, bytes: &[u8]) -> Result<ConfigValue, SourceError> {
        let decrypted = self.encryptor.decrypt(bytes)?;
        self.inner.parse(path, &decrypted)
    }

    fn serialize(&self, value: &ConfigValue) -> Result<Vec<u8>, SourceError> {
        let bytes = self.inner.serialize(value)?;
        Ok(self.encryptor.encrypt(&bytes)?)
    }
}
