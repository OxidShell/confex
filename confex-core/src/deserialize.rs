use std::collections::BTreeMap;

use serde::{
    de::{self, IntoDeserializer as _, Visitor},
    ser,
};

use crate::value::ConfigValue;

/// Error produced when deserializing a [`ConfigValue`] tree into a concrete type.
#[derive(Debug)]
pub struct DeserializeError(String);

impl std::fmt::Display for DeserializeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for DeserializeError {}

impl de::Error for DeserializeError {
    fn custom<T: std::fmt::Display>(msg: T) -> Self {
        Self(msg.to_string())
    }
}

impl ser::Error for DeserializeError {
    fn custom<T: std::fmt::Display>(msg: T) -> Self {
        Self(msg.to_string())
    }
}

/// Drives serde deserialization directly from a [`ConfigValue`] tree.
///
/// Replaces the `ConfigValue → serde_json::Value → S` round-trip with a single
/// pass that does not require the `serde_json` crate.
pub struct ConfigValueDeserializer(pub ConfigValue);

type Result<T> = std::result::Result<T, DeserializeError>;

impl<'de> de::Deserializer<'de> for ConfigValueDeserializer {
    type Error = DeserializeError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self.0 {
            ConfigValue::Null => visitor.visit_unit(),
            ConfigValue::Bool(b) => visitor.visit_bool(b),
            ConfigValue::Integer(i) => visitor.visit_i64(i),
            ConfigValue::Float(f) => visitor.visit_f64(f),
            ConfigValue::String(s) => visitor.visit_string(s),
            ConfigValue::Array(arr) => {
                visitor.visit_seq(SeqDeserializer(arr.into_iter()))
            }
            ConfigValue::Table(map) => visitor.visit_map(MapDeserializer::new(map)),
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self.0 {
            ConfigValue::Null => visitor.visit_none(),
            other => visitor.visit_some(Self(other)),
        }
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        match self.0 {
            ConfigValue::String(s) => visitor.visit_enum(s.into_deserializer()),
            ConfigValue::Table(mut map) if map.len() == 1 => {
                let (variant, payload) = map.pop_first().expect("len == 1 checked");
                visitor.visit_enum(EnumDeserializer { variant, payload })
            }
            other => Err(de::Error::invalid_type(
                unexpected(&other),
                &"string or single-key table for enum variant",
            )),
        }
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64
        char str string bytes byte_buf
        unit unit_struct
        seq tuple tuple_struct
        map struct
        identifier ignored_any
    }
}

fn unexpected(v: &ConfigValue) -> de::Unexpected<'_> {
    match v {
        ConfigValue::Null => de::Unexpected::Unit,
        ConfigValue::Bool(b) => de::Unexpected::Bool(*b),
        ConfigValue::Integer(i) => de::Unexpected::Signed(*i),
        ConfigValue::Float(f) => de::Unexpected::Float(*f),
        ConfigValue::String(s) => de::Unexpected::Str(s),
        ConfigValue::Array(_) => de::Unexpected::Seq,
        ConfigValue::Table(_) => de::Unexpected::Map,
    }
}

struct SeqDeserializer(std::vec::IntoIter<ConfigValue>);

impl<'de> de::SeqAccess<'de> for SeqDeserializer {
    type Error = DeserializeError;

    fn next_element_seed<T: de::DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>> {
        match self.0.next() {
            Some(v) => seed.deserialize(ConfigValueDeserializer(v)).map(Some),
            None => Ok(None),
        }
    }
}

struct MapDeserializer {
    iter: std::collections::btree_map::IntoIter<String, ConfigValue>,
    pending: Option<ConfigValue>,
}

impl MapDeserializer {
    fn new(map: BTreeMap<String, ConfigValue>) -> Self {
        Self {
            iter: map.into_iter(),
            pending: None,
        }
    }
}

impl<'de> de::MapAccess<'de> for MapDeserializer {
    type Error = DeserializeError;

    fn next_key_seed<K: de::DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>> {
        match self.iter.next() {
            Some((k, v)) => {
                self.pending = Some(v);
                seed.deserialize(de::value::StringDeserializer::new(k))
                    .map(Some)
            }
            None => Ok(None),
        }
    }

    fn next_value_seed<V: de::DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> Result<V::Value> {
        let v = self.pending.take().expect("called after next_key_seed");
        seed.deserialize(ConfigValueDeserializer(v))
    }
}

struct EnumDeserializer {
    variant: String,
    payload: ConfigValue,
}

impl<'de> de::EnumAccess<'de> for EnumDeserializer {
    type Error = DeserializeError;
    type Variant = Self;

    fn variant_seed<V: de::DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, Self::Variant)> {
        let tag = seed.deserialize(de::value::StringDeserializer::new(self.variant.clone()))?;
        Ok((tag, self))
    }
}

