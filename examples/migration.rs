/// Schema migration: rename a field from v1 to v2.
///
/// Run: cargo run --example migration
use confex::{
    ConfigLoader, ConfigSchema, ConfigValue, EnvSource, Migration, MigrationChain, MigrationError,
    rename_key,
};
use serde::{Deserialize, Serialize};

// Current schema (v2): `display_name` replaced the old `nick` field.
#[derive(Debug, Clone, Serialize, Deserialize, ConfigSchema)]
#[config(app_name = "chat", version = 2)]
struct ChatConfig {
    server: String,
    display_name: String,
}

struct V1ToV2;

impl Migration for V1ToV2 {
    fn from_version(&self) -> u32 {
        1
    }
    fn to_version(&self) -> u32 {
        2
    }

    fn migrate(&self, mut value: ConfigValue) -> Result<ConfigValue, MigrationError> {
        rename_key(&mut value, "nick", "display_name");
        Ok(value)
    }
}

fn main() {
    // Simulate old data arriving via env: CHAT_NICK=alice CHAT_SERVER=chat.example.com
    // A real v1 file would have `nick = "alice"` instead of `display_name`.
    let config = ConfigLoader::<ChatConfig>::default()
        .with_source(EnvSource::default())
        .with_migrations(MigrationChain::default().with_step(V1ToV2))
        .load()
        .expect("failed to load config");

    println!(
        "server={} display_name={}",
        config.server, config.display_name
    );
}
