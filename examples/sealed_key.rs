/// Use [`SealedFileKeyProvider`] so the key blob on disk is protected by a passphrase.
/// The passphrase comes from the `CONFEX_MASTER_KEY` env var — CI/CD friendly.
///
/// Run: `CONFEX_MASTER_KEY=secret cargo run --example sealed_key --features sealed-key`
fn main() {
    use std::path::PathBuf;

    use confex::{
        Aes256GcmEncryptor, ConfigLoader, ConfigSchema, EnvSource, KeyProvider, PassphraseUnsealer,
        SealedFileKeyProvider, app_data_dir,
    };
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize, ConfigSchema)]
    #[config(app_name = "app", version = 1)]
    struct AppConfig {
        endpoint: String,
        #[config(encrypted)]
        api_key: String,
    }

    let blob_path = app_data_dir("app")
        .unwrap_or_else(|| PathBuf::from("."))
        .join("keyblob");

    let unsealer =
        PassphraseUnsealer::from_env("CONFEX_MASTER_KEY").expect("CONFEX_MASTER_KEY not set");

    let provider = SealedFileKeyProvider::new(blob_path, unsealer);
    let secret_key = provider.key("app").expect("failed to unseal key");
    let encryptor = Aes256GcmEncryptor::from(&secret_key);

    let config = ConfigLoader::<AppConfig>::default()
        .with_source(EnvSource::from("APP"))
        .with_encryptor(encryptor)
        .load()
        .expect("failed to load config");

    println!("endpoint={} api_key=<redacted>", config.endpoint);
}
