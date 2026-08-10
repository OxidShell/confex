/// Persist and read config from the `SQLite` dconf-like backend.
///
/// Without `hot-reload`: one-shot load via `ConfigLoader`.
/// With `hot-reload`: two options:
/// - `SqliteSource::watch` — pure `SQLite` watcher, no other sources.
/// - `ConfigLoader::watch_auto` — selective reload; env var overrides are
///   passive (sampled once at startup, never re-read on `SQLite` change).
///
/// Run: `cargo run --example sqlite_store --features sqlite-backend`
/// Run: `cargo run --example sqlite_store --features sqlite-backend,hot-reload`
#[cfg(feature = "sqlite-backend")]
fn main() {
    use std::path::PathBuf;

    use confex::{
        ConfigLoader, ConfigSchema, ConfigValue, EnvSource, SqliteSource, SqliteStore, app_data_dir,
    };
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize, ConfigSchema)]
    #[config(app_name = "prefs", version = 1)]
    struct Prefs {
        theme: String,
        font_size: u32,
        #[config(merge = "extend")]
        recent_files: Vec<String>,
    }

    let db_path = app_data_dir("prefs")
        .unwrap_or_else(|| PathBuf::from("."))
        .join("prefs.db");

    // Write some preferences via the store handle.
    let store = SqliteStore::new(&db_path, "prefs");
    store.set("theme", &ConfigValue::String("dark".into())).expect("set failed");
    store.set("font_size", &ConfigValue::Integer(14)).expect("set failed");

    // ── One-shot load (always available) ─────────────────────────────────────
    // Merges SQLite source with env var overrides (PREFS_THEME, PREFS_FONT_SIZE…).
    let config = ConfigLoader::<Prefs>::default()
        .with_source(SqliteSource::new(&db_path, "prefs"))
        .with_source(EnvSource::from("PREFS"))
        .load()
        .expect("failed to load config");

    println!("one-shot — theme={} font_size={}", config.theme, config.font_size);

    // ── Live handle (hot-reload feature) ─────────────────────────────────────
    #[cfg(feature = "hot-reload")]
    {
        use confex::WatchOptions;

        // Option A: SqliteSource::watch — simplest, pure SQLite watcher
        let source = SqliteSource::new(&db_path, "prefs");
        let (handle_a, _watcher_a) = source
            .watch::<Prefs>(WatchOptions::default())
            .expect("failed to start sqlite watcher");
        println!("live (sqlite) — theme={}", handle_a.read().theme);

        // Option B: selective reload — env vars are passive, only SQLite triggers reload
        let (handle_b, _watcher_b) = ConfigLoader::<Prefs>::default()
            .with_source(EnvSource::from("PREFS"))
            .with_watched_source(SqliteSource::new(&db_path, "prefs"))
            .watch_auto(WatchOptions::default())
            .expect("failed to start selective watcher");
        println!("live (selective) — theme={}", handle_b.read().theme);
    }
}

#[cfg(not(feature = "sqlite-backend"))]
fn main() {
    eprintln!("Run with: cargo run --example sqlite_store --features sqlite-backend");
}
