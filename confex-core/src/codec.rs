/// Compact, self-describing codec for [`crate::value::ConfigValue`].
///
/// No external crates. Format is length-prefixed so it is binary-safe for
/// strings. Used by the `SQLite` backend to avoid a `serde_json` dependency.
///
/// Grammar (all counts/lengths are decimal digits terminated by a delimiter):
/// ```text
/// value  = "n"                          (Null)
///        | "T" | "F"                   (Bool true / false)
///        | "i" <i64>  "|"              (Integer)
///        | "f" <f64>  "|"              (Float, uses Rust's default Display)
///        | "s" <len> ":" <bytes>       (String, len in bytes)
///        | "a" <count> ":" value*      (Array)
///        | "m" <count> ":" entry*      (Table / Map)
/// entry  = <keylen> ":" <keybytes> value
/// ```
use std::collections::BTreeMap;

use crate::{error::SourceError, value::ConfigValue};

pub(crate) fn encode(v: &ConfigValue) -> String {
    let mut out = String::new();
    write_value(v, &mut out);
    out
}

fn write_value(v: &ConfigValue, out: &mut String) {
    match v {
        ConfigValue::Null => out.push('n'),
        ConfigValue::Bool(true) => out.push('T'),
        ConfigValue::Bool(false) => out.push('F'),
        ConfigValue::Integer(i) => {
            out.push('i');
            out.push_str(&i.to_string());
            out.push('|');
        }
        ConfigValue::Float(f) => {
            out.push('f');
            out.push_str(&f.to_string());
            out.push('|');
        }
        ConfigValue::String(s) => {
            out.push('s');
            out.push_str(&s.len().to_string());
            out.push(':');
            out.push_str(s);
        }
        ConfigValue::Array(arr) => {
            out.push('a');
            out.push_str(&arr.len().to_string());
            out.push(':');
            for item in arr {
                write_value(item, out);
            }
        }
        ConfigValue::Table(map) => {
            out.push('m');
            out.push_str(&map.len().to_string());
            out.push(':');
            for (k, val) in map {
                out.push_str(&k.len().to_string());
                out.push(':');
                out.push_str(k);
                write_value(val, out);
            }
        }
    }
}

pub(crate) fn decode(s: &str) -> Result<ConfigValue, SourceError> {
    let (val, rest) = parse_value(s)?;
    if !rest.is_empty() {
        return Err(SourceError::Backend(format!(
            "trailing bytes after decoded value: {rest:?}"
        )));
    }
    Ok(val)
}

fn parse_value(s: &str) -> Result<(ConfigValue, &str), SourceError> {
    let mut chars = s.chars();
    let tag = chars
        .next()
        .ok_or_else(|| SourceError::Backend("unexpected end of encoded value".into()))?;
    let rest = chars.as_str();

    match tag {
        'n' => Ok((ConfigValue::Null, rest)),
        'T' => Ok((ConfigValue::Bool(true), rest)),
        'F' => Ok((ConfigValue::Bool(false), rest)),
        'i' => {
            let (digits, rest) = split_delimiter(rest, '|')?;
            let n: i64 = digits
                .parse()
                .map_err(|_| SourceError::Backend(format!("invalid integer: {digits:?}")))?;
            Ok((ConfigValue::Integer(n), rest))
        }
        'f' => {
            let (digits, rest) = split_delimiter(rest, '|')?;
            let f: f64 = digits
                .parse()
                .map_err(|_| SourceError::Backend(format!("invalid float: {digits:?}")))?;
            Ok((ConfigValue::Float(f), rest))
        }
        's' => {
            let (len_str, rest) = split_delimiter(rest, ':')?;
            let len: usize = len_str
                .parse()
                .map_err(|_| SourceError::Backend(format!("invalid string length: {len_str:?}")))?;
            if rest.len() < len {
                return Err(SourceError::Backend(format!(
                    "string truncated: expected {len} bytes, got {}",
                    rest.len()
                )));
            }
            let (s, rest) = rest.split_at(len);
            Ok((ConfigValue::String(s.to_owned()), rest))
        }
        'a' => {
            let (count_str, mut rest) = split_delimiter(rest, ':')?;
            let count: usize = count_str
                .parse()
                .map_err(|_| SourceError::Backend(format!("invalid array count: {count_str:?}")))?;
            let mut arr = Vec::with_capacity(count);
            for _ in 0..count {
                let (item, r) = parse_value(rest)?;
                arr.push(item);
                rest = r;
            }
            Ok((ConfigValue::Array(arr), rest))
        }
        'm' => {
            let (count_str, mut rest) = split_delimiter(rest, ':')?;
            let count: usize = count_str
                .parse()
                .map_err(|_| SourceError::Backend(format!("invalid table count: {count_str:?}")))?;
            let mut map = BTreeMap::new();
            for _ in 0..count {
                let (klen_str, r) = split_delimiter(rest, ':')?;
                let klen: usize = klen_str.parse().map_err(|_| {
                    SourceError::Backend(format!("invalid key length: {klen_str:?}"))
                })?;
                if r.len() < klen {
                    return Err(SourceError::Backend(format!(
                        "key truncated: expected {klen} bytes, got {}",
                        r.len()
                    )));
                }
                let (key, r) = r.split_at(klen);
                let (val, r) = parse_value(r)?;
                map.insert(key.to_owned(), val);
                rest = r;
            }
            Ok((ConfigValue::Table(map), rest))
        }
        other => Err(SourceError::Backend(format!(
            "unknown type tag {other:?}"
        ))),
    }
}

fn split_delimiter(s: &str, delim: char) -> Result<(&str, &str), SourceError> {
    s.find(delim)
        .map(|i| (&s[..i], &s[i + delim.len_utf8()..]))
        .ok_or_else(|| {
            SourceError::Backend(format!("missing {delim:?} delimiter in {s:?}"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rt(v: &ConfigValue) {
        let encoded = encode(v);
        let decoded = decode(&encoded).expect("decode failed");
        assert_eq!(*v, decoded, "round-trip failed for encoded={encoded:?}");
    }

    #[test]
    fn round_trip_scalars() {
        rt(&ConfigValue::Null);
        rt(&ConfigValue::Bool(true));
        rt(&ConfigValue::Bool(false));
        rt(&ConfigValue::Integer(0));
        rt(&ConfigValue::Integer(i64::MIN));
        rt(&ConfigValue::Integer(i64::MAX));
        rt(&ConfigValue::Float(1.5));
        rt(&ConfigValue::Float(f64::INFINITY));
        rt(&ConfigValue::Float(f64::NEG_INFINITY));
        rt(&ConfigValue::String(String::new()));
        rt(&ConfigValue::String("hello, world!".into()));
        rt(&ConfigValue::String("héllo".into()));
        rt(&ConfigValue::String("with|pipe:colon".into()));
    }

    #[test]
    fn round_trip_nested() {
        rt(&ConfigValue::Array(vec![
            ConfigValue::Integer(1),
            ConfigValue::String("two".into()),
            ConfigValue::Bool(false),
        ]));

        let mut map = BTreeMap::new();
        map.insert("x".into(), ConfigValue::Integer(42));
        map.insert("y".into(), ConfigValue::Array(vec![ConfigValue::Null]));
        rt(&ConfigValue::Table(map));
    }
}
