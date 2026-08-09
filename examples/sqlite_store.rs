/// Persist and read config from the SQLite dconf-like backend.
///
/// Run: cargo run --example sqlite_store --features sqlite-backend
#[cfg(feature = "sqlite-backend")]
fn main() {
    use std::path::PathBuf;

    use confex::{
        ConfigLoader, ConfigSchema, EnvSource, SqliteSource, SqliteStore, app_data_dir,
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

    use confex::ConfigValue;

    // Write some preferences.
    let store = SqliteStore::new(&db_path, "prefs");
    store.set("theme", &ConfigValue::String("dark".into())).expect("set failed");
    store.set("font_size", &ConfigValue::Integer(14)).expect("set failed");

    // Read them back via SqliteSource (acts as a Source in the merge chain).
    let config = ConfigLoader::<Prefs>::default()
        .with_source(SqliteSource::new(&db_path, "prefs"))
        .with_source(EnvSource::new("PREFS"))
        .load()
        .expect("failed to load config");

    println!("theme={} font_size={}", config.theme, config.font_size);
    println!("recent_files={:?}", config.recent_files);
}

#[cfg(not(feature = "sqlite-backend"))]
fn main() {
    eprintln!("Run with: cargo run --example sqlite_store --features sqlite-backend");
}
