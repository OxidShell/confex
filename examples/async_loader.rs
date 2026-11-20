//! Load config from an async source (e.g. a remote API or secrets manager).
//!
//! Sync sources automatically implement [`AsyncSource`] via blanket impl,
//! so they can be mixed freely with async ones.
//!
//! Run: `cargo run --example async_loader --features async`

use std::collections::BTreeMap;

use confex::{
    AsyncConfigLoader, AsyncSource, CollectFuture, ConfigSchema, ConfigValue, EnvSource,
    SourceError,
};
use serde::{Deserialize, Serialize};

// ── schema ────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, ConfigSchema)]
#[config(app_name = "game", version = 1)]
pub struct GameConfig {
    pub server_addr: String,
    pub max_players: u32,
    pub debug: bool,
}

// ── custom async source ───────────────────────────────────────────────────

/// Simulates a remote config fetch (secrets manager, etcd, Consul, …).
///
/// In production this would use `reqwest`, `aws-sdk-secretsmanager`, etc.
pub struct RemoteSource {
    endpoint: String,
}

impl From<&str> for RemoteSource {
    fn from(endpoint: &str) -> Self {
        Self {
            endpoint: endpoint.to_owned(),
        }
    }
}

impl RemoteSource {
    async fn fetch(&self) -> Result<ConfigValue, SourceError> {
        // In production: reqwest::get(&self.endpoint).await?
        eprintln!("fetching remote config from {}", self.endpoint);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let mut table = BTreeMap::new();
        table.insert(
            "server_addr".to_owned(),
            ConfigValue::String("0.0.0.0:8080".to_owned()),
        );
        table.insert("max_players".to_owned(), ConfigValue::Integer(64));
        table.insert("debug".to_owned(), ConfigValue::Bool(false));
        Ok(ConfigValue::Table(table))
    }
}

impl AsyncSource for RemoteSource {
    fn collect(&self) -> CollectFuture<'_> {
        Box::pin(self.fetch())
    }
}

// ── entry point ───────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    // EnvSource is sync — the blanket impl wraps it transparently.
    // GAME_DEBUG=true overrides the remote value.
    let config = AsyncConfigLoader::<GameConfig>::default()
        .with_source(RemoteSource::from("https://config.example.com/game"))
        .with_source(EnvSource::from("GAME"))
        .load()
        .await
        .expect("failed to load config");

    println!("server_addr : {}", config.server_addr);
    println!("max_players : {}", config.max_players);
    println!("debug       : {}", config.debug);
}
