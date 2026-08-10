use std::{
    ops::{Deref, DerefMut},
    path::PathBuf,
    sync::Arc,
};

use arc_swap::ArcSwap;
use serde::Serialize;

use crate::{
    config::ConfigSchema,
    deserialize::to_config_value,
    format::Format,
    value::ConfigValue,
};

#[cfg(feature = "hot-reload")]
use crate::error::LoadError;

// ── errors ────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum HotReloadError {
    Watch(String),
    Serialize(String),
    Io(std::io::Error),
}

impl std::fmt::Display for HotReloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Watch(msg) => write!(f, "failed to set up file watcher: {msg}"),
            Self::Serialize(msg) => write!(f, "failed to serialize config: {msg}"),
            Self::Io(e) => write!(f, "I/O error persisting config: {e}"),
        }
    }
}

impl std::error::Error for HotReloadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for HotReloadError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

// ── write-back helper ─────────────────────────────────────────────────────────

struct FileWriteBack {
    path: PathBuf,
    format: Arc<dyn Format + Send + Sync>,
}

impl FileWriteBack {
    fn persist(&self, value: &ConfigValue) -> Result<(), HotReloadError> {
        let bytes = self
            .format
            .serialize(value)
            .map_err(|e| HotReloadError::Serialize(e.to_string()))?;
        std::fs::write(&self.path, bytes)?;
        Ok(())
    }
}

// ── watch options ─────────────────────────────────────────────────────────────

/// Tuning options for the file watcher.
#[derive(Debug, Clone, Copy)]
pub struct WatchOptions {
    /// Quiet period after the last file event before reloading. Default: 200 ms.
    pub debounce: std::time::Duration,
}

impl Default for WatchOptions {
    fn default() -> Self {
        Self {
            debounce: std::time::Duration::from_millis(200),
        }
    }
}

// ── handle ────────────────────────────────────────────────────────────────────

/// Live, cloneable handle to a loaded config value.
///
/// Reads are lock-free via `ArcSwap`. Writes go through [`WriteGuard`], which
/// atomically updates the in-memory state on drop and optionally persists to
/// the config file (watcher mode only).
///
/// Clone the handle freely — all clones share the same live value.
pub struct ConfigHandle<T> {
    swap: Arc<ArcSwap<T>>,
    write_back: Option<Arc<FileWriteBack>>,
}

impl<T> Clone for ConfigHandle<T> {
    fn clone(&self) -> Self {
        Self {
            swap: Arc::clone(&self.swap),
            write_back: self.write_back.clone(),
        }
    }
}

impl<T: ConfigSchema> ConfigHandle<T> {
    /// Manual mode: no file watcher, no autosave. The handle is the sole source of truth.
    #[must_use]
    pub fn new(config: T) -> Self {
        Self {
            swap: Arc::new(ArcSwap::from_pointee(config)),
            write_back: None,
        }
    }

    /// Lock-free snapshot of the current config. Equivalent to `Arc::clone` cost.
    #[must_use]
    pub fn read(&self) -> arc_swap::Guard<Arc<T>> {
        self.swap.load()
    }
}

impl<T: ConfigSchema + Clone + Serialize> ConfigHandle<T> {
    /// Return a write guard. On drop the guard:
    ///
    /// 1. Serializes the modified value and writes it to the config file (watcher mode).
    /// 2. Atomically swaps it into the `ArcSwap` so all readers see the new value.
    ///
    /// In manual mode only step 2 happens (no file path is stored).
    #[must_use]
    pub fn write(&self) -> WriteGuard<T> {
        let current = self.swap.load();
        WriteGuard {
            value: (**current).clone(),
            swap: Arc::clone(&self.swap),
            write_back: self.write_back.clone(),
        }
    }
}

// ── watcher mode ─────────────────────────────────────────────────────────────

/// Keeps the file watcher alive. Drop to stop watching.
#[cfg(feature = "hot-reload")]
pub struct WatcherHandle {
    _watcher: notify::RecommendedWatcher,
}

#[cfg(feature = "hot-reload")]
impl<T: ConfigSchema + Clone + Send + Sync + 'static> ConfigHandle<T> {
    /// Watcher mode: watches `path` for external changes and reloads atomically.
    ///
    /// `reload` is called on every detected change after the debounce window.
    /// If it fails the previous config is kept and the error is logged via `tracing`.
    ///
    /// # Errors
    ///
    /// Returns [`HotReloadError::Watch`] if the file watcher cannot be initialised.
    pub fn watch(
        initial: T,
        path: PathBuf,
        format: impl Format + Send + Sync + 'static,
        reload: impl Fn() -> Result<T, LoadError> + Send + Sync + 'static,
        options: WatchOptions,
    ) -> Result<(Self, WatcherHandle), HotReloadError> {
        use notify::{Config, RecursiveMode, Watcher as _};
        use std::sync::mpsc;

        let swap = Arc::new(ArcSwap::from_pointee(initial));
        let swap_bg = Arc::clone(&swap);
        let reload: Arc<dyn Fn() -> Result<T, LoadError> + Send + Sync> = Arc::new(reload);
        let reload_bg = Arc::clone(&reload);
        let debounce = options.debounce;

        let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
        let mut watcher =
            notify::RecommendedWatcher::new(move |ev| { let _ = tx.send(ev); }, Config::default())
                .map_err(|e| HotReloadError::Watch(e.to_string()))?;

        // Borrow before moving path into FileWriteBack.
        watcher
            .watch(&path, RecursiveMode::NonRecursive)
            .map_err(|e| HotReloadError::Watch(e.to_string()))?;

        let write_back = Arc::new(FileWriteBack {
            path,
            format: Arc::new(format),
        });

        std::thread::Builder::new()
            .name("confex-watcher".into())
            .spawn(move || watcher_loop(&rx, &swap_bg, &reload_bg, debounce))
            .map_err(HotReloadError::Io)?;

        Ok((
            Self {
                swap,
                write_back: Some(write_back),
            },
            WatcherHandle {
                _watcher: watcher,
            },
        ))
    }
}

