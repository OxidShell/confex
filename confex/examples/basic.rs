use confex::{ConfigLoader, ConfigSchema, EnvSource};
use serde::Deserialize;

#[derive(Debug, Deserialize, ConfigSchema)]
#[config(app_name = "myapp", version = 1)]
struct AppConfig {
    host: String,
    port: u16,
    nickname: Option<String>,
    #[config(merge = "extend")]
    tags: Vec<String>,
}

fn main() {
    // MYAPP_HOST=localhost MYAPP_PORT=8080 cargo run --example basic
    let config = ConfigLoader::<AppConfig>::default()
        .with_source(EnvSource::new("MYAPP"))
        .load()
        .expect("failed to load config");

    println!(
        "host={} port={} nickname={:?} tags={:?}",
        config.host, config.port, config.nickname, config.tags
    );
}
