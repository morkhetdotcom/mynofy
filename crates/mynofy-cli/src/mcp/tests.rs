use super::*;
fn message(value: Value) -> Vec<u8> {
    serde_json::to_vec(&value).unwrap()
}
fn modern(id: u32, method: &str, mut params: Value) -> Vec<u8> {
    params["_meta"] = json!({VERSION_KEY:MODERN,CAPS_KEY:{}});
    message(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
}
fn reply(action: Action) -> Value {
    match action {
        Action::Reply(v) => v,
        _ => panic!("Expected reply"),
    }
}
#[test]
fn legacy_initialization_and_modern_metadata_never_enable_uninitialized_tool_notifications() {
    let mut s = Session::default();
    assert_eq!(
        reply(s.prepare(&message(
            json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})
        )))["error"]["code"],
        -32602
    );
    assert!(matches!(s.prepare(&message(json!({"jsonrpc":"2.0","method":"tools/call","params":{"name":"mynofy_publish","arguments":{"topic":"x","payload":"bad"}}}))),Action::None));
    let hello = message(
        json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":LEGACY,"clientInfo":{"name":"test","version":"1"},"capabilities":{}}}),
    );
    assert_eq!(
        reply(s.prepare(&hello))["result"]["protocolVersion"],
        LEGACY
    );
    s.prepare(&message(
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    ));
    assert_eq!(
        reply(s.prepare(&message(
            json!({"jsonrpc":"2.0","id":3,"method":"tools/list"})
        )))["result"]["tools"]
            .as_array()
            .unwrap()
            .len(),
        7
    );
    let mut fresh = Session::default();
    let discover = reply(fresh.prepare(&modern(4, "server/discover", json!({}))));
    assert_eq!(discover["result"]["resultType"], "complete");
    assert_eq!(
        reply(fresh.prepare(&modern(5, "tools/list", json!({}))))["result"]["tools"]
            .as_array()
            .unwrap()
            .len(),
        7
    );
    let unsupported = message(
        json!({"jsonrpc":"2.0","id":6,"method":"tools/list","params":{"_meta":{VERSION_KEY:"2099-01-01",CAPS_KEY:{}}}}),
    );
    assert_eq!(reply(fresh.prepare(&unsupported))["error"]["code"], -32022);
    assert_eq!(reply(fresh.prepare(&message(json!({"jsonrpc":"2.0","id":7,"method":"tools/list","params":{"_meta":{VERSION_KEY:MODERN}}}))))["error"]["code"],-32602);
}
#[test]
fn malformed_rpc_and_unsupported_tools_cannot_escape_the_client_surface() {
    let mut s = Session::default();
    for text in ["{", r#"{"jsonrpc":"2.0","id":1,"id":2,"method":"ping"}"#] {
        assert_eq!(reply(s.prepare(text.as_bytes()))["error"]["code"], -32700);
    }
    for id in [Value::Null, json!(1.5), json!([])] {
        assert_eq!(
            reply(s.prepare(&message(json!({"jsonrpc":"2.0","id":id,"method":"ping"}))))["error"]["code"],
            -32600
        );
    }
    for name in ["admin", "mynofy_topic_create", "mynofy_rotation_start"] {
        assert!(tools::command(name, &json!({})).is_err());
    }
    for args in [
        json!({"topic":"x","payload":"ok","key":"secret"}),
        json!({"topic":"x"}),
        json!({"topic":"x","payload":"x".repeat(4097)}),
        json!({"topic":"x","payload":"ok","expires":{"time":"tomorrow"}}),
    ] {
        assert!(tools::command("mynofy_publish", &args).is_err());
    }
}
#[test]
fn event_u64_values_roundtrip_without_javascript_precision_loss() {
    let event = json!({"topic":"x","payload":{"kind":"event","catalog":1,"catalog_version":1,"category":12,"action":40,"status":7,"parameters":[{"u64":u64::MAX.to_string()}]}});
    assert_eq!(tools::command("mynofy_publish",&event).unwrap().unwrap()["payload"]["parameters"][0]["u64"].as_u64(),Some(u64::MAX));
    let mut legacy = json!({"payload":{"delivery_id":u64::MAX}});
    let original = legacy.clone();
    tools::lossless_output(&mut legacy);
    assert_eq!(legacy, original);
}
#[tokio::test]
async fn bounded_line_reader_drains_oversized_input_and_preserves_the_next_message() {
    let bytes = [
        vec![b'x'; MAX_INPUT + 100],
        b"\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n".to_vec(),
    ]
    .concat();
    let mut input = BufReader::new(bytes.as_slice());
    assert!(line(&mut input).await.unwrap().unwrap().is_err());
    let next = line(&mut input).await.unwrap().unwrap().unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&next).unwrap()["method"],
        "ping"
    );
    assert!(line(&mut input).await.unwrap().is_none());
}

