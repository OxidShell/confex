use std::{collections::BTreeMap, path::Path};

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

/// TOML format (default).
pub struct TomlFormat;

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
        Ok(toml_to_config(toml_val))
    }

    fn serialize(&self, value: &ConfigValue) -> Result<Vec<u8>, SourceError> {
        let toml_val = config_to_toml(value.clone());
        toml::to_string(&toml_val)
            .map(String::into_bytes)
            .map_err(|e| SourceError::Serialize(e.to_string()))
    }
}

fn toml_to_config(val: toml::Value) -> ConfigValue {
    match val {
        toml::Value::Boolean(b) => ConfigValue::Bool(b),
        toml::Value::Integer(i) => ConfigValue::Integer(i),
        toml::Value::Float(f) => ConfigValue::Float(f),
        toml::Value::String(s) => ConfigValue::String(s),
        toml::Value::Array(arr) => ConfigValue::Array(arr.into_iter().map(toml_to_config).collect()),
        toml::Value::Table(map) => ConfigValue::Table(
            map.into_iter()
                .map(|(k, v)| (k, toml_to_config(v)))
                .collect(),
        ),
        toml::Value::Datetime(dt) => ConfigValue::String(dt.to_string()),
    }
}

fn config_to_toml(val: ConfigValue) -> toml::Value {
    match val {
        // TOML has no null; map to empty string as lossy fallback
        ConfigValue::Null => toml::Value::String(String::new()),
        ConfigValue::Bool(b) => toml::Value::Boolean(b),
        ConfigValue::Integer(i) => toml::Value::Integer(i),
        ConfigValue::Float(f) => toml::Value::Float(f),
        ConfigValue::String(s) => toml::Value::String(s),
        ConfigValue::Array(arr) => {
            toml::Value::Array(arr.into_iter().map(config_to_toml).collect())
        }
        ConfigValue::Table(map) => toml::Value::Table(
            map.into_iter()
                .map(|(k, v)| (k, config_to_toml(v)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
    }
}
