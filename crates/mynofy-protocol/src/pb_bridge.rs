use super::pb::*;
use crate::{ApiError, Result};
use serde_json::Value;
impl Frame {
    pub fn kind(&self) -> &'static str {
        match &self.body {
            Some(Body::Poll(_)) => "poll",
            Some(Body::Messages(_)) => "messages",
            Some(Body::Publish(_)) => "publish",
            Some(Body::Result(_)) => "result",
            Some(Body::ListMessages(_)) => "list_messages",
            Some(Body::MessagesPage(_)) => "messages_page",
            Some(Body::GetMessage(_)) => "get_message",
            Some(Body::MessageStatus(_)) => "message_status",
            Some(Body::Error(_)) => "error",
            Some(Body::GetPermissions(_)) => "get_permissions",
            Some(Body::Permissions(_)) => "permissions",
            Some(Body::RotationStart(_)) => "rotation_start",
            Some(Body::RotationOffer(_)) => "rotation_offer",
            Some(Body::RotationConfirm(_)) => "rotation_confirm",
            Some(Body::RotationReady(_)) => "rotation_ready",
            Some(Body::RotationCommit(_)) => "rotation_commit",
            Some(Body::RotationComplete(_)) => "rotation_complete",
            Some(Body::GetCatalog(_)) => "get_catalog",
            Some(Body::Catalog(_)) => "catalog",
            Some(Body::AuthHello(_)) => "auth_hello",
            Some(Body::AuthChallenge(_)) => "auth_challenge",
            Some(Body::AuthProof(_)) => "auth_proof",
            Some(Body::AuthWelcome(_)) => "auth_welcome",
            None => "",
        }
    }
    pub fn from_json(mut value: Value) -> Result<Self> {
        if value["version"] != 1 {
            return Err(ApiError::invalid());
        }
        let kind = value["type"]
            .as_str()
            .ok_or_else(ApiError::invalid)?
            .to_owned();
        if crate::wire::is_client_command(&kind) {
            crate::wire::validate_command_presence(&value)?;
        }
        let map = value.as_object_mut().ok_or_else(ApiError::invalid)?;
        map.remove("version");
        map.remove("type");
        let body = match kind.as_str() {
            "poll" => Body::Poll(serde_json::from_value(value).map_err(|_| ApiError::invalid())?),
            "messages" => {
                Body::Messages(serde_json::from_value(value).map_err(|_| ApiError::invalid())?)
            }
            "publish" => {
                Body::Publish(serde_json::from_value(value).map_err(|_| ApiError::invalid())?)
            }
            "result" => {
                Body::Result(serde_json::from_value(value).map_err(|_| ApiError::invalid())?)
            }
            "list_messages" => {
                Body::ListMessages(serde_json::from_value(value).map_err(|_| ApiError::invalid())?)
            }
            "messages_page" => {
                Body::MessagesPage(serde_json::from_value(value).map_err(|_| ApiError::invalid())?)
            }
            "get_message" => {
                Body::GetMessage(serde_json::from_value(value).map_err(|_| ApiError::invalid())?)
            }
            "message_status" => {
                Body::MessageStatus(serde_json::from_value(value).map_err(|_| ApiError::invalid())?)
            }
            "error" => Body::Error(serde_json::from_value(value).map_err(|_| ApiError::invalid())?),
            "get_permissions" => Body::GetPermissions(
                serde_json::from_value(value).map_err(|_| ApiError::invalid())?,
            ),
            "permissions" => {
                Body::Permissions(serde_json::from_value(value).map_err(|_| ApiError::invalid())?)
            }
            "rotation_start" => {
                Body::RotationStart(serde_json::from_value(value).map_err(|_| ApiError::invalid())?)
            }
            "rotation_offer" => {
                Body::RotationOffer(serde_json::from_value(value).map_err(|_| ApiError::invalid())?)
            }
            "rotation_confirm" => Body::RotationConfirm(
                serde_json::from_value(value).map_err(|_| ApiError::invalid())?,
            ),
            "rotation_ready" => {
                Body::RotationReady(serde_json::from_value(value).map_err(|_| ApiError::invalid())?)
            }
            "rotation_commit" => Body::RotationCommit(
                serde_json::from_value(value).map_err(|_| ApiError::invalid())?,
            ),
            "rotation_complete" => Body::RotationComplete(
                serde_json::from_value(value).map_err(|_| ApiError::invalid())?,
            ),
            "get_catalog" => {
                Body::GetCatalog(serde_json::from_value(value).map_err(|_| ApiError::invalid())?)
            }
            "catalog" => {
                let request_id = value["request_id"]
                    .as_str()
                    .ok_or_else(ApiError::invalid)?
                    .to_owned();
                let definition =
                    serde_json::to_vec(value.get("catalog").ok_or_else(ApiError::invalid)?)
                        .map_err(|_| ApiError::invalid())?;
                Body::Catalog(Catalog {
                    request_id,
                    definition,
                })
            }
            "auth_hello" => {
                Body::AuthHello(serde_json::from_value(value).map_err(|_| ApiError::invalid())?)
            }
            "auth_challenge" => {
                Body::AuthChallenge(serde_json::from_value(value).map_err(|_| ApiError::invalid())?)
            }
            "auth_proof" => {
                Body::AuthProof(serde_json::from_value(value).map_err(|_| ApiError::invalid())?)
            }
            "auth_welcome" => {
                Body::AuthWelcome(serde_json::from_value(value).map_err(|_| ApiError::invalid())?)
            }
            _ => return Err(ApiError::new("unknown_type", "Unknown frame type.")),
        };
        Ok(Self {
            version: 1,
            body: Some(body),
        })
    }
    pub fn to_json(&self) -> Result<Value> {
        let mut value = match &self.body {
            Some(Body::Poll(x)) => serde_json::to_value(x).map_err(|_| ApiError::internal())?,
            Some(Body::Messages(x)) => serde_json::to_value(x).map_err(|_| ApiError::internal())?,
            Some(Body::Publish(x)) => serde_json::to_value(x).map_err(|_| ApiError::internal())?,
            Some(Body::Result(x)) => serde_json::to_value(x).map_err(|_| ApiError::internal())?,
            Some(Body::ListMessages(x)) => {
                serde_json::to_value(x).map_err(|_| ApiError::internal())?
            }
            Some(Body::MessagesPage(x)) => {
                serde_json::to_value(x).map_err(|_| ApiError::internal())?
            }
            Some(Body::GetMessage(x)) => {
                serde_json::to_value(x).map_err(|_| ApiError::internal())?
            }
            Some(Body::MessageStatus(x)) => {
                serde_json::to_value(x).map_err(|_| ApiError::internal())?
            }
            Some(Body::Error(x)) => serde_json::to_value(x).map_err(|_| ApiError::internal())?,
            Some(Body::GetPermissions(x)) => {
                serde_json::to_value(x).map_err(|_| ApiError::internal())?
            }
            Some(Body::Permissions(x)) => {
                serde_json::to_value(x).map_err(|_| ApiError::internal())?
            }
            Some(Body::RotationStart(x)) => {
                serde_json::to_value(x).map_err(|_| ApiError::internal())?
            }
            Some(Body::RotationOffer(x)) => {
                serde_json::to_value(x).map_err(|_| ApiError::internal())?
            }
            Some(Body::RotationConfirm(x)) => {
                serde_json::to_value(x).map_err(|_| ApiError::internal())?
            }
            Some(Body::RotationReady(x)) => {
                serde_json::to_value(x).map_err(|_| ApiError::internal())?
            }
            Some(Body::RotationCommit(x)) => {
                serde_json::to_value(x).map_err(|_| ApiError::internal())?
            }
            Some(Body::RotationComplete(x)) => {
                serde_json::to_value(x).map_err(|_| ApiError::internal())?
            }
            Some(Body::GetCatalog(x)) => {
                serde_json::to_value(x).map_err(|_| ApiError::internal())?
            }
            Some(Body::Catalog(c)) => {
                serde_json::json!({"request_id":c.request_id,"catalog":crate::strict_json(std::str::from_utf8(&c.definition).map_err(|_|ApiError::invalid())?)?})
            }
            Some(Body::AuthHello(x)) => {
                serde_json::to_value(x).map_err(|_| ApiError::internal())?
            }
            Some(Body::AuthChallenge(x)) => {
                serde_json::to_value(x).map_err(|_| ApiError::internal())?
            }
            Some(Body::AuthProof(x)) => {
                serde_json::to_value(x).map_err(|_| ApiError::internal())?
            }
            Some(Body::AuthWelcome(x)) => {
                serde_json::to_value(x).map_err(|_| ApiError::internal())?
            }
            None => return Err(ApiError::invalid()),
        };
        value["version"] = self.version.into();
        value["type"] = self.kind().into();
        Ok(value)
    }
    pub fn error(error: ApiError, id: Option<String>) -> Self {
        Self {
            version: 1,
            body: Some(Body::Error(Error {
                request_id: id,
                code: error.code.into(),
                detail: error.detail.into(),
                retry_after_ms: error.retry_after_ms,
            })),
        }
    }
}

