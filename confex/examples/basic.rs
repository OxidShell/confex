use confex::{ConfigLoader, ConfigSchema, EnvSource};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct AppConfig {
    host: String,
    port: u16,
}

impl ConfigSchema for AppConfig {
    const APP_NAME: &'static str = "myapp";
    const SCHEMA_VERSION: u32 = 1;
}

fn main() {
    // MYAPP_HOST=localhost MYAPP_PORT=8080 cargo run --example basic
    let config = ConfigLoader::<AppConfig>::default()
        .with_source(EnvSource::new("MYAPP"))
        .load()
        .expect("failed to load config");

    println!("host={} port={}", config.host, config.port);
}
