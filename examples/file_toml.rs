/// Load config from a TOML file, with env var overrides and optional hot-reload.
///
/// Without `hot-reload`: one-shot load via `ConfigLoader::load`.
/// With `hot-reload`: live handle via `ConfigLoader::watch_auto` — only the
/// TOML file is re-read on change; env var overrides are passive (sampled once
/// at startup).
///
/// Run: `cargo run --example file_toml`
/// Run: `cargo run --example file_toml --features hot-reload`
use std::path::PathBuf;

use confex::{ConfigLoader, ConfigSchema, EnvSource, FileSource, TomlFormat};
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

fn config_path() -> PathBuf {
    std::env::var_os("CONFIG_PATH")
        .map_or_else(|| PathBuf::from("server.toml"), PathBuf::from)
}

fn main() {
    let path = config_path();

    // ── one-shot load ─────────────────────────────────────────────────────────
    #[cfg(not(feature = "hot-reload"))]
    {
        let config = ConfigLoader::<ServerConfig>::default()
            .with_source(FileSource::new([path], TomlFormat))
            .with_source(EnvSource::default())
            .load()
            .expect("failed to load config");

        print_config(&config);
    }

    // ── selective hot-reload ──────────────────────────────────────────────────
    // `with_watched_source` registers the file for selective reload.
    // `with_source` keeps env vars passive (sampled once, never re-read).
    // On each file change only the file source is re-collected; the cached
    // env snapshot is merged on top without touching the environment again.
    #[cfg(feature = "hot-reload")]
    {
        use confex::WatchOptions;

        let (handle, _watcher) = ConfigLoader::<ServerConfig>::default()
            .with_watched_source(FileSource::new([path.clone()], TomlFormat))
            .with_source(EnvSource::default())
            .watch_auto(WatchOptions::default())
            .expect("failed to start watcher");

        println!("watching {} — edit it to see a reload", path.display());
        print_config(&handle.read());

        // Keep running so the watcher stays alive.
        std::thread::park();
    }
}

fn print_config(cfg: &ServerConfig) {
    println!("host={} port={}", cfg.host, cfg.port);
    println!("origins={:?}", cfg.allowed_origins);
    if let Some(tls) = &cfg.tls {
        println!("tls cert={}", tls.cert.display());
    }
}
