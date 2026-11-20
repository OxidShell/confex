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

impl From<PathBuf> for FileKeyProvider {
    fn from(path: PathBuf) -> Self {
        Self { path }
    }
}

impl KeyProvider for FileKeyProvider {
    fn key(&self, _app_name: &str) -> Result<SecretKey, KeyError> {
        if self.path.exists() {
            let bytes = fs::read(&self.path)?;
            let len = bytes.len();
            let raw: [u8; 32] = bytes.try_into().map_err(|_| KeyError::InvalidLength(len))?;
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

/// Stores the key in the system Secret Service (`org.freedesktop.secrets` via D-Bus).
///
/// Persistent across reboots. Requires a running Secret Service daemon (GNOME Keyring,
/// `KWallet`). On headless or server systems, use [`FileKeyProvider`] instead.
///
/// Requires the `native-keyring` feature.
#[cfg(all(target_os = "linux", feature = "native-keyring"))]
#[derive(Debug, Default, Clone, Copy)]
#[must_use]
pub struct SecretServiceKeyring;

#[cfg(all(target_os = "linux", feature = "native-keyring"))]
impl KeyProvider for SecretServiceKeyring {
    fn key(&self, app_name: &str) -> Result<SecretKey, KeyError> {
        ss_get_or_create(app_name).map(SecretKey)
    }
}

#[cfg(all(target_os = "linux", feature = "native-keyring"))]
fn ss_get_or_create(app_name: &str) -> Result<[u8; 32], KeyError> {
    use std::collections::HashMap;

    use zbus::{
        blocking::{Connection, Proxy},
        zvariant::{OwnedObjectPath, OwnedValue, Value},
    };

    const SERVICE: &str = "org.freedesktop.secrets";
    const SERVICE_PATH: &str = "/org/freedesktop/secrets";
    const SERVICE_IFACE: &str = "org.freedesktop.Secret.Service";
    const ITEM_IFACE: &str = "org.freedesktop.Secret.Item";
    const COLLECTION_IFACE: &str = "org.freedesktop.Secret.Collection";

    let conn = Connection::session().map_err(|_| KeyError::NoDaemon)?;

    let svc = Proxy::new(&conn, SERVICE, SERVICE_PATH, SERVICE_IFACE)
        .map_err(|e| KeyError::SecretService(e.to_string()))?;

    // Plain session — no transport encryption needed on local socket.
    let (_out, session): (OwnedValue, OwnedObjectPath) = svc
        .call("OpenSession", &("plain", Value::from("")))
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("ServiceUnknown") {
                KeyError::NoDaemon
            } else {
                KeyError::SecretService(msg)
            }
        })?;

    let attrs: HashMap<&str, &str> = [("application", "confex"), ("name", app_name)]
        .into_iter()
        .collect();

    let (mut unlocked, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) = svc
        .call("SearchItems", &attrs)
        .map_err(|e| KeyError::SecretService(e.to_string()))?;

    if !locked.is_empty() {
        let (newly, _prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) = svc
            .call("Unlock", &locked)
            .map_err(|e| KeyError::SecretService(e.to_string()))?;
        unlocked.extend(newly);
    }

    if let Some(item_path) = unlocked.into_iter().next() {
        let item = Proxy::new(&conn, SERVICE, item_path, ITEM_IFACE)
            .map_err(|e| KeyError::SecretService(e.to_string()))?;
        let (_sess, _params, value, _ct): (OwnedObjectPath, Vec<u8>, Vec<u8>, String) = item
            .call("GetSecret", &session.as_ref())
            .map_err(|e| KeyError::SecretService(e.to_string()))?;
        let len = value.len();
        return value.try_into().map_err(|_| KeyError::InvalidLength(len));
    }

    // No existing item — generate and store.
    let mut raw = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut raw);

    let collection_path: OwnedObjectPath = svc
        .call("ReadAlias", &"default")
        .map_err(|e| KeyError::SecretService(e.to_string()))?;

    if collection_path.as_str() == "/" {
        return Err(KeyError::SecretService(
            "no default Secret Service collection".into(),
        ));
    }

    let label = format!("confex:{app_name}");
    let item_attrs: HashMap<String, String> = [
        ("application".to_owned(), "confex".to_owned()),
        ("name".to_owned(), app_name.to_owned()),
    ]
    .into_iter()
    .collect();
    let mut props: HashMap<String, Value<'_>> = HashMap::new();
    props.insert(
        "org.freedesktop.Secret.Item.Label".to_owned(),
        Value::from(label.as_str()),
    );
    props.insert(
        "org.freedesktop.Secret.Item.Attributes".to_owned(),
        Value::from(item_attrs),
    );

