//! Compact application data. JSON is a CLI/debug representation, not a stored JSON tree.
use crate::{ApiError, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use bytes::Bytes;
use prost::Message;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const MAX_BYTES: usize = 4096;
pub const LEGACY_JSON: &str = "application/vnd.mynofy.legacy+json";
#[derive(Clone, PartialEq, Message)]
pub struct Payload {
    #[prost(string, optional, tag = "1")]
    pub content_type: Option<String>,
    #[prost(oneof = "Body", tags = "2,3")]
    pub body: Option<Body>,
}
#[derive(Clone, PartialEq, prost::Oneof)]
pub enum Body {
    #[prost(bytes = "bytes", tag = "2")]
    Data(Bytes),
    #[prost(message, tag = "3")]
    Event(Event),
}
#[derive(Clone, PartialEq, Message, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    #[prost(uint32, required, tag = "1")]
    pub catalog: u32,
    #[prost(uint32, required, tag = "2")]
    pub catalog_version: u32,
    #[prost(uint32, required, tag = "3")]
    pub category: u32,
    #[prost(uint32, required, tag = "4")]
    pub action: u32,
    #[prost(uint32, required, tag = "5")]
    pub status: u32,
    #[prost(message, repeated, tag = "6")]
    #[serde(default)]
    pub parameters: Vec<Parameter>,
}
#[derive(Clone, PartialEq, Message, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Parameter {
    #[prost(oneof = "ParameterValue", tags = "1,2,3,4,5,6,7")]
    pub value: Option<ParameterValue>,
}
#[derive(Clone, PartialEq, prost::Oneof, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParameterValue {
    #[prost(uint32, tag = "1")]
    U32(u32),
    #[prost(uint64, tag = "2")]
    U64(u64),
    #[prost(sint32, tag = "3")]
    I32(i32),
    #[prost(sint64, tag = "4")]
    I64(i64),
    #[prost(float, tag = "5")]
    F32(f32),
    #[prost(double, tag = "6")]
    F64(f64),
    #[prost(bool, tag = "7")]
    Bool(bool),
}
impl Payload {
    pub fn bytes(data: impl Into<Bytes>, content_type: Option<String>) -> Self {
        Self {
            body: Some(Body::Data(data.into())),
            content_type,
        }
    }
    pub fn event(event: Event) -> Self {
        Self {
            body: Some(Body::Event(event)),
            content_type: None,
        }
    }
    pub fn value_len(&self) -> usize {
        match &self.body {
            Some(Body::Data(data)) => data.len(),
            Some(Body::Event(e)) => e
                .parameters
                .iter()
                .map(|p| match p.value {
                    Some(ParameterValue::Bool(_)) => 1,
                    Some(
                        ParameterValue::U32(_) | ParameterValue::I32(_) | ParameterValue::F32(_),
                    ) => 4,
                    _ => 8,
                })
                .sum(),
            None => 0,
        }
    }
    pub fn validate(&self) -> Result<()> {
        if self
            .content_type
            .as_ref()
            .is_some_and(|s| s.is_empty() || s.len() > 128)
        {
            return Err(ApiError::invalid());
        }
        match &self.body {
            Some(Body::Data(data)) if data.len() <= MAX_BYTES => {
                if self.content_type.as_deref() == Some(LEGACY_JSON) {
                    crate::strict_json(
                        std::str::from_utf8(data).map_err(|_| ApiError::invalid())?,
                    )?;
                }
                Ok(())
            }
            Some(Body::Event(e)) => {
                if self.content_type.is_some()
                    || e.catalog != 1
                    || e.catalog_version != 1
                    || e.parameters.len() > 3
                {
                    return Err(ApiError::invalid());
                }
                if !crate::catalog_values::valid_category(e.category)
                    || !crate::catalog_values::valid_action(e.action)
                    || !crate::catalog_values::valid_status(e.status)
                {
                    return Err(ApiError::invalid());
                }
                for p in &e.parameters {
                    match p.value {
                        None => return Err(ApiError::invalid()),
                        Some(ParameterValue::F32(x)) if !x.is_finite() => {
                            return Err(ApiError::invalid());
                        }
                        Some(ParameterValue::F64(x)) if !x.is_finite() => {
                            return Err(ApiError::invalid());
                        }
                        _ => {}
                    }
                }
                Ok(())
            }
            _ => Err(ApiError::invalid()),
        }
    }
    pub fn from_json(value: Value) -> Result<Self> {
        let payload = match value.get("kind").and_then(Value::as_str) {
            Some("bytes") => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Input {
                    kind: String,
                    data: String,
                    content_type: Option<String>,
                }
                let input: Input =
                    serde_json::from_value(value).map_err(|_| ApiError::invalid())?;
                let _ = input.kind;
                if input.data.len() > MAX_BYTES.div_ceil(3) * 4 {
                    return Err(ApiError::invalid());
                }
                let data = STANDARD
                    .decode(&input.data)
                    .map_err(|_| ApiError::invalid())?;
                if STANDARD.encode(&data) != input.data {
                    return Err(ApiError::invalid());
                }
                Self::bytes(data, input.content_type)
            }
            Some("event") => {
                let mut value = value;
                value
                    .as_object_mut()
                    .ok_or_else(ApiError::invalid)?
                    .remove("kind");
                Self::event(serde_json::from_value(value).map_err(|_| ApiError::invalid())?)
            }
            _ => Self::bytes(
                serde_json::to_vec(&value).map_err(|_| ApiError::invalid())?,
                Some(LEGACY_JSON.into()),
            ),
        };
        payload.validate()?;
        Ok(payload)
    }
    pub fn to_json(&self) -> Result<Value> {
        match &self.body {
            Some(Body::Data(data)) if self.content_type.as_deref() == Some(LEGACY_JSON) => {
                crate::strict_json(std::str::from_utf8(data).map_err(|_| ApiError::internal())?)
            }
            Some(Body::Data(data)) => {
                let mut v = json!({"kind":"bytes","data":STANDARD.encode(data)});
                if let Some(t) = &self.content_type {
                    v["content_type"] = t.clone().into();
                }
                Ok(v)
            }
            Some(Body::Event(event)) => {
                let mut v = serde_json::to_value(event).map_err(|_| ApiError::internal())?;
                v["kind"] = "event".into();
                Ok(v)
            }
            None => Err(ApiError::invalid()),
        }
    }
}
impl Serialize for Payload {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        self.to_json()
            .map_err(serde::ser::Error::custom)?
            .serialize(s)
    }
}
impl<'de> Deserialize<'de> for Payload {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Self::from_json(Value::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}