impl<'de> de::VariantAccess<'de> for EnumDeserializer {
    type Error = DeserializeError;

    fn unit_variant(self) -> Result<()> {
        Ok(())
    }

    fn newtype_variant_seed<T: de::DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value> {
        seed.deserialize(ConfigValueDeserializer(self.payload))
    }

    fn tuple_variant<V: Visitor<'de>>(self, _len: usize, visitor: V) -> Result<V::Value> {
        de::Deserializer::deserialize_seq(ConfigValueDeserializer(self.payload), visitor)
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        de::Deserializer::deserialize_map(ConfigValueDeserializer(self.payload), visitor)
    }
}

/// Serializes any `T: Serialize` into a [`ConfigValue`] tree without a `serde_json` dependency.
pub struct ConfigValueSerializer;

/// Serialize any `T: serde::Serialize` into a [`ConfigValue`].
///
/// # Errors
/// Returns [`DeserializeError`] if `T` produces a type that cannot be
/// represented as a `ConfigValue` (e.g. maps with non-string keys).
pub fn to_config_value<T: serde::Serialize>(value: &T) -> std::result::Result<ConfigValue, DeserializeError> {
    value.serialize(ConfigValueSerializer)
}

impl ser::Serializer for ConfigValueSerializer {
    type Ok = ConfigValue;
    type Error = DeserializeError;

    type SerializeSeq = SeqSerializer;
    type SerializeTuple = SeqSerializer;
    type SerializeTupleStruct = SeqSerializer;
    type SerializeTupleVariant = TupleVariantSerializer;
    type SerializeMap = MapSerializer;
    type SerializeStruct = MapSerializer;
    type SerializeStructVariant = StructVariantSerializer;

