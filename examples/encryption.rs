/// Encrypt sensitive fields at rest using AES-256-GCM.
/// The key is stored in a file with 0600 permissions (FileKeyProvider).
///
/// Run: cargo run --example encryption
use std::path::PathBuf;

use confex::{
    Aes256GcmEncryptor, ConfigLoader, ConfigSchema, EnvSource, FileKeyProvider, KeyProvider,
    app_data_dir,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, ConfigSchema)]
#[config(app_name = "vault", version = 1)]
struct VaultConfig {
    endpoint: String,
    #[config(encrypted)]
    token: String,
    #[config(encrypted)]
    secret: String,
}

fn main() {
    // Key lives at ~/.local/share/vault/key (XDG_DATA_HOME on Linux).
    let key_path = app_data_dir("vault")
        .unwrap_or_else(|| PathBuf::from("."))
        .join("key");

    let provider = FileKeyProvider::new(key_path);
    let secret_key = provider.key("vault").expect("failed to load/create key");
    let encryptor = Aes256GcmEncryptor::new(&secret_key);

    let config = ConfigLoader::<VaultConfig>::default()
        .with_source(EnvSource::new("VAULT"))
        .with_encryptor(encryptor)
        .load()
        .expect("failed to load config");

    // `token` and `secret` are decrypted transparently by the loader.
    println!("endpoint={}", config.endpoint);
    println!("token=<{} chars>", config.token.len());
}
