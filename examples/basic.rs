use confex::{ConfigHandle, ConfigLoader, ConfigSchema, EnvSource};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, ConfigSchema)]
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
        .with_source(EnvSource::from("MYAPP"))
        .load()
        .expect("failed to load config");

    let handle = ConfigHandle::from(config);
    let cfg = handle.read();
    println!(
        "host={} port={} nickname={:?} tags={:?}",
        cfg.host, cfg.port, cfg.nickname, cfg.tags
    );
}
