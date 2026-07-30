use std::{fs, path::PathBuf};

use rand::RngCore as _;

use crate::error::KeyError;

/// 256-bit symmetric key used for AES-256-GCM encryption.
pub struct SecretKey([u8; 32]);

impl SecretKey {
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Retrieves or creates the encryption key for an application.
pub trait KeyProvider {
    /// Returns the key for `app_name`.
    ///
    /// # Errors
    /// Returns [`KeyError`] on I/O failure or invalid stored key data.
    fn key(&self, app_name: &str) -> Result<SecretKey, KeyError>;
}

/// Stores the key as a 32-byte file with permissions `0600` (Unix).
///
/// Generates a new random key on first use and persists it.
///
/// # Security note
/// The key lives in the same blast radius as `$HOME`. This protects against
/// other local users, not against a full session compromise. See plan §5.
pub struct FileKeyProvider {
    path: PathBuf,
}

impl FileKeyProvider {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl KeyProvider for FileKeyProvider {
    fn key(&self, _app_name: &str) -> Result<SecretKey, KeyError> {
        if self.path.exists() {
            let bytes = fs::read(&self.path)?;
            let len = bytes.len();
            let raw: [u8; 32] = bytes
                .try_into()
                .map_err(|_| KeyError::InvalidLength(len))?;
            return Ok(SecretKey(raw));
        }

        let mut raw = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut raw);

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&self.path, raw)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600))?;
        }

        Ok(SecretKey(raw))
    }
}
