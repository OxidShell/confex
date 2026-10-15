/// Mutate config at runtime via [`WriteGuard`].
/// Changes are visible to all clones of the handle immediately.
///
/// Run: `cargo run --example write_guard`
use confex::{ConfigHandle, ConfigLoader, ConfigSchema, EnvSource};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, ConfigSchema)]
#[config(app_name = "game", version = 1)]
struct GameConfig {
    volume: u8,
    fullscreen: bool,
    username: String,
}

fn main() {
    let config = ConfigLoader::<GameConfig>::default()
        .with_source(EnvSource::from("GAME"))
        .load()
        .expect("failed to load config");

    let handle = ConfigHandle::from(config);
    let reader = handle.clone(); // another "component" reading the same config

    println!(
        "before: volume={} fullscreen={}",
        handle.read().volume,
        handle.read().fullscreen
    );

    // Mutate through WriteGuard — atomically swaps on drop.
    {
        let mut guard = handle.write();
        guard.volume = 80;
        guard.fullscreen = true;
    }

    // All clones see the new value immediately.
    let cfg = reader.read();
    println!(
        "after:  volume={} fullscreen={}",
        cfg.volume, cfg.fullscreen
    );
}
