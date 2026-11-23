//! Serial JSON/protobuf core benchmark without additional benchmark libraries; excludes transport.
#![forbid(unsafe_code)]
use mynofy_server::{
    BoxError,
    config::{Action, Client, Config, PublicKey, Rule},
    crypto,
    engine::Engine,
    protocol,
};
use protocol::{
    payload::{Event, Parameter, ParameterValue, Payload},
    pb,
};
use serde_json::json;
use std::time::Instant;

mod support;
use support::*;

async fn run(
    messages: usize,
    payload_bytes: usize,
    encoding: &str,
    kind: &str,
    output_mode: &str,
) -> Result<(), BoxError> {
    let key = crypto::generate_key()?;
    let public = crypto::public_key(&key)?;
    let client_id = crypto::initial_client_id(&public);
    let mut config: Config =
        serde_json::from_str(include_str!("../../../config/scenarios/local.json"))?;
    config.clients = vec![Client {
        disabled: false,
        client_id: client_id.clone(),
        display_name: None,
        keys: vec![PublicKey {
            algorithm: "ed25519".into(),
            public_key: crypto::encode(&public),
            valid_until: protocol::timestamp(NOW + 86_400_000)?,
        }],
    }];
    config.topics[0].acl = Some(vec![Rule {
        client_id,
        allow: vec![Action::Publish, Action::Subscribe, Action::InspectOwn],
    }]);
    config.limits.client_burst = 1_000_000;
    config.limits.client_requests_per_second = 1_000_000;
    config.limits.max_active_messages = messages;
    config.limits.max_active_message_bytes = (messages * (payload_bytes + 4096)).max(65536);
    let ttl = config.topics[0]
        .policy
        .as_ref()
        .unwrap()
        .default_ttl_seconds;
    let mut engine = Engine::new(config, NOW)?;
    engine.start_id_allocator();
    let session = authenticate(&mut engine, &key);
    let mut output = (output_mode == "direct").then(|| {
        mynofy_server::output::ResponseBuffer::new(engine.config.limits.max_response_bytes)
    });
    let payload = if kind.starts_with("event") {
        Payload::event(Event {
            catalog: 1,
            catalog_version: 1,
            category: 12,
            action: 10,
            status: 7,
            parameters: if kind == "event-empty" {
                Vec::new()
            } else {
                vec![
                    Parameter {
                        value: Some(ParameterValue::U64(4711)),
                    },
                    Parameter {
                        value: Some(ParameterValue::U32(1)),
                    },
                    Parameter {
                        value: Some(ParameterValue::Bool(true)),
                    },
                ]
            },
        })
    } else {
        Payload::bytes(vec![b'x'; payload_bytes], None)
    };
    let publish = frame(pb::Body::Publish(pb::Publish {
        id: "pub".into(),
        topic: "local/events".into(),
        payload,
        expires: None,
    }));
    let raw_publish = encode_request(publish, encoding);
    println!(
        "{}",
        json!({"phase":"wire_size","publish_request_bytes":raw_publish.len()})
    );
    let mut publish_samples = Vec::with_capacity(messages);
    let mut poll_samples = Vec::with_capacity(messages);
    let mut empty_samples = Vec::with_capacity(1000);
    // Fault benchmark sample pages in before the RSS baseline: do not attribute
    // the driver's sample storage to retained server messages.
    publish_samples.resize(messages, 1);
    publish_samples.clear();
    poll_samples.resize(messages, 1);
    poll_samples.clear();
    empty_samples.resize(1000, 1);
    empty_samples.clear();
    // One immutable prepared Publish template is deliberately reused; no generator in the loop.
    let rss_baseline = rss_bytes();
    for i in 0..messages {
        request(
            &mut engine,
            session,
            &raw_publish,
            &mut publish_samples,
            encoding,
            output.as_mut(),
        )
        .check_result();
        // Model the listener yielding to the runtime; allow bounded RID refill.
        if i % 256 == 255 {
            tokio::task::yield_now().await;
        }
    }
    let rss_active = rss_bytes();
    report("publish", &mut publish_samples, messages);
    println!(
        "{}",
        json!({"phase":"storage_layout","storage":engine.storage_stats()})
    );
    let mut cursor: Option<String> = None;
    let mut received = 0;
    loop {
        let raw_poll = encode_request(
            frame(pb::Body::Poll(pb::Poll {
                id: "poll".into(),
                topic: "local/events".into(),
                limit: Some(100),
                after: cursor.take(),
            })),
            encoding,
        );
        let response = request(
            &mut engine,
            session,
            &raw_poll,
            &mut poll_samples,
            encoding,
            output.as_mut(),
        );
        let (deliveries, next_cursor, has_more) = response.page();
        received += deliveries.len();
        cursor = Some(next_cursor);

        if !has_more {
            break;
        }
    }
    assert_eq!(received, messages);
    report("poll", &mut poll_samples, received);
    let raw_empty = encode_request(
        frame(pb::Body::Poll(pb::Poll {
            id: "empty".into(),
            topic: "local/events".into(),
            after: cursor,
            limit: Some(100),
        })),
        encoding,
    );
    for _ in 0..1000 {
        let value = request(
            &mut engine,
            session,
            &raw_empty,
            &mut empty_samples,
            encoding,
            output.as_mut(),
        );
        assert!(value.page().0.is_empty());
    }
    report("poll_at_head", &mut empty_samples, 0);
    let rss_after_poll = rss_bytes();
    println!(
        "{}",
        json!({"phase":"storage_after_poll","storage":engine.storage_stats()})
    );
    let start = Instant::now();
    engine.sweep(NOW + ttl * 1000);
    let sweep_ms = start.elapsed().as_secs_f64() * 1000.0;
    let rss_after_expiry = rss_bytes();
    let reclaim_start = Instant::now();
    for _ in 0..10_000 {
        let progress = engine.maintenance(NOW + ttl * 1000);
        if !progress.more && engine.storage_stats().record_capacity == 0 {
            break;
        }
    }
    let reclaim_ms = reclaim_start.elapsed().as_secs_f64() * 1000.0;
    let rss_after_reclaim = rss_bytes();
    println!(
        "{}",
        json!({"phase":"storage_after_reclaim","storage":engine.storage_stats(),"rss_bytes":rss_after_reclaim,"reclaim_ms":reclaim_ms})
    );
    let expired = engine.handle(
        session,
        &json!({"version":1,"type":"get_message","id":"expired","message_id":"00000000"})
            .to_string(),
        NOW + ttl * 1000,
    );
    assert_eq!(expired["code"], "not_found");
    println!(
        "{}",
        json!({"phase":"memory_and_expiry", "rss_baseline_bytes":rss_baseline,"rss_active_bytes":rss_active,"rss_after_poll_bytes":rss_after_poll,"rss_after_expiry_bytes":rss_after_expiry,"rss_after_reclaim_bytes":rss_after_reclaim,"reclaim_ms":reclaim_ms,"rss_active_delta_per_message":rss_active.zip(rss_baseline).map(|(a,b)|a.saturating_sub(b) as f64/messages as f64),"expiry_sweep_ms":sweep_ms,"expiry_scope":"first_bounded_request_step"})
    );
    Ok(())
}
fn main() -> Result<(), BoxError> {
    let mut messages = 10_000;
    let mut payload_bytes = 256;
    let mut encoding = "protobuf".to_owned();
    let mut kind = "bytes".to_owned();
    let mut output_mode = "direct".to_owned();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--output-mode" => output_mode = args.next().ok_or("Missing output mode")?,
            "--encoding" => encoding = args.next().ok_or("Missing encoding")?,
            "--payload-kind" => kind = args.next().ok_or("Missing payload kind")?,
            "--messages" => messages = args.next().ok_or("Missing --messages value")?.parse()?,
            "--payload-bytes" => {
                payload_bytes = args
                    .next()
                    .ok_or("Missing --payload-bytes value")?
                    .parse()?
            }
            "--bench" => {} // Cargo adds this for custom benchmark harnesses.
            _ => return Err(format!("Unknown argument: {arg}").into()),
        }
    }
    if !(1..=100_000).contains(&messages) || !(1..=4096).contains(&payload_bytes) {
        return Err("Use --messages 1..100000 and --payload-bytes 1..4096".into());
    }
    if !["json", "protobuf"].contains(&encoding.as_str())
        || !["bytes", "event", "event-empty"].contains(&kind.as_str())
    {
        return Err("Invalid encoding or payload kind".into());
    }
    if !["typed", "direct"].contains(&output_mode.as_str()) {
        return Err("Invalid output mode".into());
    }
    println!(
        "{}",
        json!({"output_mode":output_mode,"encoding":encoding,"payload_kind":kind,"phase":"configuration","messages":messages,"payload_bytes":payload_bytes,"benchmark_version":2,"timing":"input_decode_core_response_encode_only","request_generation_timed":false,"driver_json_view":false,"runtime":"current_thread","transport":"none","warmup":"none","debug_assertions":cfg!(debug_assertions),"os":std::env::consts::OS,"arch":std::env::consts::ARCH})
    );
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(run(messages, payload_bytes, &encoding, &kind, &output_mode))
}