#[cfg(feature = "hot-reload")]
fn watcher_loop<T: Send + Sync + 'static>(
    rx: &std::sync::mpsc::Receiver<notify::Result<notify::Event>>,
    swap: &Arc<ArcSwap<T>>,
    reload: &Arc<dyn Fn() -> Result<T, LoadError> + Send + Sync>,
    debounce: std::time::Duration,
) {
    use std::sync::mpsc::RecvTimeoutError;

    loop {
        // Block until the first event for this change burst.
        match rx.recv() {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => { tracing::warn!("watcher error: {e}"); continue; }
            Err(_) => break,
        }
        // Drain additional events within the debounce window.
        loop {
            match rx.recv_timeout(debounce) {
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        // Reload.
        match reload() {
            Ok(cfg) => {
                swap.store(Arc::new(cfg));
                tracing::debug!("config hot-reloaded");
            }
            Err(e) => {
                tracing::warn!("hot-reload failed, keeping previous config: {e}");
            }
        }
    }
}

// ── write guard ───────────────────────────────────────────────────────────────

/// Holds a mutable snapshot of `T`. On drop:
///
/// 1. If a file path is configured, serializes and writes to disk.
/// 2. Atomically swaps the new value into the live handle.
///
/// Disk errors are logged via `tracing::error!`; the in-memory swap always happens.
pub struct WriteGuard<T: ConfigSchema + Clone + Serialize> {
    value: T,
    swap: Arc<ArcSwap<T>>,
    write_back: Option<Arc<FileWriteBack>>,
}

impl<T: ConfigSchema + Clone + Serialize> Deref for WriteGuard<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}

impl<T: ConfigSchema + Clone + Serialize> DerefMut for WriteGuard<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.value
    }
}

impl<T: ConfigSchema + Clone + Serialize> Drop for WriteGuard<T> {
    fn drop(&mut self) {
        if let Some(wb) = &self.write_back {
            match to_config_value(&self.value) {
                Ok(cv) => {
                    if let Err(e) = wb.persist(&cv) {
                        tracing::error!("failed to write config to disk: {e}");
                    }
                }
                Err(e) => tracing::error!("failed to serialize config for disk write: {e}"),
            }
        }
        self.swap.store(Arc::new(self.value.clone()));
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    use super::*;
    use crate::{config::ConfigSchema, merge::FieldStrategies};

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct Cfg {
        value: i64,
    }

    impl ConfigSchema for Cfg {
        const APP_NAME: &'static str = "test";
        const SCHEMA_VERSION: u32 = 1;
        fn field_strategies() -> FieldStrategies { &[] }
        fn encrypted_fields() -> &'static [&'static str] { &[] }
    }

    #[test]
    fn read_returns_initial_value() {
        let h = ConfigHandle::new(Cfg { value: 42 });
        assert_eq!(h.read().value, 42);
    }

    #[test]
    fn write_guard_updates_in_memory_on_drop() {
        let h = ConfigHandle::new(Cfg { value: 1 });
        {
            let mut g = h.write();
            (*g).value = 99;
        }
        assert_eq!(h.read().value, 99);
    }

    #[test]
    fn cloned_handle_sees_write() {
        let h = ConfigHandle::new(Cfg { value: 0 });
        let h2 = h.clone();
        {
            let mut g = h.write();
            (*g).value = 7;
        }
        assert_eq!(h2.read().value, 7);
    }

    #[cfg(feature = "hot-reload")]
    #[test]
    fn watcher_reloads_on_file_change() {
        use crate::{
            format::TomlFormat,
            source::{FileSource, Source},
        };
        use std::{thread, time::Duration};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "value = 1\n").unwrap();

        let path_clone = path.clone();
        let (handle, _watcher) = ConfigHandle::watch(
            Cfg { value: 1 },
            path.clone(),
            TomlFormat,
            move || {
                let src = FileSource::new(vec![path_clone.clone()], TomlFormat);
                let cv = src.collect()?;
                Ok(serde::de::Deserialize::deserialize(crate::deserialize::ConfigValueDeserializer(cv))?)
            },
            WatchOptions::default(),
        )
        .unwrap();

        assert_eq!(handle.read().value, 1);

        std::fs::write(&path, "value = 42\n").unwrap();
        // Wait for debounce + watcher to fire.
        thread::sleep(Duration::from_millis(600));

        assert_eq!(handle.read().value, 42);
    }
}