    fn serialize_bool(self, v: bool) -> Result<ConfigValue> { Ok(ConfigValue::Bool(v)) }
    fn serialize_i8(self, v: i8) -> Result<ConfigValue> { Ok(ConfigValue::Integer(i64::from(v))) }
    fn serialize_i16(self, v: i16) -> Result<ConfigValue> { Ok(ConfigValue::Integer(i64::from(v))) }
    fn serialize_i32(self, v: i32) -> Result<ConfigValue> { Ok(ConfigValue::Integer(i64::from(v))) }
    fn serialize_i64(self, v: i64) -> Result<ConfigValue> { Ok(ConfigValue::Integer(v)) }
    fn serialize_i128(self, v: i128) -> Result<ConfigValue> {
        i64::try_from(v).map(ConfigValue::Integer).map_err(ser::Error::custom)
    }
    fn serialize_u8(self, v: u8) -> Result<ConfigValue> { Ok(ConfigValue::Integer(i64::from(v))) }
    fn serialize_u16(self, v: u16) -> Result<ConfigValue> { Ok(ConfigValue::Integer(i64::from(v))) }
    fn serialize_u32(self, v: u32) -> Result<ConfigValue> { Ok(ConfigValue::Integer(i64::from(v))) }
    fn serialize_u64(self, v: u64) -> Result<ConfigValue> {
        i64::try_from(v).map(ConfigValue::Integer).map_err(ser::Error::custom)
    }
    fn serialize_u128(self, v: u128) -> Result<ConfigValue> {
        i64::try_from(v).map(ConfigValue::Integer).map_err(ser::Error::custom)
    }
    fn serialize_f32(self, v: f32) -> Result<ConfigValue> { Ok(ConfigValue::Float(f64::from(v))) }
    fn serialize_f64(self, v: f64) -> Result<ConfigValue> { Ok(ConfigValue::Float(v)) }
    fn serialize_char(self, v: char) -> Result<ConfigValue> {
        Ok(ConfigValue::String(v.to_string()))
    }
    fn serialize_str(self, v: &str) -> Result<ConfigValue> {
        Ok(ConfigValue::String(v.to_owned()))
    }
    fn serialize_bytes(self, v: &[u8]) -> Result<ConfigValue> {
        Ok(ConfigValue::Array(v.iter().map(|b| ConfigValue::Integer(i64::from(*b))).collect()))
    }
    fn serialize_none(self) -> Result<ConfigValue> { Ok(ConfigValue::Null) }
    fn serialize_some<T: serde::Serialize + ?Sized>(self, value: &T) -> Result<ConfigValue> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<ConfigValue> { Ok(ConfigValue::Null) }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<ConfigValue> {
        Ok(ConfigValue::Null)
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<ConfigValue> {
        Ok(ConfigValue::String(variant.to_owned()))
    }
    fn serialize_newtype_struct<T: serde::Serialize + ?Sized>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<ConfigValue> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: serde::Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<ConfigValue> {
        let mut map = BTreeMap::new();
        map.insert(variant.to_owned(), value.serialize(Self)?);
        Ok(ConfigValue::Table(map))
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<SeqSerializer> {
        Ok(SeqSerializer(Vec::with_capacity(len.unwrap_or(0))))
    }
    fn serialize_tuple(self, len: usize) -> Result<SeqSerializer> {
        Ok(SeqSerializer(Vec::with_capacity(len)))
    }
    fn serialize_tuple_struct(self, _name: &'static str, len: usize) -> Result<SeqSerializer> {
        Ok(SeqSerializer(Vec::with_capacity(len)))
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<TupleVariantSerializer> {
        Ok(TupleVariantSerializer {
            variant: variant.to_owned(),
            items: Vec::with_capacity(len),
        })
    }
    fn serialize_map(self, _len: Option<usize>) -> Result<MapSerializer> {
        Ok(MapSerializer::default())
    }
    fn serialize_struct(self, _name: &'static str, _len: usize) -> Result<MapSerializer> {
        Ok(MapSerializer::default())
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<StructVariantSerializer> {
        Ok(StructVariantSerializer {
            variant: variant.to_owned(),
            fields: BTreeMap::new(),
        })
    }
}

pub struct SeqSerializer(Vec<ConfigValue>);

impl ser::SerializeSeq for SeqSerializer {
    type Ok = ConfigValue;
    type Error = DeserializeError;
    fn serialize_element<T: serde::Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.0.push(value.serialize(ConfigValueSerializer)?);
        Ok(())
    }
    fn end(self) -> Result<ConfigValue> { Ok(ConfigValue::Array(self.0)) }
}
impl ser::SerializeTuple for SeqSerializer {
    type Ok = ConfigValue;
    type Error = DeserializeError;
    fn serialize_element<T: serde::Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<ConfigValue> { ser::SerializeSeq::end(self) }
}
impl ser::SerializeTupleStruct for SeqSerializer {
    type Ok = ConfigValue;
    type Error = DeserializeError;
    fn serialize_field<T: serde::Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<ConfigValue> { ser::SerializeSeq::end(self) }
}

pub struct TupleVariantSerializer {
    variant: String,
    items: Vec<ConfigValue>,
}
impl ser::SerializeTupleVariant for TupleVariantSerializer {
    type Ok = ConfigValue;
    type Error = DeserializeError;
    fn serialize_field<T: serde::Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.items.push(value.serialize(ConfigValueSerializer)?);
        Ok(())
    }
    fn end(self) -> Result<ConfigValue> {
        let mut map = BTreeMap::new();
        map.insert(self.variant, ConfigValue::Array(self.items));
        Ok(ConfigValue::Table(map))
    }
}

#[derive(Default)]
pub struct MapSerializer {
    map: BTreeMap<String, ConfigValue>,
    pending_key: Option<String>,
}
impl ser::SerializeMap for MapSerializer {
    type Ok = ConfigValue;
    type Error = DeserializeError;
    fn serialize_key<T: serde::Serialize + ?Sized>(&mut self, key: &T) -> Result<()> {
        let k = match key.serialize(ConfigValueSerializer)? {
            ConfigValue::String(s) => s,
            other => return Err(ser::Error::custom(format!("map key must be a string, got {other:?}"))),
        };
        self.pending_key = Some(k);
        Ok(())
    }
    fn serialize_value<T: serde::Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        let k = self.pending_key.take().expect("serialize_value called after serialize_key");
        self.map.insert(k, value.serialize(ConfigValueSerializer)?);
        Ok(())
    }
    fn end(self) -> Result<ConfigValue> { Ok(ConfigValue::Table(self.map)) }
}
impl ser::SerializeStruct for MapSerializer {
    type Ok = ConfigValue;
    type Error = DeserializeError;
    fn serialize_field<T: serde::Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<()> {
        self.map.insert(key.to_owned(), value.serialize(ConfigValueSerializer)?);
        Ok(())
    }
    fn end(self) -> Result<ConfigValue> { Ok(ConfigValue::Table(self.map)) }
}

pub struct StructVariantSerializer {
    variant: String,
    fields: BTreeMap<String, ConfigValue>,
}
impl ser::SerializeStructVariant for StructVariantSerializer {
    type Ok = ConfigValue;
    type Error = DeserializeError;
    fn serialize_field<T: serde::Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<()> {
        self.fields.insert(key.to_owned(), value.serialize(ConfigValueSerializer)?);
        Ok(())
    }
    fn end(self) -> Result<ConfigValue> {
        let mut map = BTreeMap::new();
        map.insert(self.variant, ConfigValue::Table(self.fields));
        Ok(ConfigValue::Table(map))
    }
}
