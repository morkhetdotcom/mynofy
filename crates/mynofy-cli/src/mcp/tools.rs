use super::*;
fn object(properties: Value, required: &[&str]) -> Value {
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}
fn topic() -> Value {
    json!({"type":"string","minLength":1,"maxLength":256})
}
pub(super) fn list() -> Value {
    definitions().clone()
}
fn definitions() -> &'static Value {
    static DEFINITIONS: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    DEFINITIONS.get_or_init(|| {
    let definitions = [
        (
            "mynofy_connection_info",
            "Authenticate and report the configured owner, client and restart counter. No credentials are returned.",
            object(json!({}), &[]),
            true,
            true,
        ),
        (
            "mynofy_publish",
            "Publish once to an existing topic. Payload is existing mynofy JSON/event/bytes syntax, at most 4096 data bytes. No automatic retry; a response is not job completion.",
            object(
                json!({"topic":topic(),"payload":{},"expires":{"type":"object","properties":{"time":{"type":"string","format":"date-time"}},"required":["time"],"additionalProperties":false}}),
                &["topic", "payload"],
            ),
            false,
            false,
        ),
        (
            "mynofy_poll",
            "Read one bounded page immediately. Pass next_cursor as after on the next call; do not advance business state implicitly. Follow the workflow polling interval.",
            object(
                json!({"topic":topic(),"after":{"type":"string","minLength":8,"maxLength":8},"limit":{"type":"integer","minimum":1,"maximum":100}}),
                &["topic"],
            ),
            true,
            true,
        ),
        (
            "mynofy_list_messages",
            "List this producer's own active original messages and available status. Missing messages do not prove success or non-acceptance.",
            object(
                json!({"topic":topic(),"limit":{"type":"integer","minimum":1,"maximum":100},"cursor":{"type":"string"}}),
                &[],
            ),
            true,
            true,
        ),
        (
            "mynofy_get_message",
            "Read this producer's own active original message and available status by owner-local message ID.",
            object(
                json!({"message_id":{"type":"string","minLength":8,"maxLength":8}}),
                &["message_id"],
            ),
            true,
            true,
        ),
        (
            "mynofy_get_permissions",
            "Query current rights for this authenticated client on an exact topic. This snapshot does not grant rights.",
            object(json!({"topic":topic()}), &["topic"]),
            true,
            true,
        ),
        (
            "mynofy_get_catalog",
            "Fetch the built-in event catalog. This is not a workflow profile or execution authorization.",
            object(
                json!({"name":{"type":"string","default":"mynofy"},"catalog_version":{"type":"integer","minimum":1,"maximum":u32::MAX,"default":1}}),
                &[],
            ),
            true,
            true,
        ),
    ];
    let tools:Vec<_>=definitions.into_iter().map(|(name,description,input,read,idempotent)|json!({"name":name,"description":description,"inputSchema":input,"annotations":{"readOnlyHint":read,"destructiveHint":false,"idempotentHint":idempotent,"openWorldHint":true}})).collect();
    json!({"tools":tools})
    })
}
pub(super) fn command(name: &str, arguments: &Value) -> Result<Option<Value>, &'static str> {
    let Some(fields) = arguments.as_object() else {
        return Err("Tool arguments must be an object");
    };
    let definition = definitions()["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|tool| tool["name"] == name)
        .ok_or("Unknown tool")?;
    let schema = &definition["inputSchema"];
    if fields
        .keys()
        .any(|key| schema["properties"].get(key).is_none())
        || schema["required"]
            .as_array()
            .expect("required")
            .iter()
            .any(|name| !fields.contains_key(name.as_str().expect("field")))
    {
        return Err("Unknown or missing tool argument");
    }
    for (key, value) in fields {
        let field = &schema["properties"][key];
        match field["type"].as_str() {
            Some("string") => {
                let Some(text) = value.as_str() else {
                    return Err("Expected string argument");
                };
                if field["minLength"]
                    .as_u64()
                    .is_some_and(|n| text.len() < (n as usize))
                    || field["maxLength"]
                        .as_u64()
                        .is_some_and(|n| text.len() > (n as usize))
                {
                    return Err("String argument out of bounds");
                }
            }
            Some("integer") => {
                let Some(n) = value.as_u64() else {
                    return Err("Expected unsigned integer argument");
                };
                if field["minimum"].as_u64().is_some_and(|min| n < min)
                    || field["maximum"].as_u64().is_some_and(|max| n > max)
                {
                    return Err("Integer argument out of bounds");
                }
            }
            Some("object") if !value.is_object() => return Err("Expected object argument"),
            _ => {}
        }
    }
    if name == "mynofy_connection_info" {
        return Ok(None);
    }
    let method = name.strip_prefix("mynofy_").ok_or("Unknown tool")?;
    let mut request = arguments.clone();
    request["version"] = 1.into();
    request["type"] = method.into();
    request["id"] = "mcp-validate".into();
    if method == "get_catalog" {
        if request.get("name").is_none() {
            request["name"] = "mynofy".into();
        }
        if request.get("catalog_version").is_none() {
            request["catalog_version"] = 1.into();
        }
    }

    if let Some(payload) = request.get_mut("payload") {
        input_event(payload)?;
    }
    protocol::parse_value(request.clone()).map_err(|_| "Invalid mynofy command arguments")?;
    let frame =
        protocol::pb::Frame::from_json(request.clone()).map_err(|_| "Invalid mynofy payload")?;
    if let Some(protocol::pb::Body::Publish(publish)) = frame.body {
        publish
            .payload
            .validate()
            .map_err(|_| "Invalid or oversized payload")?;
        if let Some(expires) = publish.expires {
            protocol::parse_time(&expires.time).map_err(|_| "Invalid expiry timestamp")?;
        }
    }
    Ok(Some(request))
}