    // Secret struct: (session_path, parameters, value, content_type)
    let secret = (
        session.as_ref(),
        &b""[..],
        &raw[..],
        "application/octet-stream",
    );

    let collection = Proxy::new(&conn, SERVICE, collection_path, COLLECTION_IFACE)
        .map_err(|e| KeyError::SecretService(e.to_string()))?;

    let _: (OwnedObjectPath, OwnedObjectPath) = collection
        .call("CreateItem", &(props, secret, true))
        .map_err(|e| KeyError::SecretService(e.to_string()))?;

    Ok(raw)
}

/// Stores the key in the macOS Keychain under service `"confex"`.
///
/// Requires the `native-keyring` feature.
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
                let raw: [u8; 32] = data.try_into().map_err(|_| KeyError::InvalidLength(len))?;
                Ok(SecretKey(raw))
            }
            Err(_) => {
                let mut raw = [0u8; 32];
                rand::thread_rng().fill_bytes(&mut raw);
                set_generic_password("confex", app_name, &raw).map_err(KeyError::Keychain)?;
                Ok(SecretKey(raw))
            }
        }
    }
}

/// Stores the key in the Windows Credential Manager.
///
/// Persistent across reboots, bound to the current user. Requires the
/// `native-keyring` feature.
#[cfg(all(windows, feature = "native-keyring"))]
#[derive(Debug, Default, Clone, Copy)]
#[must_use]
pub struct WindowsCredentialManager;

#[cfg(all(windows, feature = "native-keyring"))]
impl KeyProvider for WindowsCredentialManager {
    fn key(&self, app_name: &str) -> Result<SecretKey, KeyError> {
        if let Some(raw) = cred_read(app_name)? {
            return Ok(SecretKey(raw));
        }
        let mut raw = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut raw);
        cred_write(app_name, &raw)?;
        Ok(SecretKey(raw))
    }
}

#[cfg(all(windows, feature = "native-keyring"))]
fn cred_target(app_name: &str) -> Vec<u16> {
    format!("confex:{app_name}")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(all(windows, feature = "native-keyring"))]
fn cred_read(app_name: &str) -> Result<Option<[u8; 32]>, KeyError> {
    use windows::Win32::{
        Foundation::{ERROR_NOT_FOUND, GetLastError},
        Security::Credentials::{CRED_TYPE_GENERIC, CredFree, CredReadW},
    };
    use windows::core::PCWSTR;

    let target = cred_target(app_name);
    let mut pcred = std::ptr::null_mut();

    // SAFETY: target is a valid null-terminated UTF-16 string; pcred is a valid out-pointer.
    let ok = unsafe { CredReadW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, 0, &mut pcred) };

    if !ok.as_bool() {
        // SAFETY: called immediately after a failed Win32 API on the same thread.
        let err = unsafe { GetLastError() };
        return if err == ERROR_NOT_FOUND {
            Ok(None)
        } else {
            Err(KeyError::CredentialManager(format!(
                "CredReadW failed: {err:?}"
            )))
        };
    }

    // SAFETY: CredReadW succeeded; pcred points to a valid CREDENTIALW.
    let cred = unsafe { &*pcred };
    let blob_size = cred.CredentialBlobSize as usize;

    if blob_size != 32 {
        // SAFETY: pcred was allocated by CredReadW.
        unsafe { CredFree(pcred.cast()) };
        return Err(KeyError::InvalidLength(blob_size));
    }

    // SAFETY: CredentialBlob is valid for CredentialBlobSize bytes.
    let raw: [u8; 32] = unsafe {
        std::slice::from_raw_parts(cred.CredentialBlob, 32)
            .try_into()
            .unwrap()
    };

    // SAFETY: pcred was allocated by CredReadW.
    unsafe { CredFree(pcred.cast()) };

    Ok(Some(raw))
}

