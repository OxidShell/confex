/// Merge from multiple sources in priority order:
/// defaults < file < env vars < CLI args.
///
/// Run: cargo run --example multi_source -- --port 9090
use confex::{CliSource, ConfigLoader, ConfigSchema, ConfigValue, EnvSource, FileSource, TomlFormat};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, ConfigSchema)]
#[config(app_name = "app", version = 1)]
struct Config {
    host: String,
    port: u16,
    debug: bool,
}

fn main() {
    // Parse --key value pairs from argv.
    let cli_pairs = std::env::args()
        .skip(1)
        .collect::<Vec<_>>()
        .chunks(2)
        .filter_map(|chunk| {
            if chunk.len() == 2 {
                let key = chunk[0].trim_start_matches('-').to_string();
                let val: ConfigValue = chunk[1]
                    .parse::<i64>()
                    .map(ConfigValue::Integer)
                    .unwrap_or_else(|_| ConfigValue::String(chunk[1].clone()));
                Some((key, val))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    let config = ConfigLoader::<Config>::default()
        // lowest priority
        .with_source(FileSource::new(vec!["app.toml".into()], TomlFormat))
        // mid priority
        .with_source(EnvSource::new("APP"))
        // highest priority
        .with_source(CliSource::from_pairs(cli_pairs))
        .load()
        .expect("failed to load config");

    println!("host={} port={} debug={}", config.host, config.port, config.debug);
}
