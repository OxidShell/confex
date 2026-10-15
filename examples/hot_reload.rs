/// Selective hot-reload via `watch_auto`.
///
/// Demonstrates the key behaviour:
/// - `EnvSource` is **passive**: read once at startup, never re-collected.
/// - `FileSource` is **watchable**: only the file is re-read on disk change.
/// - Self-write suppression: if the merged result is unchanged after
///   re-collection, the in-memory swap is skipped entirely.
///
/// Run: `cargo run --example hot_reload`
/// Then edit `hot_reload.toml` while the process is running.
///
/// Run: `cargo run --example hot_reload --features hot-reload`
#[cfg(feature = "hot-reload")]
fn main() {
    use std::{path::PathBuf, thread, time::Duration};

    use confex::{ConfigLoader, ConfigSchema, EnvSource, FileSource, TomlFormat, WatchOptions};
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize, ConfigSchema)]
    #[config(app_name = "server", version = 1)]
    struct ServerConfig {
        host: String,
        port: u16,
        debug: bool,
    }

    let config_path = PathBuf::from("hot_reload.toml");

    if !config_path.exists() {
        std::fs::write(
            &config_path,
            "host = \"127.0.0.1\"\nport = 8080\ndebug = false\n",
        )
        .expect("failed to write starter config");
        println!("created hot_reload.toml — edit it while this runs");
    }

    // Priority order (lowest → highest):
    //   file  →  env vars
    //
    // Only the file triggers reloads. SERVER_HOST / SERVER_PORT override the
    // file once at startup and are never re-sampled from the environment.
    let (handle, _watcher) = ConfigLoader::<ServerConfig>::default()
        .with_watched_source(FileSource::new([config_path.clone()], TomlFormat))
        .with_source(EnvSource::default())
        .watch_auto(WatchOptions::default())
        .expect("failed to start watcher");

    println!("watching {} — Ctrl-C to stop", config_path.display());
    println!("initial : {:?}", *handle.read());

    let mut last_port = handle.read().port;
    loop {
        thread::sleep(Duration::from_millis(200));
        let cfg = handle.read();
        if cfg.port != last_port {
            println!("reloaded: {:?}", *cfg);
            last_port = cfg.port;
        }
    }
}

#[cfg(not(feature = "hot-reload"))]
fn main() {
    eprintln!("Run with: cargo run --example hot_reload --features hot-reload");
}