#[cfg(all(windows, feature = "native-keyring"))]
fn cred_write(app_name: &str, raw: &[u8; 32]) -> Result<(), KeyError> {
    use windows::Win32::Security::Credentials::{
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredWriteW,
    };
    use windows::core::PWSTR;

    let mut target = cred_target(app_name);
    let mut comment: Vec<u16> = "confex encryption key"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let cred = CREDENTIALW {
        Flags: 0,
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(target.as_mut_ptr()),
        Comment: PWSTR(comment.as_mut_ptr()),
        LastWritten: Default::default(),
        CredentialBlobSize: 32,
        // SAFETY: raw lives for the duration of this call; CredWriteW only reads it.
        CredentialBlob: raw.as_ptr().cast_mut(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        AttributeCount: 0,
        Attributes: std::ptr::null_mut(),
        TargetAlias: PWSTR::null(),
        UserName: PWSTR::null(),
    };

    // SAFETY: cred is a fully initialised CREDENTIALW; all pointer fields are valid for
    // the duration of the call (target, comment, raw are on the stack above).
    let ok = unsafe { CredWriteW(&cred, 0) };

    if !ok.as_bool() {
        return Err(KeyError::CredentialManager("CredWriteW failed".into()));
    }

    Ok(())
}

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
impl From<String> for PassphraseUnsealer {
    fn from(passphrase: String) -> Self {
        Self { passphrase }
    }
}

#[cfg(feature = "sealed-key")]
impl From<&str> for PassphraseUnsealer {
    fn from(passphrase: &str) -> Self {
        Self {
            passphrase: passphrase.to_owned(),
        }
    }
}

#[cfg(feature = "sealed-key")]
impl PassphraseUnsealer {
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

        let params = Params::new(65_536, 3, 4, Some(32)).map_err(KeyError::Kdf)?;
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
        let encryptor = Aes256GcmEncryptor::from(&SecretKey(kek));
        let encrypted = encryptor.encrypt(key).map_err(|_| KeyError::InvalidBlob)?;

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
        let encryptor = Aes256GcmEncryptor::from(&SecretKey(kek));
        let plain = encryptor
            .decrypt(ciphertext)
            .map_err(|_| KeyError::InvalidBlob)?;

        let len = plain.len();
        plain.try_into().map_err(|_| KeyError::InvalidLength(len))
    }
}

/// Stores a sealed (encrypted) key blob on disk, using a pluggable [`Unsealer`].
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
        if self.path.exists() {
            let blob = fs::read(&self.path)?;
            let raw = self.unsealer.unseal(&blob)?;
            return Ok(SecretKey(raw));
        }

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

        Ok(SecretKey(raw))
    }
}

/// Platform-native keyring provider, selected at compile time.
///
/// - Linux: `SecretServiceKeyring` (Secret Service via D-Bus, reboot-persistent)
/// - macOS: `MacOsKeychain` (system Keychain, login-persistent)
/// - Windows: `WindowsCredentialManager` (Credential Manager, reboot-persistent)
///
/// Requires the `native-keyring` feature.
#[cfg(all(target_os = "linux", feature = "native-keyring"))]
pub type NativeKeyring = SecretServiceKeyring;

#[cfg(all(target_os = "macos", feature = "native-keyring"))]
pub type NativeKeyring = MacOsKeychain;

#[cfg(all(windows, feature = "native-keyring"))]
pub type NativeKeyring = WindowsCredentialManager;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn file_key_provider_generates_and_persists() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("key");
        let provider = FileKeyProvider::from(path.clone());
        let k1 = provider.key("app").unwrap();
        let k2 = provider.key("app").unwrap();
        assert_eq!(k1.as_bytes(), k2.as_bytes());
    }

    #[cfg(all(target_os = "linux", feature = "native-keyring"))]
    #[test]
    fn secret_service_stores_and_retrieves() {
        let provider = SecretServiceKeyring;
        let app = "confex-test-ss-42";
        let k1 = match provider.key(app) {
            Ok(k) => k,
            Err(KeyError::NoDaemon) => return,
            Err(e) => panic!("{e}"),
        };
        let k2 = provider.key(app).unwrap();
        assert_eq!(k1.as_bytes(), k2.as_bytes());
    }

    #[cfg(feature = "sealed-key")]
    #[test]
    fn passphrase_unsealer_round_trip() {
        let unsealer = PassphraseUnsealer::from("hunter2");
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
        let provider = SealedFileKeyProvider::new(path, PassphraseUnsealer::from("secret"));
        let k1 = provider.key("app").unwrap();
        let k2 = provider.key("app").unwrap();
        assert_eq!(k1.as_bytes(), k2.as_bytes());
    }

    #[cfg(feature = "sealed-key")]
    #[test]
    fn sealed_provider_wrong_passphrase_errors() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("key.sealed");
        let p1 = SealedFileKeyProvider::new(path.clone(), PassphraseUnsealer::from("correct"));
        p1.key("app").unwrap();
        let p2 = SealedFileKeyProvider::new(path, PassphraseUnsealer::from("wrong"));
        assert!(matches!(p2.key("app"), Err(KeyError::InvalidBlob)));
    }
}
