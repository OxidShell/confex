/// Layered config: defaults < file < env vars < CLI args.
///
/// Without `hot-reload`: one-shot load.
/// With `hot-reload`: only the TOML file triggers selective reload; env vars
/// and CLI args are passive (fixed at startup, never re-sampled).
///
/// Run: `cargo run --example multi_source -- --port 9090`
/// Run: `cargo run --example multi_source --features hot-reload -- --port 9090`
use confex::{CliSource, ConfigLoader, ConfigSchema, ConfigValue, EnvSource, FileSource, TomlFormat};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, ConfigSchema)]
#[config(app_name = "app", version = 1)]
struct Config {
    host: String,
    port: u16,
    debug: bool,
}

fn cli_source() -> CliSource {
    std::env::args()
        .skip(1)
        .collect::<Vec<_>>()
        .chunks(2)
        .filter_map(|chunk| {
            if chunk.len() == 2 {
                let key = chunk[0].trim_start_matches('-').to_string();
                let val: ConfigValue = chunk[1]
                    .parse::<i64>()
                    .map_or_else(|_| ConfigValue::String(chunk[1].clone()), ConfigValue::Integer);
                Some((key, val))
            } else {
                None
            }
        })
        .collect()
}

fn main() {
    // ── one-shot load ─────────────────────────────────────────────────────────
    #[cfg(not(feature = "hot-reload"))]
    {
        let config = ConfigLoader::<Config>::default()
            .with_source(FileSource::new(["app.toml".into()], TomlFormat)) // lowest priority
            .with_source(EnvSource::from("APP"))
            .with_source(cli_source()) // highest priority
            .load()
            .expect("failed to load config");

        println!("host={} port={} debug={}", config.host, config.port, config.debug);
    }

    // ── selective hot-reload ──────────────────────────────────────────────────
    // The file is watchable: `app.toml` changes trigger a selective re-read of
    // only that source. Env vars and CLI args remain fixed after startup — they
    // override whatever the file says, but are never re-sampled on reload.
    #[cfg(feature = "hot-reload")]
    {
        use confex::WatchOptions;

        let (handle, _watcher) = ConfigLoader::<Config>::default()
            .with_watched_source(FileSource::new(["app.toml".into()], TomlFormat))
            .with_source(EnvSource::from("APP"))
            .with_source(cli_source())
            .watch_auto(WatchOptions::default())
            .expect("failed to start watcher");

        println!("initial: host={} port={} debug={}", handle.read().host, handle.read().port, handle.read().debug);
        println!("watching app.toml — edit it to trigger a selective reload");

        std::thread::park();
    }
}
