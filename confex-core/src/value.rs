use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Default)]
pub enum ConfigValue {
    #[default]
    Null,
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    Array(Vec<ConfigValue>),
    Table(BTreeMap<String, ConfigValue>),
}

impl ConfigValue {
    #[must_use]
    pub fn as_table(&self) -> Option<&BTreeMap<String, ConfigValue>> {
        match self {
            Self::Table(t) => Some(t),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_table_mut(&mut self) -> Option<&mut BTreeMap<String, ConfigValue>> {
        match self {
            Self::Table(t) => Some(t),
            _ => None,
        }
    }

    #[must_use]
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }
}

impl From<bool> for ConfigValue {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}

impl From<i64> for ConfigValue {
    fn from(v: i64) -> Self {
        Self::Integer(v)
    }
}

impl From<f64> for ConfigValue {
    fn from(v: f64) -> Self {
        Self::Float(v)
    }
}

impl From<String> for ConfigValue {
    fn from(v: String) -> Self {
        Self::String(v)
    }
}

impl From<&str> for ConfigValue {
    fn from(v: &str) -> Self {
        Self::String(v.to_owned())
    }
}

impl From<ConfigValue> for serde_json::Value {
    fn from(v: ConfigValue) -> Self {
        match v {
            ConfigValue::Null => Self::Null,
            ConfigValue::Bool(b) => Self::Bool(b),
            ConfigValue::Integer(i) => Self::Number(i.into()),
            ConfigValue::Float(f) => {
                serde_json::Number::from_f64(f).map_or(Self::Null, Self::Number)
            }
            ConfigValue::String(s) => Self::String(s),
            ConfigValue::Array(arr) => Self::Array(arr.into_iter().map(Into::into).collect()),
            ConfigValue::Table(map) => {
                Self::Object(map.into_iter().map(|(k, v)| (k, v.into())).collect())
            }
        }
    }
}