impl Frame {
    pub fn request_id(&self) -> Option<&str> {
        match &self.body {
            Some(Body::Poll(x)) => Some(&x.id),
            Some(Body::Messages(x)) => Some(&x.request_id),
            Some(Body::Publish(x)) => Some(&x.id),
            Some(Body::Result(x)) => Some(&x.request_id),
            Some(Body::ListMessages(x)) => Some(&x.id),
            Some(Body::MessagesPage(x)) => Some(&x.request_id),
            Some(Body::GetMessage(x)) => Some(&x.id),
            Some(Body::MessageStatus(x)) => Some(&x.request_id),
            Some(Body::Error(x)) => x.request_id.as_deref(),
            Some(Body::GetPermissions(x)) => Some(&x.id),
            Some(Body::Permissions(x)) => Some(&x.request_id),
            Some(Body::RotationStart(x)) => Some(&x.id),
            Some(Body::RotationOffer(x)) => Some(&x.request_id),
            Some(Body::RotationConfirm(x)) => Some(&x.id),
            Some(Body::RotationReady(x)) => Some(&x.request_id),
            Some(Body::RotationCommit(x)) => Some(&x.id),
            Some(Body::RotationComplete(x)) => Some(&x.request_id),
            Some(Body::GetCatalog(x)) => Some(&x.id),
            Some(Body::Catalog(x)) => Some(&x.request_id),
            Some(Body::AuthHello(x)) => Some(&x.id),
            Some(Body::AuthChallenge(x)) => Some(&x.request_id),
            Some(Body::AuthProof(x)) => Some(&x.id),
            Some(Body::AuthWelcome(x)) => Some(&x.request_id),
            None => None,
        }
    }
}
