use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fmt};

pub type Result<T> = std::result::Result<T, ApiError>;
#[derive(Debug, Clone)]
pub struct ApiError {
    pub code: &'static str,
    pub detail: &'static str,
    pub retry_after_ms: Option<u64>,
}
impl ApiError {
    pub fn new(code: &'static str, detail: &'static str) -> Self {
        Self {
            code,
            detail,
            retry_after_ms: None,
        }
    }
    pub fn invalid() -> Self {
        Self::new("invalid_frame", "Invalid frame or field value.")
    }
    pub fn unauthorized() -> Self {
        Self::new(
            "unauthorized",
            "Authentication failed or credential is no longer valid.",
        )
    }
    pub fn internal() -> Self {
        Self::new("internal_error", "Internal operation failed.")
    }
    pub fn exhausted() -> Self {
        Self::new("resource_exhausted", "Configured capacity reached.")
    }
    pub fn limited(ms: u64) -> Self {
        Self {
            code: "rate_limited",
            detail: "Rate limit exceeded.",
            retry_after_ms: Some(ms.max(1)),
        }
    }
    pub fn frame(&self, id: Option<&str>) -> Value {
        let mut frame = json!({"version":1,"type":"error","code":self.code,"detail":self.detail});
        if let Some(id) = id {
            frame["request_id"] = id.into();
        }
        if let Some(ms) = self.retry_after_ms {
            frame["retry_after_ms"] = ms.into();
        }
        frame
    }
}
impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}
impl std::error::Error for ApiError {}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Expires {
    pub time: String,
}
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    AuthHello {
        id: String,
        token_id: String,
    },
    AuthProof {
        id: String,
        auth_id: String,
        signature: String,
    },
    Publish {
        id: String,
        topic: String,
        payload: Value,
        expires: Option<Expires>,
    },
    Poll {
        id: String,
        topic: String,
        after: Option<String>,
        limit: Option<usize>,
    },
    GetMessage {
        id: String,
        message_id: String,
    },
    ListMessages {
        id: String,
        topic: Option<String>,
        limit: Option<usize>,
        cursor: Option<String>,
    },
    GetCatalog {
        id: String,
        name: String,
        catalog_version: u32,
    },
    GetPermissions {
        id: String,
        topic: String,
    },
    RotationStart {
        id: String,
    },
    RotationConfirm {
        id: String,
        rotation_id: String,
        shared_secret: String,
    },
    RotationCommit {
        id: String,
        rotation_id: String,
        encrypted_public_key: String,
        signature: String,
    },
}
impl Command {
    pub fn id(&self) -> &str {
        match self {
            Self::AuthHello { id, .. }
            | Self::AuthProof { id, .. }
            | Self::Publish { id, .. }
            | Self::Poll { id, .. }
            | Self::GetMessage { id, .. }
            | Self::ListMessages { id, .. }
            | Self::GetCatalog { id, .. }
            | Self::GetPermissions { id, .. }
            | Self::RotationStart { id, .. }
            | Self::RotationConfirm { id, .. }
            | Self::RotationCommit { id, .. } => id,
        }
    }
}
/// Reject duplicate object keys at every depth, including application payloads.
struct StrictValue(Value);
impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = StrictValue;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("JSON without duplicate object keys")
            }
            fn visit_bool<E: serde::de::Error>(
                self,
                v: bool,
            ) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(v.into()))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(v.into()))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(v.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> std::result::Result<Self::Value, E> {
                serde_json::Number::from_f64(v)
                    .map(|n| StrictValue(n.into()))
                    .ok_or_else(|| E::custom("non-finite number"))
            }
            fn visit_str<E: serde::de::Error>(
                self,
                v: &str,
            ) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(v.into()))
            }
            fn visit_string<E: serde::de::Error>(
                self,
                v: String,
            ) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(v.into()))
            }
            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(Value::Null))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut v = Vec::new();
                while let Some(StrictValue(x)) = a.next_element()? {
                    v.push(x);
                }
                Ok(StrictValue(v.into()))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut v = BTreeMap::new();
                while let Some(k) = a.next_key::<String>()? {
                    if v.contains_key(&k) {
                        return Err(serde::de::Error::custom("duplicate JSON key"));
                    }
                    let StrictValue(x) = a.next_value()?;
                    v.insert(k, x);
                }
                Ok(StrictValue(
                    serde_json::to_value(v).map_err(serde::de::Error::custom)?,
                ))
            }
        }
        d.deserialize_any(Visitor)
    }
}
pub fn strict_json(text: &str) -> Result<Value> {
    serde_json::from_str::<StrictValue>(text)
        .map(|v| v.0)
        .map_err(|_| ApiError::invalid())
}
pub fn parse(text: &str) -> Result<Command> {
    parse_value(strict_json(text)?)
}
pub(crate) fn validate_command_presence(value: &Value) -> Result<()> {
    if value.as_object().is_none_or(|map| {
        map.iter()
            .any(|(key, value)| key != "payload" && value.is_null())
    }) {
        return Err(ApiError::invalid());
    }
    Ok(())
}
pub(crate) fn is_client_command(kind: &str) -> bool {
    matches!(
        kind,
        "auth_hello"
            | "auth_proof"
            | "publish"
            | "poll"
            | "get_message"
            | "list_messages"
            | "get_permissions"
            | "get_catalog"
            | "rotation_start"
            | "rotation_confirm"
            | "rotation_commit"
    )
}
pub fn parse_value(mut value: Value) -> Result<Command> {
    if value.get("version").and_then(Value::as_u64) != Some(1) {
        return Err(ApiError::new(
            "unsupported_version",
            "Expected protocol version 1.",
        ));
    }
    validate_command_presence(&value)?;
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(ApiError::invalid)?;
    if !is_client_command(kind) {
        if [
            "auth_challenge",
            "auth_welcome",
            "result",
            "messages",
            "messages_page",
            "message_status",
            "permissions",
            "catalog",
            "error",
            "rotation_offer",
            "rotation_ready",
            "rotation_complete",
        ]
        .contains(&kind)
        {
            return Err(ApiError::new(
                "invalid_direction",
                "Server-only frame sent by client.",
            ));
        }
        return Err(ApiError::new("unknown_type", "Unknown client frame type."));
    }
    value
        .as_object_mut()
        .ok_or_else(ApiError::invalid)?
        .remove("version");
    let cmd: Command = serde_json::from_value(value).map_err(|_| ApiError::invalid())?;
    if cmd.id().is_empty() || cmd.id().len() > 128 {
        return Err(ApiError::invalid());
    }
    Ok(cmd)
}
/// Canonical RFC3339 UTC for millisecond timestamps, using a fixed stack buffer.
/// Calendar conversion remains delegated to `time`; no custom date arithmetic.
#[derive(Clone)]
pub struct Timestamp {
    bytes: [u8; 24],
    len: usize,
}
impl Timestamp {
    pub fn new(ms: u64) -> Result<Self> {
        let date = time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
            .map_err(|_| ApiError::invalid())?;
        let (year, month, day) = date.to_calendar_date();
        if !(0..=9999).contains(&year) {
            return Err(ApiError::internal());
        }
        let (hour, minute, second) = date.to_hms();
        let mut bytes = *b"0000-00-00T00:00:00.000Z";
        let year = year as u16;
        bytes[0] = b'0' + (year / 1000) as u8;
        bytes[1] = b'0' + (year / 100 % 10) as u8;
        bytes[2] = b'0' + (year / 10 % 10) as u8;
        bytes[3] = b'0' + (year % 10) as u8;
        fn pair(bytes: &mut [u8], value: u8) {
            bytes[0] = b'0' + value / 10;
            bytes[1] = b'0' + value % 10;
        }
        pair(&mut bytes[5..7], month as u8);
        pair(&mut bytes[8..10], day);
        pair(&mut bytes[11..13], hour);
        pair(&mut bytes[14..16], minute);
        pair(&mut bytes[17..19], second);
        let fraction = (ms % 1000) as u16;
        let len = if fraction == 0 {
            bytes[19] = b'Z';
            20
        } else {
            bytes[20] = b'0' + (fraction / 100) as u8;
            bytes[21] = b'0' + (fraction / 10 % 10) as u8;
            bytes[22] = b'0' + (fraction % 10) as u8;
            let mut end = 23;
            while bytes[end - 1] == b'0' {
                end -= 1;
            }
            bytes[end] = b'Z';
            end + 1
        };
        Ok(Self { bytes, len })
    }
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.len]).expect("RFC3339 output is ASCII")
    }
}
impl std::ops::Deref for Timestamp {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}
pub fn timestamp(ms: u64) -> Result<String> {
    Ok(Timestamp::new(ms)?.as_str().to_owned())
}
pub fn parse_time(s: &str) -> Result<u64> {
    if !s.ends_with('Z') {
        return Err(ApiError::invalid());
    }
    let d = time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
        .map_err(|_| ApiError::invalid())?;
    u64::try_from(d.unix_timestamp_nanos() / 1_000_000).map_err(|_| ApiError::invalid())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_duplicate_fields_at_every_depth() {
        for raw in [
            r#"{"version":1,"version":1,"type":"poll","id":"p","topic":"t"}"#,
            r#"{"version":1,"type":"publish","id":"p","topic":"t","payload":{"x":1,"x":2}}"#,
        ] {
            assert!(parse(raw).is_err());
        }
    }
    #[test]
    fn rejects_unknown_fields_and_wrong_types() {
        for raw in [
            r#"{"version":1,"type":"poll","id":"p","topic":"t","limit":true}"#,
            r#"{"version":1,"type":"poll","id":"p","topic":"t","reply_to":"x"}"#,
            r#"{"version":true,"type":"poll","id":"p","topic":"t"}"#,
        ] {
            assert!(parse(raw).is_err());
        }
    }
    #[test]
    fn rfc3339_is_validated_and_round_trips() {
        let now = 1_800_000_000_123;
        assert_eq!(parse_time(&timestamp(now).unwrap()).unwrap(), now);
        for s in [
            "invalidZ",
            "2026-02-30T12:00:00Z",
            "2026-12-01T12:00:00+01:00",
        ] {
            assert!(parse_time(s).is_err());
        }
    }
}

#[cfg(test)]
mod optional_and_direction_tests {
    use super::*;
    #[test]
    fn null_optional_fields_are_rejected_but_payload_null_is_valid() {
        assert!(parse(r#"{"version":1,"type":"poll","id":"p","topic":"t","after":null}"#).is_err());
        assert!(
            parse(r#"{"version":1,"type":"publish","id":"p","topic":"t","payload":null}"#).is_ok()
        );
    }
    #[test]
    fn distinguishes_unknown_and_server_only_frames() {
        assert_eq!(
            parse(r#"{"version":1,"type":"made_up","id":"x"}"#)
                .unwrap_err()
                .code,
            "unknown_type"
        );
        assert_eq!(
            parse(r#"{"version":1,"type":"result","request_id":"x"}"#)
                .unwrap_err()
                .code,
            "invalid_direction"
        );
    }
}