#[test]
fn typed_event_output_never_reinterprets_event_shaped_legacy_data() {
    use mynofy_protocol::{
        payload::{Body, Payload},
        pb,
    };
    let opaque =
        json!({"kind":"event","catalog":999,"parameters":[{"u64":7}],"note":"opaque legacy JSON"});
    let mut frame = pb::Frame {
        version: 1,
        body: Some(pb::Body::MessageStatus(pb::MessageStatus {
            request_id: "get".into(),
            created_at: "2026-11-14T00:00:00Z".into(),
            message: pb::StoredMessage {
                id: "00000001".into(),
                topic: "t".into(),
                producer_id: "client:owner".into(),
                expires: pb::Expires {
                    time: "2030-01-01T00:00:00Z".into(),
                },
                payload: Payload {
                    body: Some(Body::Data(serde_json::to_vec(&opaque).unwrap().into())),
                    content_type: Some("application/vnd.mynofy.legacy+json".into()),
                },
            },
        })),
    };
    let mut output = tools::frame_output(&frame).unwrap();
    tools::lossless_output(&mut output);
    assert_eq!(output["message"]["payload"], opaque);
    let Some(pb::Body::MessageStatus(status)) = &mut frame.body else {
        unreachable!()
    };
    status.message.payload = Payload::from_json(json!({"kind":"event","catalog":1,"catalog_version":1,"category":12,"action":40,"status":7,"parameters":[{"u64":u64::MAX},{"i64":i64::MIN}]})).unwrap();
    let output = tools::frame_output(&frame).unwrap();
    assert_eq!(
        output["message"]["payload"]["parameters"][0]["u64"],
        u64::MAX.to_string()
    );
    assert_eq!(
        output["message"]["payload"]["parameters"][1]["i64"],
        i64::MIN.to_string()
    );
}

#[test]
fn modern_version_errors_distinguish_invalid_types_from_unsupported_strings() {
    let mut session = Session::default();
    for method in ["server/discover", "tools/list", "tools/call"] {
        for version in [
            Value::Null,
            json!(17),
            json!(1.5),
            json!(true),
            json!([]),
            json!({}),
        ] {
            let input = message(
                json!({"jsonrpc":"2.0","id":17,"method":method,"params":{"_meta":{VERSION_KEY:version,CAPS_KEY:{}}}}),
            );
            let response = reply(session.prepare(&input));
            assert_eq!(response["error"]["code"], -32602, "{response}");
            assert_eq!(response["id"], 17);
            assert!(response["error"].get("data").is_none());
            assert!(matches!(session.phase, Phase::New));
        }
    }
    for version in ["", "1900-01-01", "2099-01-01"] {
        let input = message(
            json!({"jsonrpc":"2.0","id":18,"method":"server/discover","params":{"_meta":{VERSION_KEY:version,CAPS_KEY:{}}}}),
        );
        let response = reply(session.prepare(&input));
        assert_eq!(response["error"]["code"], -32022);
        assert_eq!(
            response["error"]["data"]["requested"].as_str(),
            Some(version)
        );
        assert!(
            response["error"]["data"]["supported"]
                .as_array()
                .unwrap()
                .iter()
                .all(Value::is_string)
        );
    }
    let response = reply(session.prepare(&modern(19, "server/discover", json!({}))));
    assert_eq!(response["result"]["resultType"], "complete");
}
