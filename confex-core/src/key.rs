use std::{fs, path::PathBuf};

use rand::RngCore as _;

use crate::error::KeyError;

/// 256-bit symmetric key used for AES-256-GCM encryption.
pub struct SecretKey(pub(crate) [u8; 32]);

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

// ---------------------------------------------------------------------------
// FileKeyProvider
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// LinuxKeyring
// ---------------------------------------------------------------------------

/// Stores the key in the Linux kernel keyring (User keyring).
///
/// Session-persistent: survives process restarts within the same login session
/// but is lost on reboot. Requires the `linux-keyring` feature.
#[cfg(all(target_os = "linux", feature = "native-keyring"))]
#[derive(Debug, Default, Clone, Copy)]
#[must_use]
pub struct LinuxKeyring;

#[cfg(all(target_os = "linux", feature = "native-keyring"))]
impl KeyProvider for LinuxKeyring {
    fn key(&self, app_name: &str) -> Result<SecretKey, KeyError> {
        use keyutils::{Keyring, SpecialKeyring, keytypes::User};

        let description = format!("confex:{app_name}");
        let mut keyring = Keyring::attach_or_create(SpecialKeyring::User)
            .map_err(KeyError::Keyring)?;

        match keyring.search_for_key::<User>(description.as_str(), None) {
            Ok(key) => {
                let data = key.read().map_err(KeyError::Keyring)?;
                let len = data.len();
                let raw: [u8; 32] = data
                    .try_into()
                    .map_err(|_| KeyError::InvalidLength(len))?;
                Ok(SecretKey(raw))
            }
            Err(_) => {
                let mut raw = [0u8; 32];
                rand::thread_rng().fill_bytes(&mut raw);
                keyring
                    .add_key::<User>(description.as_str(), raw.as_ref())
                    .map_err(KeyError::Keyring)?;
                Ok(SecretKey(raw))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// MacOsKeychain
// ---------------------------------------------------------------------------

/// Stores the key in the macOS Keychain under service `"confex"`.
///
/// Requires the `macos-keychain` feature.
#[cfg(all(target_os = "macos", feature = "native-keyring"))]
#[derive(Debug, Default, Clone, Copy)]
#[must_use]
pub struct MacOsKeychain;

#[cfg(all(target_os = "macos", feature = "native-keyring"))]
impl KeyProvider for MacOsKeychain {
    fn key(&self, app_name: &str) -> Result<SecretKey, KeyError> {
        use security_framework::passwords::{get_generic_password, set_generic_password};

        match get_generic_password("confex", app_name) {
            Ok(data) => {
                let len = data.len();
                let raw: [u8; 32] = data
                    .try_into()
                    .map_err(|_| KeyError::InvalidLength(len))?;
                Ok(SecretKey(raw))
            }
            Err(_) => {
                let mut raw = [0u8; 32];
                rand::thread_rng().fill_bytes(&mut raw);
                set_generic_password("confex", app_name, &raw)
                    .map_err(KeyError::Keychain)?;
                Ok(SecretKey(raw))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// WindowsDpapi
// ---------------------------------------------------------------------------

/// Stores a DPAPI-encrypted key blob at `blob_path`.
///
/// Requires the `windows-dpapi` feature. Windows-only.
#[cfg(all(windows, feature = "native-keyring"))]
pub struct WindowsDpapi {
    blob_path: PathBuf,
}

#[cfg(all(windows, feature = "native-keyring"))]
impl WindowsDpapi {
    #[must_use]
    pub fn new(blob_path: PathBuf) -> Self {
        Self { blob_path }
    }
}

#[cfg(all(windows, feature = "native-keyring"))]
impl KeyProvider for WindowsDpapi {
    fn key(&self, _app_name: &str) -> Result<SecretKey, KeyError> {
        if self.blob_path.exists() {
            let blob = fs::read(&self.blob_path)?;
            let plain = dpapi_unprotect(&blob)?;
            let len = plain.len();
            let raw: [u8; 32] = plain
                .try_into()
                .map_err(|_| KeyError::InvalidLength(len))?;
            return Ok(SecretKey(raw));
        }

        let mut raw = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut raw);

        let blob = dpapi_protect(&raw)?;

        if let Some(parent) = self.blob_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&self.blob_path, &blob)?;

        Ok(SecretKey(raw))
    }
}

#[cfg(all(windows, feature = "native-keyring"))]
unsafe fn dpapi_protect(data: &[u8]) -> Result<Vec<u8>, KeyError> {
    use windows::{
        Win32::{
            Foundation::LocalFree,
            Security::Cryptography::{
                CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData,
            },
            System::Memory::HLOCAL,
        },
        core::PWSTR,
    };

    let cb_data =
        u32::try_from(data.len()).map_err(|_| KeyError::Dpapi("data too large".into()))?;
    let in_blob = CRYPT_INTEGER_BLOB {
        cbData: cb_data,
        // SAFETY: CryptProtectData takes the input blob as read-only; the pointer is valid
        // for the duration of the call and data outlives it.
        pbData: data.as_ptr().cast_mut(),
    };
    let mut out_blob = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };

    // SAFETY: All pointers are either null (optional params) or valid stack/heap references.
    // CryptProtectData allocates out_blob.pbData via LocalAlloc; we free it with LocalFree.
    let ok = unsafe {
        CryptProtectData(
            &in_blob,
            PWSTR::null(),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out_blob,
        )
    };
    if !ok.as_bool() {
        return Err(KeyError::Dpapi("CryptProtectData failed".into()));
    }

    let len = out_blob.cbData as usize;
    // SAFETY: out_blob.pbData was allocated by CryptProtectData and is valid for `len` bytes.
    let result = unsafe { std::slice::from_raw_parts(out_blob.pbData, len) }.to_vec();
    // SAFETY: out_blob.pbData was allocated by LocalAlloc (via CryptProtectData).
    unsafe { LocalFree(HLOCAL(out_blob.pbData.cast())) };

    Ok(result)
}

#[cfg(all(windows, feature = "native-keyring"))]
unsafe fn dpapi_unprotect(blob: &[u8]) -> Result<Vec<u8>, KeyError> {
    use windows::{
        Win32::{
            Foundation::LocalFree,
            Security::Cryptography::{
                CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData,
            },
            System::Memory::HLOCAL,
        },
    };

    let cb_data =
        u32::try_from(blob.len()).map_err(|_| KeyError::Dpapi("blob too large".into()))?;
    let in_blob = CRYPT_INTEGER_BLOB {
        cbData: cb_data,
        // SAFETY: CryptUnprotectData takes the input blob as read-only; the pointer is valid
        // for the duration of the call and blob outlives it.
        pbData: blob.as_ptr().cast_mut(),
    };
    let mut out_blob = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };

    // SAFETY: All optional parameters are null; in_blob and out_blob are valid references.
    // CryptUnprotectData allocates out_blob.pbData via LocalAlloc; freed below with LocalFree.
    let ok = unsafe {
        CryptUnprotectData(
            &in_blob,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out_blob,
        )
    };
    if !ok.as_bool() {
        return Err(KeyError::Dpapi("CryptUnprotectData failed".into()));
    }

    let len = out_blob.cbData as usize;
    // SAFETY: out_blob.pbData was allocated by CryptUnprotectData and is valid for `len` bytes.
    let result = unsafe { std::slice::from_raw_parts(out_blob.pbData, len) }.to_vec();
    // SAFETY: out_blob.pbData was allocated by LocalAlloc (via CryptUnprotectData).
    unsafe { LocalFree(HLOCAL(out_blob.pbData.cast())) };

    Ok(result)
}

// ---------------------------------------------------------------------------
// Unsealer trait + PassphraseUnsealer
// ---------------------------------------------------------------------------

/// Pluggable strategy for wrapping and unwrapping a 32-byte key into/from an opaque blob.
#[cfg(feature = "sealed-key")]
pub trait Unsealer {
    /// Encrypts `key` into an opaque blob.
    ///
    /// # Errors
    /// Returns [`KeyError`] if sealing fails.
    fn seal(&self, key: &[u8; 32]) -> Result<Vec<u8>, KeyError>;

    /// Decrypts `blob` back to a 32-byte key.
    ///
    /// # Errors
    /// Returns [`KeyError::InvalidBlob`] if the blob is malformed or decryption fails.
    fn unseal(&self, blob: &[u8]) -> Result<[u8; 32], KeyError>;
}

/// Seals a 32-byte key with a passphrase using Argon2id KDF + AES-256-GCM.
///
/// Blob layout: `[32-byte Argon2id salt][12-byte AES nonce][32-byte ciphertext][16-byte GCM tag]`
/// = 92 bytes total.
#[cfg(feature = "sealed-key")]
pub struct PassphraseUnsealer {
    passphrase: String,
}

#[cfg(feature = "sealed-key")]
impl PassphraseUnsealer {
    /// Creates a new unsealer from the given passphrase.
    #[must_use]
    pub fn new(passphrase: impl Into<String>) -> Self {
        Self {
            passphrase: passphrase.into(),
        }
    }

    /// Creates a new unsealer by reading the passphrase from an environment variable.
    ///
    /// # Errors
    /// Returns [`KeyError::Passphrase`] if the variable is absent or not valid UTF-8.
    pub fn from_env(var: &str) -> Result<Self, KeyError> {
        let passphrase = std::env::var(var).map_err(|_| KeyError::Passphrase)?;
        Ok(Self { passphrase })
    }

    fn derive_kek(&self, salt: &[u8]) -> Result<[u8; 32], KeyError> {
        use argon2::{Algorithm, Argon2, Params, Version};

        let params =
            Params::new(65_536, 3, 4, Some(32)).map_err(KeyError::Kdf)?;
        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        let mut kek = [0u8; 32];
        argon2
            .hash_password_into(self.passphrase.as_bytes(), salt, &mut kek)
            .map_err(KeyError::Kdf)?;
        Ok(kek)
    }
}

#[cfg(feature = "sealed-key")]
impl Unsealer for PassphraseUnsealer {
    fn seal(&self, key: &[u8; 32]) -> Result<Vec<u8>, KeyError> {
        use crate::crypto::Aes256GcmEncryptor;
        use crate::crypto::Encryptor as _;

        let mut salt = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut salt);

        let kek = self.derive_kek(&salt)?;
        let encryptor = Aes256GcmEncryptor::new(&SecretKey(kek));
        let encrypted = encryptor
            .encrypt(key)
            .map_err(|_| KeyError::InvalidBlob)?;

        // salt (32) + nonce+ciphertext+tag (12+32+16 = 60) = 92 bytes
        let mut blob = Vec::with_capacity(32 + encrypted.len());
        blob.extend_from_slice(&salt);
        blob.extend_from_slice(&encrypted);
        Ok(blob)
    }

    fn unseal(&self, blob: &[u8]) -> Result<[u8; 32], KeyError> {
        use crate::crypto::Aes256GcmEncryptor;
        use crate::crypto::Encryptor as _;

        if blob.len() < 33 {
            return Err(KeyError::InvalidBlob);
        }
        let (salt, ciphertext) = blob.split_at(32);
        let kek = self.derive_kek(salt)?;
        let encryptor = Aes256GcmEncryptor::new(&SecretKey(kek));
        let plain = encryptor.decrypt(ciphertext).map_err(|_| KeyError::InvalidBlob)?;

        let len = plain.len();
        plain
            .try_into()
            .map_err(|_| KeyError::InvalidLength(len))
    }
}

// ---------------------------------------------------------------------------
// Linux keyring cache helpers (used by SealedFileKeyProvider)
// ---------------------------------------------------------------------------

#[cfg(all(target_os = "linux", feature = "native-keyring"))]
fn keyring_read(description: &str) -> Option<[u8; 32]> {
    use keyutils::{Keyring, SpecialKeyring, keytypes::User};

    let keyring = Keyring::attach_or_create(SpecialKeyring::User).ok()?;
    let key = keyring.search_for_key::<User>(description, None).ok()?;
    let data = key.read().ok()?;
    data.try_into().ok()
}

#[cfg(all(target_os = "linux", feature = "native-keyring"))]
fn keyring_write(description: &str, raw: &[u8; 32]) {
    use keyutils::{Keyring, SpecialKeyring, keytypes::User};

    if let Ok(mut keyring) = Keyring::attach_or_create(SpecialKeyring::User) {
        // Best-effort cache: ignore errors.
        let _ = keyring.add_key::<User>(description, raw.as_ref());
    }
}

// ---------------------------------------------------------------------------
// SealedFileKeyProvider
// ---------------------------------------------------------------------------

/// Stores a sealed (encrypted) key blob on disk, using a pluggable [`Unsealer`].
///
/// On Linux with the `linux-keyring` feature, caches the unsealed key in the
/// kernel keyring after the first unseal to avoid repeated KDF cost.
#[cfg(feature = "sealed-key")]
pub struct SealedFileKeyProvider<U: Unsealer> {
    path: PathBuf,
    unsealer: U,
}

#[cfg(feature = "sealed-key")]
impl<U: Unsealer> SealedFileKeyProvider<U> {
    #[must_use]
    pub fn new(path: PathBuf, unsealer: U) -> Self {
        Self { path, unsealer }
    }
}

#[cfg(feature = "sealed-key")]
impl<U: Unsealer> KeyProvider for SealedFileKeyProvider<U> {
    fn key(&self, _app_name: &str) -> Result<SecretKey, KeyError> {
        #[cfg(all(target_os = "linux", feature = "native-keyring"))]
        let cache_desc = format!("confex.sealed:{_app_name}");

        // Fast path: kernel keyring cache (Linux only).
        #[cfg(all(target_os = "linux", feature = "native-keyring"))]
        if let Some(raw) = keyring_read(&cache_desc) {
            return Ok(SecretKey(raw));
        }

        if self.path.exists() {
            let blob = fs::read(&self.path)?;
            let raw = self.unsealer.unseal(&blob)?;

            #[cfg(all(target_os = "linux", feature = "native-keyring"))]
            keyring_write(&cache_desc, &raw);

            return Ok(SecretKey(raw));
        }

        // Generate a new key, seal it, and persist the blob.
        let mut raw = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut raw);

        let blob = self.unsealer.seal(&raw)?;

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&self.path, &blob)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600))?;
        }

        #[cfg(all(target_os = "linux", feature = "native-keyring"))]
        keyring_write(&cache_desc, &raw);

        Ok(SecretKey(raw))
    }
}

