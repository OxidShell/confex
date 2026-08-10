# confex

Layered configuration library for Rust. Merges multiple sources (files, env vars, CLI args, SQLite) into a single typed struct, with optional hot-reload that only re-reads what changed.

## Problem

Most config libraries either:
- Load once and make you restart to pick up changes
- Reload the entire pipeline on every file event (re-reading env vars, re-parsing CLI args, etc.)
- Don't compose sources with different priorities

confex merges sources in priority order, runs schema migrations on stale data, and on change only re-collects the source that actually changed — env overrides stay fixed, the file or SQLite store gets re-read.

## Features

| Feature | Description |
|---|---|
| `toml` | TOML file source |
| `json` | JSON file source |
| `sqlite-backend` | SQLite key-value store source |
| `hot-reload` | Live config handle with selective per-source file watcher |
| `async` | Async source trait + `AsyncConfigLoader` |
| `native-keyring` | OS keyring key provider (Linux kernel keyring, macOS Keychain) |
| `sealed-key` | Passphrase-sealed key blob (Argon2id + AES-256-GCM) |

Default features: `toml`, `sqlite-backend`, `hot-reload`.

## Quick start

```toml
[dependencies]
confex = "0.1"
serde = { version = "1", features = ["derive"] }
```

```rust
use confex::{ConfigLoader, ConfigSchema, EnvSource, FileSource, TomlFormat};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, ConfigSchema)]
#[config(app_name = "myapp", version = 1)]
struct Config {
    host: String,
    port: u16,
}

let config = ConfigLoader::<Config>::default()
    .with_source(FileSource::new(["config.toml".into()], TomlFormat))
    .with_source(EnvSource::from("MYAPP"))  // MYAPP_HOST, MYAPP_PORT
    .load()?;
```

Sources merge in registration order — last writer wins. `EnvSource` registered after `FileSource` overrides it.

## Hot-reload

```rust
use confex::WatchOptions;

// Selective reload: only the file is re-read on change.
// Env var overrides are passive — sampled once at startup, never re-read.
let (handle, _watcher) = ConfigLoader::<Config>::default()
    .with_watched_source(FileSource::new(["config.toml".into()], TomlFormat))
    .with_source(EnvSource::from("MYAPP"))
    .watch_auto(WatchOptions::default())?;

// Lock-free read from any thread.
println!("{}", handle.read().host);
```

`_watcher` keeps the background thread alive. Drop it to stop watching.

If the merged result after re-collection is identical to the previous one (e.g. your own process wrote to the file), the in-memory swap is skipped.

## SQLite backend

```rust
use confex::{SqliteSource, SqliteStore, ConfigValue};

let store = SqliteStore::new("prefs.db", "myapp");
store.set("theme", &ConfigValue::String("dark".into()))?;

// One-shot
let config = ConfigLoader::<Prefs>::default()
    .with_source(SqliteSource::new("prefs.db", "myapp"))
    .load()?;

// Live, reacts to writes from other processes
let (handle, _watcher) = ConfigLoader::<Prefs>::default()
    .with_watched_source(SqliteSource::new("prefs.db", "myapp"))
    .with_source(EnvSource::from("PREFS"))
    .watch_auto(WatchOptions::default())?;
```

The watcher monitors the directory (not the file) so WAL writes from other processes are picked up immediately without waiting for a checkpoint.

## Schema migrations

```rust
use confex::{Migration, MigrationChain, MigrationError, rename_key, ConfigValue};

struct V1ToV2;
impl Migration for V1ToV2 {
    fn from_version(&self) -> u32 { 1 }
    fn to_version(&self) -> u32 { 2 }
    fn migrate(&self, mut v: ConfigValue) -> Result<ConfigValue, MigrationError> {
        rename_key(&mut v, "nick", "display_name");
        Ok(v)
    }
}

let config = ConfigLoader::<ChatConfig>::default()
    .with_source(FileSource::new(["chat.toml".into()], TomlFormat))
    .with_migrations(MigrationChain::default().with_step(V1ToV2))
    .load()?;
```

Migrations run automatically when the stored `__schema_version` is below `SCHEMA_VERSION`.

## Encryption at rest

```rust
use confex::{Aes256GcmEncryptor, FileKeyProvider};
use std::path::PathBuf;

let key_provider = FileKeyProvider::from(PathBuf::from(".key"));
let key = key_provider.key("myapp")?;
let enc = Aes256GcmEncryptor::from(&key);

let config = ConfigLoader::<Config>::default()
    .with_source(FileSource::new(["config.toml".into()], TomlFormat))
    .with_encryptor(enc)
    .load()?;
```

Fields listed in `ConfigSchema::encrypted_fields()` are decrypted transparently on load.

## Runtime writes

```rust
let handle = ConfigHandle::from(config);
{
    let mut guard = handle.write();
    guard.volume = 80;
} // atomically swaps on drop, writes back to file
```

## Examples

```
cargo run --example hot_reload
cargo run --example sqlite_store --features sqlite-backend
cargo run --example multi_source -- --port 9090
cargo run --example migration
cargo run --example encryption
```
