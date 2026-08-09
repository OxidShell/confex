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
}

/// Reads config from environment variables with a given prefix.
///
/// `PREFIX_WINDOW__WIDTH=1024` → `{ "window": { "width": 1024 } }`
///
/// Double-underscore (`__`) is the nested key separator.
pub struct EnvSource {
    prefix: String,
}

impl EnvSource {
    #[must_use]
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
        }
    }
}

impl Source for EnvSource {
    fn collect(&self) -> Result<ConfigValue, SourceError> {
        let prefix = format!("{}_", self.prefix.to_uppercase());
        let mut table = BTreeMap::new();

        for (key, val) in std::env::vars() {
            let Some(stripped) = key.strip_prefix(&prefix) else {
                continue;
            };
            let path: Vec<String> = stripped.to_lowercase().split("__").map(String::from).collect();
            insert_nested(&mut table, &path, parse_env_value(&val));
        }

        Ok(ConfigValue::Table(table))
    }
}

fn parse_env_value(s: &str) -> ConfigValue {
    if s.eq_ignore_ascii_case("true") {
        return ConfigValue::Bool(true);
    }
    if s.eq_ignore_ascii_case("false") {
        return ConfigValue::Bool(false);
    }
    if let Ok(i) = s.parse::<i64>() {
        return ConfigValue::Integer(i);
    }
    if let Ok(f) = s.parse::<f64>() {
        return ConfigValue::Float(f);
    }
    ConfigValue::String(s.to_owned())
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

impl CliSource {
    #[must_use]
    pub fn from_pairs(pairs: impl IntoIterator<Item = (String, ConfigValue)>) -> Self {
        Self {
            values: pairs.into_iter().collect(),
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
    pub fn new(candidates: Vec<PathBuf>, format: F) -> Self {
        Self { candidates, format }
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

// ── async source ──────────────────────────────────────────────────────────────

/// Owned, boxed future returned by [`AsyncSource::collect`].
#[cfg(feature = "async")]
pub type CollectFuture<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<ConfigValue, SourceError>> + Send + 'a>>;

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
