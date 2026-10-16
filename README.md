# confex

Layered configuration library for Rust. Merges multiple sources (files, env vars, CLI args, redb key-value store) into a single typed struct, with optional hot-reload that only re-reads what changed.

## Problem

Most config libraries either:
- Load once and make you restart to pick up changes
- Reload the entire pipeline on every file event (re-reading env vars, re-parsing CLI args, etc.)
- Don't compose sources with different priorities

confex merges sources in priority order, runs schema migrations on stale data, and on change only re-collects the source that actually changed — env overrides stay fixed, the file or redb store gets re-read.

## Two use cases

confex covers two shapes of persistent state. Pick the shape before you pick an API:

**A. Layered config (merge + hot-reload).** User-facing settings that come from several places and need to be composed: TOML/JSON files, env vars, CLI args, optionally a dconf-like key-value store for admin overrides. Multiple sources, one typed struct, hot-reload if you want it. Field-level encryption for the one or two secret fields in the mix. → [Layered config](#layered-config)

**B. App data (single source of truth).** State the application owns and writes to itself: session tokens, cached auth, "recent files" lists, window geometry. One backing store, no merging, usually opaque on disk. Written by the app, not hand-edited. → [App data](#app-data)

Both share the same `#[derive(ConfigSchema)]`, `ConfigValue` tree, and migration machinery. The difference is which sources you register and whether you reach for `EncryptedFormat<F>` (whole-file crypto) or `#[config(encrypted)]` (per-field crypto inside a merged struct).

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
| `istmo` | Delegate path resolution to `istmo-core` (adds Android/iOS targets) |

Default features: `toml`, `redb-backend`, `hot-reload`.

```toml
[dependencies]
confex = "0.1"
serde = { version = "1", features = ["derive"] }
```

---

# Layered config

User-editable settings merged from multiple sources into one typed struct.

## Quick start

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

## redb-backed override

A dconf-like key-value store slotted into the merge chain, typically between file and env. Pure Rust, no C deps, safe across OS processes.

```rust
use confex::{RedbSource, RedbStore, ConfigValue};

// Admin writes an override directly to the store.
RedbStore::new("overrides.redb", Config::APP_NAME)
    .set("host", &ConfigValue::String("0.0.0.0".into()))?;

let config = ConfigLoader::<Config>::default()
    .with_source(FileSource::new(["config.toml".into()], TomlFormat))
    .with_source(RedbSource::new("overrides.redb"))   // overrides the file
    .with_source(EnvSource::default())                // overrides the store
    .load()?;
```

With `hot-reload` enabled, use `with_watched_source(RedbSource::new(...))` to react to writes from other processes without polling. The watcher monitors the containing directory, so cross-process writes land immediately.

Multi-process support rides on redb's `experimental-multiprocess` feature (file byte-range locks) — **not reliable on network filesystems** (NFS/SMB). Use a local path.

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

## Field-level encryption

Use when the config is mostly plaintext but a couple of fields are secret (API token, OAuth refresh token). The file stays human-readable; only the marked fields are ciphertext.

```rust
#[derive(Debug, Clone, Serialize, Deserialize, ConfigSchema)]
#[config(app_name = "vault", version = 1)]
struct VaultConfig {
    endpoint: String,
    #[config(encrypted)]
    token: String,
    #[config(encrypted)]
    secret: String,
}

use confex::{Aes256GcmEncryptor, FileKeyProvider, KeyProvider};
use std::path::PathBuf;

let key_provider = FileKeyProvider::from(PathBuf::from(".key"));
let key = key_provider.key("vault")?;
let enc = Aes256GcmEncryptor::from(&key);

let config = ConfigLoader::<VaultConfig>::default()
    .with_source(FileSource::new(["vault.toml".into()], TomlFormat))
    .with_encryptor(enc)
    .load()?;
```

Fields listed in `ConfigSchema::encrypted_fields()` (generated from `#[config(encrypted)]`) are decrypted transparently on load.

For whole-file encryption instead of per-field, see [App data → encrypted file](#encrypted-file).

## Runtime writes

```rust
let handle = ConfigHandle::from(config);
{
    let mut guard = handle.write();
    guard.volume = 80;
} // atomically swaps on drop, writes back to file
```

---

# App data

State the app owns. Single source, no merging, often opaque on disk. The app writes it; the user does not hand-edit it. Hot-reload is usually unnecessary here — the app knows when it changed something.

## Encrypted file

`EncryptedFormat<F>` wraps any `Format` so the whole serialized payload is encrypted at rest. The file is unreadable without the key. Default choice for session blobs, cached credentials, or anything that would be awkward lying around in plaintext.

```rust
use confex::{
    Aes256GcmEncryptor, ConfigLoader, ConfigSchema, EncryptedFormat,
    FileSource, KeyProvider, NativeKeyring, TomlFormat, app_data_dir,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, ConfigSchema)]
#[config(app_name = "myapp-session", version = 1)]
struct Session {
    user_id: String,
    refresh_token: String,
    expires_at: i64,
}

let key = NativeKeyring.key(Session::APP_NAME)?;
let enc = Aes256GcmEncryptor::from(&key);
let fmt = EncryptedFormat::new(TomlFormat, &enc);

let path = app_data_dir(Session::APP_NAME).unwrap().join("session.enc");

let session: Session = ConfigLoader::<Session>::default()
    .with_source(FileSource::new([path].to_vec(), fmt))
    .load()?;
```

Do **not** add `EnvSource` or CLI sources to an app-data loader — those are user-facing override channels and have no place in app-owned state.

## redb-backed app data

For state that changes incrementally (recent-files list, UI counters, per-key settings), reach for `RedbStore` directly. One `app_name` namespace in the shared `confex_kv` table; each `set` is an atomic redb transaction and safe across processes.

```rust
use confex::{RedbStore, ConfigValue, app_data_dir};

let store = RedbStore::new(
    app_data_dir("myapp").unwrap().join("state.redb"),
    "myapp",
);

store.set("recent_files.0", &ConfigValue::String("/path/to/doc.md".into()))?;
store.set("window.width", &ConfigValue::Integer(1920))?;
```

Not encrypted at rest. If the data is sensitive, use `EncryptedFormat` on a file instead, or wrap the values with field-level encryption before writing (same `Aes256GcmEncryptor::encrypt` → store a hex-encoded ciphertext).

## Headless secrets (no desktop keyring)

For servers or CI where no Secret Service / Keychain / Credential Manager is running, use `SealedFileKeyProvider` with `PassphraseUnsealer`. The key is sealed with a user-supplied passphrase via Argon2id + AES-256-GCM; nothing on disk is useful without the passphrase.

```rust
use confex::{PassphraseUnsealer, SealedFileKeyProvider};

let unsealer = PassphraseUnsealer::from_env("MYAPP_PASSPHRASE")?;
let provider = SealedFileKeyProvider::new(
    app_data_dir("myapp").unwrap().join("key.sealed"),
    unsealer,
);
```

See the `sealed_key` example for a full flow.

---

## Mobile targets (`istmo` feature)

Enable the `istmo` feature to delegate path resolution to [`istmo-core`](https://docs.rs/istmo-core), which adds Android (`Context.filesDir`, `cacheDir`) and iOS (`NSFileManager`) on top of the three desktop OSes confex covers natively.

```toml
[dependencies]
confex = { version = "0.1", features = ["istmo"] }
```

In this mode `app_config_dir` / `app_data_dir` ignore their `app_name` argument — istmo already resolved the app identity at build time from `ISTMO_APP_BUNDLE_ID` (baked by `istmo-build::emit_app`). Two extra helpers show up: `app_cache_dir` and `app_state_dir`. Everything else — merge, hot-reload, redb, encryption — works unchanged.

On Android, make sure the istmo transport has called `istmo_core::path::install(...)` before confex touches any path (it does so automatically in a normal `istmo::runtime!` setup).

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

## Examples

Grouped by the use case they illustrate.

**Layered config:**

```
cargo run --example file_toml                            # single-source TOML
cargo run --example multi_source -- --port 9090          # file + env + CLI merge
cargo run --example hot_reload --features hot-reload     # selective per-source reload
cargo run --example migration                            # schema migration
cargo run --example encryption                           # field-level #[config(encrypted)]
cargo run --example redb_store --features redb-backend   # redb override in the merge chain
cargo run --example write_guard                          # ConfigHandle.write() → file write-back
```

**App data:**

```
cargo run --example sealed_key --features sealed-key     # passphrase-sealed key for headless hosts
```