// ---------------------------------------------------------------------------
// NativeKeyring alias — platform-transparent accessor
// ---------------------------------------------------------------------------

/// Platform-native keyring provider, selected at compile time.
///
/// - Linux: [`LinuxKeyring`] (kernel keyring, session-persistent)
/// - macOS: [`MacOsKeychain`] (system Keychain, login-persistent)
/// - Windows: not aliased — use [`WindowsDpapi`] directly (requires a blob path).
///
/// Requires the `native-keyring` feature.
#[cfg(all(target_os = "linux", feature = "native-keyring"))]
pub type NativeKeyring = LinuxKeyring;

#[cfg(all(target_os = "macos", feature = "native-keyring"))]
pub type NativeKeyring = MacOsKeychain;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn file_key_provider_generates_and_persists() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("key");
        let provider = FileKeyProvider::new(path.clone());
        let k1 = provider.key("app").unwrap();
        let k2 = provider.key("app").unwrap();
        assert_eq!(k1.as_bytes(), k2.as_bytes());
    }

    #[cfg(all(target_os = "linux", feature = "native-keyring"))]
    #[test]
    fn linux_keyring_stores_and_retrieves() {
        // use unique description to avoid test collision
        let provider = LinuxKeyring;
        let app = "confex-test-keyring-42";
        let k1 = provider.key(app).unwrap();
        let k2 = provider.key(app).unwrap();
        assert_eq!(k1.as_bytes(), k2.as_bytes());
    }

    #[cfg(feature = "sealed-key")]
    #[test]
    fn passphrase_unsealer_round_trip() {
        let unsealer = PassphraseUnsealer::new("hunter2");
        let key = [0x42u8; 32];
        let blob = unsealer.seal(&key).unwrap();
        let recovered = unsealer.unseal(&blob).unwrap();
        assert_eq!(key, recovered);
    }

    #[cfg(feature = "sealed-key")]
    #[test]
    fn sealed_provider_creates_and_loads() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("key.sealed");
        let provider = SealedFileKeyProvider::new(path, PassphraseUnsealer::new("secret"));
        let k1 = provider.key("app").unwrap();
        let k2 = provider.key("app").unwrap();
        assert_eq!(k1.as_bytes(), k2.as_bytes());
    }

    #[cfg(feature = "sealed-key")]
    #[test]
    fn sealed_provider_wrong_passphrase_errors() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("key.sealed");
        let p1 = SealedFileKeyProvider::new(path.clone(), PassphraseUnsealer::new("correct"));
        p1.key("app").unwrap();
        let p2 = SealedFileKeyProvider::new(path, PassphraseUnsealer::new("wrong"));
        assert!(matches!(p2.key("app"), Err(KeyError::InvalidBlob)));
    }
}
