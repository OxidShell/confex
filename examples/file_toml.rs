/// Load config from a TOML file, fall back to env vars.
///
/// Run: `cargo run --example file_toml`
use std::path::PathBuf;

use confex::{ConfigHandle, ConfigLoader, ConfigSchema, EnvSource, FileSource, TomlFormat};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, ConfigSchema)]
#[config(app_name = "server", version = 1)]
struct ServerConfig {
    host: String,
    port: u16,
    #[config(merge = "extend")]
    allowed_origins: Vec<String>,
    tls: Option<TlsConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TlsConfig {
    cert: PathBuf,
    key: PathBuf,
}

fn main() {
    let config_path = std::env::var_os("CONFIG_PATH")
        .map_or_else(|| PathBuf::from("server.toml"), PathBuf::from);

    let config = ConfigLoader::<ServerConfig>::default()
        .with_source(FileSource::new(vec![config_path], TomlFormat))
        .with_source(EnvSource::from("SERVER"))
        .load()
        .expect("failed to load config");

    let handle = ConfigHandle::from(config);
    let cfg = handle.read();
    println!("host={} port={}", cfg.host, cfg.port);
    println!("origins={:?}", cfg.allowed_origins);
    if let Some(tls) = &cfg.tls {
        println!("tls cert={}", tls.cert.display());
    }
}
