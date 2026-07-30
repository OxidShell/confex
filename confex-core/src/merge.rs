use crate::value::ConfigValue;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeStrategy {
    /// Last writer wins. Default for scalars, enums, and arrays.
    Replace,
    /// Override only when the incoming value is non-null. Suitable for `Option<T>` fields.
    ReplaceIfPresent,
    /// Concatenate arrays; fall back to `Replace` for non-array values.
    Extend,
    /// Recurse into nested tables; fall back to `Replace` otherwise.
    Deep,
}

/// Pairs of `(field_name, strategy)` that override the default `Replace` behavior.
pub type FieldStrategies = &'static [(&'static str, MergeStrategy)];

/// Merge `override_val` on top of `base`, applying per-field strategies where specified.
///
/// A top-level `Null` override always leaves `base` unchanged.
#[must_use]
pub fn merge(
    base: ConfigValue,
    override_val: ConfigValue,
    strategies: FieldStrategies,
) -> ConfigValue {
    if override_val.is_null() {
        return base;
    }

    match (base, override_val) {
        (ConfigValue::Table(mut base_map), ConfigValue::Table(over_map)) => {
            for (key, over_val) in over_map {
                let strategy = strategies
                    .iter()
                    .find(|(k, _)| *k == key.as_str())
                    .map_or(MergeStrategy::Replace, |(_, s)| *s);

                let current = base_map.remove(&key);

                let merged = match strategy {
                    MergeStrategy::Replace => over_val,
                    MergeStrategy::ReplaceIfPresent => {
                        if over_val.is_null() {
                            if let Some(kept) = current {
                                base_map.insert(key, kept);
                            }
                            continue;
                        }
                        over_val
                    }
                    MergeStrategy::Extend => {
                        match (current, over_val) {
                            (
                                Some(ConfigValue::Array(mut base_arr)),
                                ConfigValue::Array(over_arr),
                            ) => {
                                base_arr.extend(over_arr);
                                ConfigValue::Array(base_arr)
                            }
                            (_, val) => val,
                        }
                    }
                    MergeStrategy::Deep => match (current, over_val) {
                        (Some(base_inner), over_inner) => merge(base_inner, over_inner, strategies),
                        (None, val) => val,
                    },
                };

                base_map.insert(key, merged);
            }
            ConfigValue::Table(base_map)
        }
        (_, override_val) => override_val,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn table(pairs: &[(&str, ConfigValue)]) -> ConfigValue {
        let map: BTreeMap<String, ConfigValue> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect();
        ConfigValue::Table(map)
    }

    #[test]
    fn replace_overwrites() {
        let base = table(&[("key", ConfigValue::Integer(1))]);
        let over = table(&[("key", ConfigValue::Integer(2))]);
        assert_eq!(
            merge(base, over, &[]),
            table(&[("key", ConfigValue::Integer(2))])
        );
    }

    #[test]
    fn null_override_leaves_base() {
        let base = table(&[("key", ConfigValue::Integer(1))]);
        assert_eq!(merge(base.clone(), ConfigValue::Null, &[]), base);
    }

    #[test]
    fn replace_if_present_keeps_base_when_override_null() {
        let base = table(&[("key", ConfigValue::String("base".into()))]);
        let over = table(&[("key", ConfigValue::Null)]);
        assert_eq!(
            merge(base, over, &[("key", MergeStrategy::ReplaceIfPresent)]),
            table(&[("key", ConfigValue::String("base".into()))])
        );
    }

    #[test]
    fn replace_if_present_replaces_when_override_non_null() {
        let base = table(&[("key", ConfigValue::Integer(1))]);
        let over = table(&[("key", ConfigValue::Integer(2))]);
        assert_eq!(
            merge(base, over, &[("key", MergeStrategy::ReplaceIfPresent)]),
            table(&[("key", ConfigValue::Integer(2))])
        );
    }

    #[test]
    fn deep_merge_nested_tables() {
        let base = table(&[(
            "nested",
            table(&[("a", ConfigValue::Integer(1)), ("b", ConfigValue::Integer(2))]),
        )]);
        let over = table(&[(
            "nested",
            table(&[("b", ConfigValue::Integer(3)), ("c", ConfigValue::Integer(4))]),
        )]);
        assert_eq!(
            merge(base, over, &[("nested", MergeStrategy::Deep)]),
            table(&[(
                "nested",
                table(&[
                    ("a", ConfigValue::Integer(1)),
                    ("b", ConfigValue::Integer(3)),
                    ("c", ConfigValue::Integer(4)),
                ])
            )])
        );
    }

    #[test]
    fn extend_concatenates_arrays() {
        let base = table(&[(
            "tags",
            ConfigValue::Array(vec![ConfigValue::String("a".into())]),
        )]);
        let over = table(&[(
            "tags",
            ConfigValue::Array(vec![ConfigValue::String("b".into())]),
        )]);
        assert_eq!(
            merge(base, over, &[("tags", MergeStrategy::Extend)]),
            table(&[(
                "tags",
                ConfigValue::Array(vec![
                    ConfigValue::String("a".into()),
                    ConfigValue::String("b".into()),
                ])
            )])
        );
    }
}
