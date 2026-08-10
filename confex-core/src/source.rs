use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use crate::{error::SourceError, format::Format, value::ConfigValue};

/// A source that can produce a `ConfigValue` tree.
pub trait Source {
    /// Collect config values from this source.
    ///
    /// # Errors
    /// Returns [`SourceError`] on I/O, parse, or environment variable errors.
    fn collect(&self) -> Result<ConfigValue, SourceError>;

    /// Inject the application name derived from [`crate::config::ConfigSchema::APP_NAME`].
    ///
    /// Called automatically by [`crate::config::ConfigLoader::with_source`] and
    /// [`crate::config::ConfigLoader::with_watched_source`] before the source is
    /// stored. The default is a no-op; override it for sources that use the app
    /// name to scope their data (e.g. `SqliteSource`, `EnvSource`). The name
    /// must not override an explicit value already set on the source.
    #[must_use]
    fn with_app_name(self, _name: &str) -> Self
    where
        Self: Sized,
    {
        self
    }
}

/// Reads config from environment variables with a given prefix.
///
/// `PREFIX_WINDOW__WIDTH=1024` → `{ "window": { "width": 1024 } }`
///
/// Double-underscore (`__`) is the nested key separator.
#[derive(Default)]
pub struct EnvSource {
    prefix: String,
}

impl From<String> for EnvSource {
    fn from(prefix: String) -> Self {
        Self { prefix }
    }
}

impl From<&str> for EnvSource {
    fn from(prefix: &str) -> Self {
        Self {
            prefix: prefix.to_owned(),
        }
    }
}

impl Source for EnvSource {
    fn with_app_name(mut self, name: &str) -> Self {
        if self.prefix.is_empty() {
            name.clone_into(&mut self.prefix);
        }
        self
    }

    fn collect(&self) -> Result<ConfigValue, SourceError> {
        if self.prefix.is_empty() {
            return Ok(ConfigValue::Table(BTreeMap::new()));
        }
        let prefix = format!("{}_", self.prefix.to_uppercase());
        let mut table = BTreeMap::new();

        for (key, val) in std::env::vars() {
            let Some(stripped) = key.strip_prefix(&prefix) else {
                continue;
            };
            let path: Vec<String> = stripped
                .to_lowercase()
                .split("__")
                .map(String::from)
                .collect();
            insert_nested(&mut table, &path, val.parse().unwrap());
        }

        Ok(ConfigValue::Table(table))
    }
}

fn insert_nested(table: &mut BTreeMap<String, ConfigValue>, path: &[String], value: ConfigValue) {
    let Some((head, tail)) = path.split_first() else {
        return;
    };
    if tail.is_empty() {
        table.insert(head.clone(), value);
        return;
    }
    let entry = table
        .entry(head.clone())
        .or_insert_with(|| ConfigValue::Table(BTreeMap::new()));
    if let ConfigValue::Table(inner) = entry {
        insert_nested(inner, tail, value);
    }
}

/// Source backed by `(key, value)` pairs — parser-agnostic CLI bridge.
///
/// Any CLI parser (clap, pico-args, raw argv) produces `(String, ConfigValue)` pairs;
/// wrap them here to plug into `ConfigLoader`.
pub struct CliSource {
    values: BTreeMap<String, ConfigValue>,
}

impl FromIterator<(String, ConfigValue)> for CliSource {
    fn from_iter<I: IntoIterator<Item = (String, ConfigValue)>>(iter: I) -> Self {
        Self {
            values: iter.into_iter().collect(),
        }
    }
}

impl Source for CliSource {
    fn collect(&self) -> Result<ConfigValue, SourceError> {
        Ok(ConfigValue::Table(self.values.clone()))
    }
}

/// Reads config from the first file in `candidates` that exists on disk.
pub struct FileSource<F: Format> {
    candidates: Vec<PathBuf>,
    format: F,
}

impl<F: Format> FileSource<F> {
    #[must_use]
    pub fn new(candidates: impl IntoIterator<Item = PathBuf>, format: F) -> Self {
        Self {
            candidates: candidates.into_iter().collect(),
            format,
        }
    }

    /// Returns the first candidate path that exists, or `None`.
    #[must_use]
    pub fn resolve(&self) -> Option<&Path> {
        self.candidates
            .iter()
            .find(|p| p.exists())
            .map(PathBuf::as_path)
    }
}

impl<F: Format> Source for FileSource<F> {
    fn collect(&self) -> Result<ConfigValue, SourceError> {
        let Some(path) = self.resolve() else {
            return Ok(ConfigValue::Table(BTreeMap::new()));
        };
        let bytes = std::fs::read(path).map_err(|e| SourceError::Io {
            path: path.to_owned(),
            source: e,
        })?;
        self.format.parse(path, &bytes)
    }
}

/// A [`Source`] that can declare which filesystem paths it monitors.
///
/// Implemented by [`FileSource`] and [`SqliteSource`]. Passive sources (env
/// vars, CLI args) do not implement this — their values are fixed at load time
/// and never need a reload trigger.
#[cfg(feature = "hot-reload")]
pub trait WatchableSource: Source + Send + Sync {
    /// Filesystem paths (files or directories) whose changes should trigger a
    /// reload of this source. An empty vec means "not currently watchable"
    /// (e.g. no candidate file exists yet).
    fn watch_paths(&self) -> Vec<std::path::PathBuf>;
}

#[cfg(feature = "hot-reload")]
impl<F: Format + Send + Sync> WatchableSource for FileSource<F> {
    fn watch_paths(&self) -> Vec<std::path::PathBuf> {
        self.candidates.clone()
    }
}

/// Owned, boxed future returned by [`AsyncSource::collect`].
#[cfg(feature = "async")]
pub type CollectFuture<'a> = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<ConfigValue, SourceError>> + Send + 'a>,
>;

/// Async variant of [`Source`]. Implement this when collection requires I/O
/// that must not block the async executor (e.g. HTTP fetches, cloud APIs).
///
/// # Implementing
///
/// ```rust,ignore
/// use confex::{AsyncSource, CollectFuture, ConfigValue, SourceError};
///
/// struct RemoteSource { url: String }
///
/// impl AsyncSource for RemoteSource {
///     fn collect(&self) -> CollectFuture<'_> {
///         Box::pin(async move {
///             // async HTTP fetch, parse, return ConfigValue
///             todo!()
///         })
///     }
/// }
/// ```
#[cfg(feature = "async")]
pub trait AsyncSource: Send + Sync {
    /// Asynchronously collect config values from this source.
    ///
    /// # Errors
    /// Returns [`SourceError`] on I/O, parse, or network failure.
    fn collect(&self) -> CollectFuture<'_>;
}

/// Every sync `Source` that is `Send + Sync` is also an `AsyncSource`.
#[cfg(feature = "async")]
impl<T: Source + Send + Sync> AsyncSource for T {
    fn collect(&self) -> CollectFuture<'_> {
        Box::pin(std::future::ready(Source::collect(self)))
    }
}