fn input_event(payload: &mut Value) -> Result<(), &'static str> {
    if payload["kind"] != "event" {
        return Ok(());
    }
    if let Some(parameters) = payload.get_mut("parameters").and_then(Value::as_array_mut) {
        for parameter in parameters {
            for key in ["u64", "i64"] {
                if let Some(value) = parameter.get_mut(key) {
                    if let Some(text) = value.as_str() {
                        let number = if key == "u64" {
                            let n = text.parse::<u64>().map_err(|_| "Invalid u64 parameter")?;
                            if n.to_string() != text {
                                return Err("Non-canonical u64 parameter");
                            }
                            Value::from(n)
                        } else {
                            let n = text.parse::<i64>().map_err(|_| "Invalid i64 parameter")?;
                            if n.to_string() != text {
                                return Err("Non-canonical i64 parameter");
                            }
                            Value::from(n)
                        };
                        *value = number;
                    } else if value.as_u64().is_some_and(|n| n > 9_007_199_254_740_991)
                        || value.as_i64().is_some_and(|n| n < -9_007_199_254_740_991)
                    {
                        return Err("Use decimal strings for large 64-bit event parameters");
                    }
                }
            }
        }
    }
    Ok(())
}
pub(super) fn lossless_output(value: &mut Value) {
    match value {
        Value::Array(items) => {
            for item in items {
                lossless_output(item)
            }
        }
        Value::Object(fields) => {
            for (key, value) in fields {
                if key == "payload" {
                    // Payload data is opaque, including event-shaped legacy JSON.
                } else {
                    lossless_output(value)
                }
            }
        }
        _ => {}
    }
}

/// Convert event integers using the actual wire variant, before losing its type.
pub(super) fn frame_output(
    frame: &mynofy_protocol::pb::Frame,
) -> Result<Value, mynofy_protocol::BoxError> {
    use mynofy_protocol::{
        payload::{Body, Payload},
        pb,
    };
    fn event(payload: &Payload, value: &mut Value) {
        if matches!(payload.body, Some(Body::Event(_)))
            && let Some(parameters) = value.get_mut("parameters").and_then(Value::as_array_mut)
        {
            for parameter in parameters {
                for name in ["u64", "i64"] {
                    if let Some(number) = parameter.get_mut(name)
                        && number.is_number()
                    {
                        *number = Value::String(number.to_string());
                    }
                }
            }
        }
    }
    let mut value = frame.to_json()?;
    match &frame.body {
        Some(pb::Body::Messages(page)) => {
            for (item, json) in page.items.iter().zip(
                value["items"]
                    .as_array_mut()
                    .ok_or("Invalid messages page")?,
            ) {
                event(&item.message.payload, &mut json["message"]["payload"]);
            }
        }
        Some(pb::Body::MessageStatus(status)) => {
            event(&status.message.payload, &mut value["message"]["payload"])
        }
        _ => {}
    }
    Ok(value)
}
