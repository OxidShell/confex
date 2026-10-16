# confex

Layered configuration library for Rust. Merges multiple sources (files, env vars, CLI args, redb key-value store) into a single typed struct, with optional hot-reload that only re-reads what changed.

## Problem

Most config libraries either:
- Load once and make you restart to pick up changes
- Reload the entire pipeline on every file event (re-reading env vars, re-parsing CLI args, etc.)
- Don't compose sources with different priorities

confex merges sources in priority order, runs schema migrations on stale data, and on change only re-collects the source that actually changed — env overrides stay fixed, the file or redb store gets re-read.

## Features

| Feature | Description |
|---|---|
| `toml` | TOML file source |
| `json` | JSON file source |
| `redb-backend` | redb-backed dconf-like key-value store source (pure Rust, multi-process via `experimental-multiprocess`) |
| `hot-reload` | Live config handle with selective per-source file watcher |
| `async` | Async source trait + `AsyncConfigLoader` |
| `native-keyring` | OS keyring key provider (Linux Secret Service, macOS Keychain, Windows Credential Manager) |
| `sealed-key` | Passphrase-sealed key blob (Argon2id + AES-256-GCM) |

Default features: `toml`, `redb-backend`, `hot-reload`.

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
    .with_source(EnvSource::default())  // MYAPP_HOST, MYAPP_PORT (prefix from app_name)
    .load()?;
```

Sources merge in registration order — last writer wins. `EnvSource` registered after `FileSource` overrides it.

The `app_name` defined in `#[config(...)]` is automatically injected into sources that need it — `EnvSource` derives its prefix from it, `RedbSource` uses it to scope the key-value store. Pass an explicit `EnvSource::from("CUSTOM")` to override the prefix.

## Hot-reload

```rust
use confex::WatchOptions;

// Selective reload: only the file is re-read on change.
// Env var overrides are passive — sampled once at startup, never re-read.
let (handle, _watcher) = ConfigLoader::<Config>::default()
    .with_watched_source(FileSource::new(["config.toml".into()], TomlFormat))
    .with_source(EnvSource::default())
    .watch_auto(WatchOptions::default())?;

// Lock-free read from any thread.
println!("{}", handle.read().host);
```

`_watcher` keeps the background thread alive. Drop it to stop watching.

If the merged result after re-collection is identical to the previous one (e.g. your own process wrote to the file), the in-memory swap is skipped.

## redb backend

A dconf-like key-value store backed by [redb](https://crates.io/crates/redb) in `MultiWriter` concurrency mode. Pure Rust, no C deps, safe for concurrent access from multiple independent OS processes.

```rust
use confex::{RedbSource, RedbStore, ConfigValue};

// RedbStore needs an explicit name for direct write access.
let store = RedbStore::new("prefs.redb", Prefs::APP_NAME);
store.set("theme", &ConfigValue::String("dark".into()))?;

// One-shot — app_name injected automatically from the schema type.
let config = ConfigLoader::<Prefs>::default()
    .with_source(RedbSource::new("prefs.redb"))
    .load()?;

// Live, reacts to writes from other processes.
let (handle, _watcher) = ConfigLoader::<Prefs>::default()
    .with_watched_source(RedbSource::new("prefs.redb"))
    .with_source(EnvSource::default())
    .watch_auto(WatchOptions::default())?;
```

The watcher monitors the containing directory so cross-process writes are picked up immediately. Multi-process support rides on the `experimental-multiprocess` feature of redb (file byte-range locks); as such it is **not reliable on network filesystems** (NFS/SMB) — use a local path.

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

## Key providers

Pick one depending on the trust boundary and whether the machine is headed (desktop/laptop with a keyring) or headless (server/CI).

| Provider | Feature | Platform | Storage |
|---|---|---|---|
| `FileKeyProvider` | (default) | all | `0600` file (random 32 bytes) |
| `SecretServiceKeyring` | `native-keyring` | Linux | freedesktop.org Secret Service (gnome-keyring, kwallet, keepassxc-secret-service) over D-Bus |
| `MacOsKeychain` | `native-keyring` | macOS | login Keychain via `security-framework` |
| `WindowsCredentialManager` | `native-keyring` | Windows | Credential Manager (`CRED_PERSIST_LOCAL_MACHINE`, built on DPAPI) |
| `SealedFileKeyProvider<PassphraseUnsealer>` | `sealed-key` | all | passphrase-sealed blob (Argon2id → AES-256-GCM) |

`NativeKeyring` is a cross-platform type alias that resolves to the right provider for the current target (`SecretServiceKeyring` on Linux, `MacOsKeychain` on macOS, `WindowsCredentialManager` on Windows). Use it when you want one call site that works everywhere.

### Trust boundary — know what `FileKeyProvider` does NOT protect

`FileKeyProvider` is a **baseline against other local users** on the same machine (the `0600` permission bits), not a defence against a compromise of your own session. The key lives in the same `$HOME` as the data it decrypts, so any process running as your user can read both. For genuinely sensitive secrets (session tokens, API keys, credentials you would not want in a stolen disk image), use `native-keyring` on desktops or `sealed-key` with a user-supplied passphrase on headless hosts.

### Divergence from the original design

The native providers landed in forms that differ from the initial plan:

- **Linux** uses the Secret Service D-Bus API, not the kernel keyring (`keyutils`). Kernel keys die with the session and need PAM integration; Secret Service survives logout and is already running on every mainstream desktop.
- **Windows** uses Credential Manager, not raw DPAPI. Credential Manager is built on DPAPI and gives named storage out of the box.
- **macOS** was added after the initial plan.
- `sealed-key` was added as the headless fallback — no keyring daemon, user supplies a passphrase (optionally via `PassphraseUnsealer::from_env`).

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
cargo run --example hot_reload --features hot-reload
cargo run --example redb_store --features redb-backend
cargo run --example multi_source -- --port 9090
cargo run --example migration
cargo run --example encryption
```
