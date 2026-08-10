use std::path::Path;

use crate::{error::SourceError, value::ConfigValue};

/// Pluggable serialization format for config files.
pub trait Format {
    fn name(&self) -> &'static str;

    /// Parse raw bytes from `path` into a `ConfigValue` tree.
    ///
    /// # Errors
    /// Returns [`SourceError::Parse`] if the bytes are not valid for this format.
    fn parse(&self, path: &Path, bytes: &[u8]) -> Result<ConfigValue, SourceError>;

    /// Serialize a `ConfigValue` tree to bytes.
    ///
    /// # Errors
    /// Returns [`SourceError::Serialize`] if the value cannot be expressed in this format.
    fn serialize(&self, value: &ConfigValue) -> Result<Vec<u8>, SourceError>;
}

/// TOML format. Requires the `toml` feature.
#[cfg(feature = "toml")]
pub struct TomlFormat;

#[cfg(feature = "toml")]
impl Format for TomlFormat {
    fn name(&self) -> &'static str {
        "toml"
    }

    fn parse(&self, path: &Path, bytes: &[u8]) -> Result<ConfigValue, SourceError> {
        let s = std::str::from_utf8(bytes).map_err(|e| SourceError::Parse {
            path: path.to_owned(),
            message: e.to_string(),
        })?;
        let toml_val: toml::Value = toml::from_str(s).map_err(|e| SourceError::Parse {
            path: path.to_owned(),
            message: e.to_string(),
        })?;
        Ok(toml_val.into())
    }

    fn serialize(&self, value: &ConfigValue) -> Result<Vec<u8>, SourceError> {
        let toml_val: toml::Value = value.clone().into();
        toml::to_string(&toml_val)
            .map(String::into_bytes)
            .map_err(|e| SourceError::Serialize(e.to_string()))
    }
}
