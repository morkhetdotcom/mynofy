use super::*;
use crate::config::*;
use serde_json::json;
const NOW: u64 = 1_800_000_000_000;
struct Fixture {
    cfg: Config,
    keys: Vec<Vec<u8>>,
    ids: Vec<String>,
}
fn fixture() -> Fixture {
    let keys: Vec<_> = (0..2).map(|_| crypto::generate_key().unwrap()).collect();
    let ids: Vec<_> = keys
        .iter()
        .map(|k| crypto::initial_client_id(&crypto::public_key(k).unwrap()))
        .collect();
    let mut cfg: Config =
        serde_json::from_str(include_str!("../../../config/scenarios/local.json")).unwrap();
    cfg.clients = keys
        .iter()
        .zip(&ids)
        .map(|(k, id)| Client {
            disabled: false,
            client_id: id.clone(),
            display_name: None,
            keys: vec![PublicKey {
                algorithm: "ed25519".into(),
                public_key: crypto::encode(&crypto::public_key(k).unwrap()),
                valid_until: protocol::timestamp(NOW + 86_400_000).unwrap(),
            }],
        })
        .collect();
    cfg.topics[0].acl = Some(
        ids.iter()
            .map(|id| Rule {
                client_id: id.clone(),
                allow: vec![Action::Publish, Action::Subscribe, Action::InspectOwn],
            })
            .collect(),
    );
    let mut second = cfg.topics[0].clone();
    second.name = "other".into();
    cfg.topics.push(second);
    Fixture { cfg, keys, ids }
}
fn send(e: &mut Engine, s: u64, v: Value, now: u64) -> Value {
    e.handle(s, &v.to_string(), now)
}
fn auth(e: &mut Engine, key: &[u8], now: u64) -> u64 {
    let s = e.connect(now).unwrap();
    let v = send(
        e,
        s,
        json!({"version":1,"type":"auth_hello","id":"h","token_id":crypto::token_id(&crypto::public_key(key).unwrap())}),
        now,
    );
    assert_eq!(v["type"], "auth_challenge", "{v}");
    let n = crypto::decode(v["nonce"].as_str().unwrap()).unwrap();
    let public = crypto::public_key(key).unwrap();
    let sig = crypto::sign(key, &crypto::auth_bytes(&n, &public, &e.config.service.id)).unwrap();
    let r = send(
        e,
        s,
        json!({"version":1,"type":"auth_proof","id":"p","auth_id":"h","signature":sig}),
        now,
    );
    assert_eq!(r["type"], "auth_welcome", "{r}");
    assert_eq!(r["restart_counter"], e.restart_counter);
    s
}
fn publish(e: &mut Engine, s: u64, value: Value, now: u64) -> Value {
    send(
        e,
        s,
        json!({"version":1,"type":"publish","id":"pub","topic":"local/events","payload":value}),
        now,
    )
}
fn poll(e: &mut Engine, s: u64, after: Option<&str>, limit: usize, now: u64) -> Value {
    let mut v = json!({"version":1,"type":"poll","id":"poll","topic":"local/events","limit":limit});
    if let Some(c) = after {
        v["after"] = c.into();
    }
    send(e, s, v, now)
}
fn get(e: &mut Engine, s: u64, id: &str, now: u64) -> Value {
    send(
        e,
        s,
        json!({"version":1,"type":"get_message","id":"get","message_id":id}),
        now,
    )
}
fn rotation(e: &mut Engine, s: u64, now: u64) -> (Value, Value, [u8; 32]) {
    let offer = send(
        e,
        s,
        json!({"version":1,"type":"rotation_start","id":"start"}),
        now,
    );
    assert_eq!(offer["type"], "rotation_offer");
    let secret = crypto::random::<32>().unwrap();
    let ready = send(
        e,
        s,
        json!({"version":1,"type":"rotation_confirm","id":"confirm","rotation_id":offer["rotation_id"],"shared_secret":crypto::encode(&secret)}),
        now,
    );
    assert_eq!(ready["type"], "rotation_ready");
    assert!(ready.get("shared_secret").is_none());
    (offer, ready, secret)
}
fn commit_request(
    e: &Engine,
    client: &str,
    old: &[u8],
    new: &[u8],
    offer: &Value,
    ready: &Value,
    secret: &[u8; 32],
) -> Value {
    let pubkey = crypto::public_key(new).unwrap();
    let nonce = crypto::decode(ready["nonce"].as_str().unwrap()).unwrap();
    let challenge = crypto::decode(ready["challenge"].as_str().unwrap()).unwrap();
    let rid = offer["rotation_id"].as_str().unwrap();
    let old_token = crypto::token_id(&crypto::public_key(old).unwrap());
    let until = protocol::parse_time(ready["new_valid_until"].as_str().unwrap()).unwrap();
    let aad = rotation_aad(&e.config.service.id, client, rid, &old_token, until);
    assert_eq!(ready["aad"], crypto::encode(&aad));
    let cipher = crypto::encrypt_rotation(secret, nonce, &aad, &pubkey).unwrap();
    let signature = crypto::sign(new, &rotation_proof_bytes(&aad, &pubkey, &challenge)).unwrap();
    json!({"version":1,"type":"rotation_commit","id":"commit","rotation_id":rid,"encrypted_public_key":cipher,"signature":signature})
}

#[test]
fn empty_polls_do_not_allocate_deliveries_and_cursor_reconnects() {
    let f = fixture();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let s = auth(&mut e, &f.keys[0], NOW);
    let empty = poll(&mut e, s, None, 20, NOW);
    let cursor = empty["next_cursor"].as_str().unwrap();
    e.disconnect(s);
    let s = auth(&mut e, &f.keys[0], NOW);
    for _ in 0..10 {
        assert_eq!(poll(&mut e, s, Some(cursor), 20, NOW)["items"], json!([]));
    }
    publish(&mut e, s, json!(1), NOW);
    assert_eq!(
        poll(&mut e, s, Some(cursor), 20, NOW)["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn cursor_survives_expired_anchor_and_completely_empty_topic() {
    let mut f = fixture();
    f.cfg.topics[0].policy.as_mut().unwrap().default_ttl_seconds = 1;
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let s = auth(&mut e, &f.keys[0], NOW);
    publish(&mut e, s, json!(1), NOW);
    let page = poll(&mut e, s, None, 20, NOW);
    let cursor = page["next_cursor"].as_str().unwrap();
    let empty = poll(&mut e, s, Some(cursor), 20, NOW + 1001);
    assert_eq!(empty["items"], json!([]));
    assert_eq!(e.topics["local/events"].head, 1);
    publish(&mut e, s, json!(2), NOW + 1002);
    let new = poll(&mut e, s, Some(cursor), 20, NOW + 1003);
    assert_eq!(new["items"][0]["message"]["payload"], 2);
    assert_eq!(e.index.len(), 1);
    assert_eq!(e.own[&f.ids[0]].len(), 1);
}
#[test]
fn compact_cursor_is_a_position_with_acl_and_range_checks() {
    let f = fixture();
    let mut e = Engine::new(f.cfg.clone(), NOW).unwrap();
    let s = auth(&mut e, &f.keys[0], NOW);
    let other = auth(&mut e, &f.keys[1], NOW);
    publish(&mut e, s, json!(1), NOW);
    let page = poll(&mut e, s, None, 20, NOW);
    let cursor = page["next_cursor"].as_str().unwrap();
    assert_eq!(cursor, "00000001");
    assert_eq!(
        poll(&mut e, other, Some(cursor), 20, NOW)["items"],
        json!([])
    );
    let wrong = send(
        &mut e,
        s,
        json!({"version":1,"type":"poll","id":"p","topic":"other","after":cursor}),
        NOW,
    );
    assert_eq!(wrong["type"], "messages");
    assert_eq!(wrong["items"], json!([])); // future position resets to the active beginning
    for invalid in ["00000002", "0000000!", "0000000", "000000001"] {
        assert_eq!(
            poll(&mut e, s, Some(invalid), 20, NOW)["items"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
    e.set_acl(&f.ids[1], "local/events", vec![]).unwrap();
    let other = auth(&mut e, &f.keys[1], NOW);
    assert_eq!(
        poll(&mut e, other, Some(cursor), 20, NOW)["code"],
        "forbidden"
    );
    let mut restarted = Engine::new(f.cfg, NOW).unwrap();
    let rs = auth(&mut restarted, &f.keys[0], NOW);
    assert_eq!(
        poll(&mut restarted, rs, Some(cursor), 20, NOW)["items"],
        json!([])
    );
    publish(&mut restarted, rs, json!(2), NOW);
    // A bare position cannot detect restart once the new head catches up.
    assert_eq!(
        poll(&mut restarted, rs, Some(cursor), 20, NOW)["items"],
        json!([])
    );
}
#[test]
fn rights_revocation_keeps_own_content_but_not_status_or_publish() {
    let f = fixture();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let s = auth(&mut e, &f.keys[0], NOW);
    let p = publish(&mut e, s, json!("own"), NOW);
    let mid = p["message_id"].as_str().unwrap();
    e.set_acl(&f.ids[0], "local/events", vec![]).unwrap();
    assert!(!e.authenticated(s, NOW));
    let s = auth(&mut e, &f.keys[0], NOW);
    let own = get(&mut e, s, mid, NOW);
    assert_eq!(own["message"]["payload"], "own");

    assert_eq!(publish(&mut e, s, json!(2), NOW)["code"], "forbidden");
    assert_eq!(poll(&mut e, s, None, 20, NOW)["code"], "forbidden");
}

#[test]
fn rate_limit_is_shared_across_sessions_and_malformed_requests() {
    let mut f = fixture();
    f.cfg.limits.client_burst = 2;
    f.cfg.limits.client_requests_per_second = 1;
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let a = auth(&mut e, &f.keys[0], NOW);
    let b = auth(&mut e, &f.keys[0], NOW);
    assert_eq!(poll(&mut e, a, None, 20, NOW)["type"], "messages");
    assert_eq!(e.handle(b, "{", NOW)["code"], "invalid_frame");
    assert_eq!(poll(&mut e, b, None, 20, NOW)["code"], "rate_limited");
    assert_eq!(poll(&mut e, b, None, 20, NOW + 1000)["type"], "messages");
}
#[test]
fn expiry_removes_content_indexes_pending_and_payload_budget() {
    let mut f = fixture();
    f.cfg.topics[0].policy.as_mut().unwrap().default_ttl_seconds = 1;
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let s = auth(&mut e, &f.keys[0], NOW);
    let p = publish(&mut e, s, json!("data"), NOW);
    poll(&mut e, s, None, 20, NOW);
    e.sweep(NOW + 1000);
    assert_eq!(e.active_bytes, 0);
    assert!(e.index.is_empty());
    assert_eq!(
        get(&mut e, s, p["message_id"].as_str().unwrap(), NOW + 1000)["code"],
        "not_found"
    );
}
#[test]
fn owner_default_expiry_is_reported_and_invalid_producer_times_are_rejected() {
    let f = fixture();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let s = auth(&mut e, &f.keys[0], NOW);
    let r = publish(&mut e, s, json!(1), NOW);
    assert_eq!(
        r["expires"]["time"],
        protocol::timestamp(NOW + 300_000).unwrap()
    );
    for time in [
        protocol::timestamp(NOW).unwrap(),
        protocol::timestamp(NOW + 4_000_000).unwrap(),
        "tomorrowZ".into(),
    ] {
        let r = send(
            &mut e,
            s,
            json!({"version":1,"type":"publish","id":"p","topic":"local/events","payload":1,"expires":{"time":time}}),
            NOW,
        );
        assert_eq!(r["code"], "invalid_frame");
    }
    assert_eq!(e.index.len(), 1);
}
#[test]
fn list_snapshot_excludes_later_publishes_and_cursor_scopes() {
    let f = fixture();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let s = auth(&mut e, &f.keys[0], NOW);
    publish(&mut e, s, json!(1), NOW);
    publish(&mut e, s, json!(2), NOW);
    let list = send(
        &mut e,
        s,
        json!({"version":1,"type":"list_messages","id":"l","limit":1}),
        NOW,
    );
    let c = list["next_cursor"].as_str().unwrap();
    publish(&mut e, s, json!(3), NOW);
    let next = send(
        &mut e,
        s,
        json!({"version":1,"type":"list_messages","id":"l2","limit":1,"cursor":c}),
        NOW,
    );
    assert_eq!(next["items"].as_array().unwrap().len(), 1);
    assert!(next.get("next_cursor").is_none());
    assert_eq!(
        poll(&mut e, s, Some(c), 20, NOW)["items"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}
#[test]
fn poll_pages_fit_output_byte_budget_and_make_progress() {
    let mut f = fixture();
    f.cfg.limits.max_payload_bytes = 1024;
    f.cfg.limits.max_response_bytes = 9216;
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let s = auth(&mut e, &f.keys[0], NOW);
    for i in 0..10 {
        let r = publish(&mut e, s, json!({"i":i,"data":"x".repeat(900)}), NOW);
        assert_eq!(r["type"], "result");
    }
    let mut c = None;
    let mut total = 0;
    loop {
        let p = poll(&mut e, s, c.as_deref(), 20, NOW);
        assert_eq!(p["type"], "messages", "{p}");
        assert!(p.to_string().len() <= 9216);
        total += p["items"].as_array().unwrap().len();
        c = Some(p["next_cursor"].as_str().unwrap().to_owned());
        if p["has_more"] == false {
            break;
        }
    }
    assert_eq!(total, 10);
}
#[test]
fn auth_challenge_is_owner_and_session_bound_and_consumed_once() {
    let f = fixture();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let s = e.connect(NOW).unwrap();
    let public = crypto::public_key(&f.keys[0]).unwrap();
    let h = send(
        &mut e,
        s,
        json!({"version":1,"type":"auth_hello","id":"h","token_id":crypto::token_id(&public)}),
        NOW,
    );
    let n = crypto::decode(h["nonce"].as_str().unwrap()).unwrap();
    let wrong = crypto::sign(&f.keys[0], &crypto::auth_bytes(&n, &public, "other-owner")).unwrap();
    assert_eq!(
        send(
            &mut e,
            s,
            json!({"version":1,"type":"auth_proof","id":"p","auth_id":"h","signature":wrong}),
            NOW
        )["code"],
        "unauthorized"
    );
    let good = crypto::sign(&f.keys[0], &crypto::auth_bytes(&n, &public, "local")).unwrap();
    assert_eq!(
        send(
            &mut e,
            s,
            json!({"version":1,"type":"auth_proof","id":"p2","auth_id":"h","signature":good}),
            NOW
        )["code"],
        "unauthorized"
    );
    assert!(!e.authenticated(s, NOW));
}
#[test]
fn acl_change_invalidates_in_flight_auth_proof() {
    let f = fixture();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let s = e.connect(NOW).unwrap();
    let public = crypto::public_key(&f.keys[0]).unwrap();
    let h = send(
        &mut e,
        s,
        json!({"version":1,"type":"auth_hello","id":"h","token_id":crypto::token_id(&public)}),
        NOW,
    );
    let n = crypto::decode(h["nonce"].as_str().unwrap()).unwrap();
    let sig = crypto::sign(&f.keys[0], &crypto::auth_bytes(&n, &public, "local")).unwrap();
    e.set_acl(&f.ids[0], "local/events", vec![]).unwrap();
    assert_eq!(
        send(
            &mut e,
            s,
            json!({"version":1,"type":"auth_proof","id":"p","auth_id":"h","signature":sig}),
            NOW
        )["code"],
        "unauthorized"
    );
}
#[test]
fn rotation_preserves_identity_content_and_rate_bucket_but_invalidates_old_key() {
    let f = fixture();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let s = auth(&mut e, &f.keys[0], NOW);
    let published = publish(&mut e, s, json!("original"), NOW);
    let old_peer = auth(&mut e, &f.keys[0], NOW);
    let new = crypto::generate_key().unwrap();
    let (o, r, secret) = rotation(&mut e, s, NOW);
    let request = commit_request(&e, &f.ids[0], &f.keys[0], &new, &o, &r, &secret);
    let done = send(&mut e, s, request, NOW);
    assert_eq!(done["type"], "rotation_complete", "{done}");
    assert_eq!(done["client_id"], f.ids[0]);
    assert!(!e.authenticated(old_peer, NOW));
    let ns = auth(&mut e, &new, NOW);
    assert_eq!(
        get(&mut e, ns, published["message_id"].as_str().unwrap(), NOW)["message"]["payload"],
        "original"
    );
    assert_eq!(
        get(&mut e, ns, published["message_id"].as_str().unwrap(), NOW)["message"]["producer_id"],
        f.ids[0]
    );
    publish(&mut e, ns, json!("after rotation"), NOW);
    let peer = auth(&mut e, &f.keys[1], NOW);
    let messages = poll(&mut e, peer, None, 20, NOW);
    assert!(
        messages["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["message"]["producer_id"] == f.ids[0])
    );
    assert!(e.clients[&f.ids[0]].bucket.units < e.config.limits.client_burst * 1000);
}
#[test]
fn rotation_timeout_keeps_old_key_and_does_not_reset_attempt_budget() {
    let mut f = fixture();
    f.cfg.rotation.attempts_per_client = 1;
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let s = auth(&mut e, &f.keys[0], NOW);
    rotation(&mut e, s, NOW);
    let later = NOW + e.config.rotation.timeout_seconds * 1000 + 1;
    e.sweep(later);
    assert!(e.rotations.is_empty());
    assert!(e.authenticated(s, later));
    assert_eq!(
        send(
            &mut e,
            s,
            json!({"version":1,"type":"rotation_start","id":"again"}),
            later
        )["code"],
        "rate_limited"
    );
}
#[test]
fn malformed_rotation_proof_is_consumed_and_old_key_survives() {
    let f = fixture();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let s = auth(&mut e, &f.keys[0], NOW);
    let new = crypto::generate_key().unwrap();
    let (o, r, secret) = rotation(&mut e, s, NOW);
    let mut request = commit_request(&e, &f.ids[0], &f.keys[0], &new, &o, &r, &secret);
    request["signature"] = crypto::encode(&[0; 64]).into();
    assert_eq!(
        send(&mut e, s, request.clone(), NOW)["code"],
        "unauthorized"
    );
    assert!(e.authenticated(s, NOW));
    assert_eq!(send(&mut e, s, request, NOW)["code"], "rotation_invalid");
}
#[test]
fn rotation_is_owner_local_and_new_key_does_not_update_other_owner() {
    let f = fixture();
    let mut a = Engine::new(f.cfg.clone(), NOW).unwrap();
    let mut cfg = f.cfg;
    cfg.service.id = "owner-b".into();
    for t in &mut cfg.topics {
        t.owner_id = "owner-b".into();
    }
    let mut b = Engine::new(cfg, NOW).unwrap();
    let bs = auth(&mut b, &f.keys[0], NOW);
    let a_session = auth(&mut a, &f.keys[0], NOW);
    let new = crypto::generate_key().unwrap();
    let (o, r, secret) = rotation(&mut a, a_session, NOW);
    let request = commit_request(&a, &f.ids[0], &f.keys[0], &new, &o, &r, &secret);
    assert_eq!(
        send(&mut a, a_session, request, NOW)["type"],
        "rotation_complete"
    );
    assert!(b.authenticated(bs, NOW));
    assert!(
        !b.keys
            .contains_key(&crypto::token_id(&crypto::public_key(&new).unwrap()))
    );
}
#[test]
fn replayed_auth_on_an_authenticated_session_closes_its_identity() {
    let f = fixture();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let s = auth(&mut e, &f.keys[0], NOW);
    let r = send(
        &mut e,
        s,
        json!({"version":1,"type":"auth_hello","id":"again","token_id":crypto::token_id(&crypto::public_key(&f.keys[0]).unwrap())}),
        NOW,
    );
    assert_eq!(r["code"], "unauthorized");
    assert!(!e.authenticated(s, NOW));
}
#[test]
fn empty_own_list_and_config_duplicate_rules_do_not_panic() {
    let mut f = fixture();
    let mut e = Engine::new(f.cfg.clone(), NOW).unwrap();
    let s = auth(&mut e, &f.keys[0], NOW);
    assert_eq!(
        send(
            &mut e,
            s,
            json!({"version":1,"type":"list_messages","id":"empty"}),
            NOW
        )["items"],
        json!([])
    );
    let rule = f.cfg.topics[0].acl.as_ref().unwrap()[0].clone();
    f.cfg.topics[0].acl.as_mut().unwrap().push(rule);
    assert!(f.cfg.validate().is_err());
}

#[test]
fn key_revocation_during_rotation_cannot_register_the_new_key() {
    let f = fixture();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let s = auth(&mut e, &f.keys[0], NOW);
    let new = crypto::generate_key().unwrap();
    let (o, r, secret) = rotation(&mut e, s, NOW);
    let request = commit_request(&e, &f.ids[0], &f.keys[0], &new, &o, &r, &secret);
    e.revoke_key(&crypto::token_id(&crypto::public_key(&f.keys[0]).unwrap()))
        .unwrap();
    assert_eq!(send(&mut e, s, request, NOW)["code"], "unauthorized");
    assert!(
        !e.keys
            .contains_key(&crypto::token_id(&crypto::public_key(&new).unwrap()))
    );
    assert!(e.rotations.is_empty());
    assert!(e.rotation_expiry.is_empty());
}
#[test]
fn memory_capacity_rejection_is_atomic() {
    let mut f = fixture();
    f.cfg.limits.max_active_messages = 1;
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let s = auth(&mut e, &f.keys[0], NOW);
    assert_eq!(publish(&mut e, s, json!(1), NOW)["type"], "result");
    let charge = e.active_bytes;
    assert_eq!(
        publish(&mut e, s, json!(2), NOW)["code"],
        "resource_exhausted"
    );
    assert_eq!(e.next_message, 1);
    assert_eq!(e.active_bytes, charge);
    assert_eq!(e.own[&f.ids[0]].len(), 1);
}
#[test]
fn credential_expiration_ends_authenticated_access_even_to_own_content() {
    let mut f = fixture();
    f.cfg.clients[0].keys[0].valid_until = protocol::timestamp(NOW + 1000).unwrap();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let s = auth(&mut e, &f.keys[0], NOW);
    let p = publish(&mut e, s, json!(1), NOW);
    assert_eq!(
        get(&mut e, s, p["message_id"].as_str().unwrap(), NOW + 1000)["code"],
        "unauthorized"
    );
}

#[test]
fn rid_wrap_never_overwrites_active_messages_and_reuses_expired_ids() {
    let f = fixture();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let s = auth(&mut e, &f.keys[0], NOW);
    assert_eq!(
        publish(&mut e, s, json!("original"), NOW)["message_id"],
        "00000000"
    );
    e.message_ids = crate::ids::IdPool::at(0);
    assert_eq!(
        publish(&mut e, s, json!("collision"), NOW)["code"],
        "resource_exhausted"
    );
    assert_eq!(e.next_message, 1);
    assert_eq!(e.index.len(), 1);
    assert_eq!(
        get(&mut e, s, "00000000", NOW)["message"]["payload"],
        "original"
    );
    let slot = e.topics["local/events"].messages.values().next().unwrap();
    let expiry = e.records.get(slot).unwrap().until();
    e.sweep(expiry);
    e.message_ids = crate::ids::IdPool::at(0);
    assert_eq!(
        publish(&mut e, s, json!("reused"), expiry)["message_id"],
        "00000000"
    );
}

#[test]
fn poll_crosses_eight_character_wrap() {
    let f = fixture();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let s = auth(&mut e, &f.keys[0], NOW);
    e.topics.get_mut("local/events").unwrap().head = crate::ids::SPACE - 1;
    publish(&mut e, s, json!("after wrap"), NOW);
    let page = poll(&mut e, s, Some("zzzzzzzz"), 20, NOW);
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["next_cursor"], "00000000");
    let empty = poll(&mut e, s, Some("00000000"), 20, NOW);
    assert_eq!(empty["items"], json!([]));
}

#[test]
fn catalog_metadata_requires_auth_but_no_topic_rights() {
    let f = fixture();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let query =
        json!({"version":1,"type":"get_catalog","id":"schema","name":"mynofy","catalog_version":1});
    let anon = e.connect(NOW).unwrap();
    assert_eq!(
        send(&mut e, anon, query.clone(), NOW)["code"],
        "unauthorized"
    );
    e.set_acl(&f.ids[0], "local/events", vec![]).unwrap();
    e.set_acl(&f.ids[0], "other", vec![]).unwrap();
    let s = auth(&mut e, &f.keys[0], NOW);
    let catalog = send(&mut e, s, query.clone(), NOW);
    assert_eq!(catalog["type"], "catalog");
    assert_eq!(catalog["request_id"], "schema");
    assert_eq!(
        &catalog["catalog"],
        protocol::catalog::builtin_catalog().unwrap()
    );
    assert!(serde_json::to_vec(&catalog).unwrap().len() <= e.config.limits.max_response_bytes);
    for (name, version) in [("unknown", 1), ("mynofy", 2)] {
        let mut bad = query.clone();
        bad["name"] = name.into();
        bad["catalog_version"] = version.into();
        assert_eq!(send(&mut e, s, bad, NOW)["code"], "not_found");
    }
    let mut bad = query.clone();
    bad["catalog_version"] = "1".into();
    assert_eq!(send(&mut e, s, bad, NOW)["code"], "invalid_frame");
    let mut bad = query;
    bad["extra"] = true.into();
    assert_eq!(send(&mut e, s, bad, NOW)["code"], "invalid_frame");
    e.clients.get_mut(&f.ids[0]).unwrap().bucket.units = 0;
    assert_eq!(
        send(
            &mut e,
            s,
            json!({"version":1,"type":"get_catalog","id":"limited","name":"mynofy","catalog_version":1}),
            NOW
        )["code"],
        "rate_limited"
    );
}

#[test]
fn binary_payload_limits_poll_status_and_shared_budgets() {
    use protocol::ProtobufMessage as _;
    let f = fixture();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let s = auth(&mut e, &f.keys[0], NOW);
    let publish = |data| pb::Frame {
        version: 1,
        body: Some(pb::Body::Publish(pb::Publish {
            id: "pb".into(),
            topic: "local/events".into(),
            payload: Payload::bytes(data, None),
            expires: None,
        })),
    };
    let ok = e
        .handle_protobuf(s, &publish(vec![0xff; 4096]).encode_to_vec(), NOW)
        .to_json()
        .unwrap();
    assert_eq!(ok["type"], "result");
    assert_eq!(
        e.handle_protobuf(s, &publish(vec![0; 4097]).encode_to_vec(), NOW)
            .to_json()
            .unwrap()["code"],
        "invalid_frame"
    );
    assert_eq!(e.next_message, 1);
    let request = pb::Frame::from_json(
        json!({"version":1,"type":"poll","id":"p","topic":"local/events","limit":100}),
    )
    .unwrap();
    let result = e.handle_protobuf(s, &request.encode_to_vec(), NOW);
    let Some(pb::Body::Messages(result)) = result.body else {
        panic!("Expected messages")
    };
    assert_eq!(
        result.items[0].message.payload,
        Payload::bytes(vec![0xff; 4096], None)
    );
    let status = get(&mut e, s, ok["message_id"].as_str().unwrap(), NOW);
    assert_eq!(
        status["message"]["payload"],
        Payload::bytes(vec![0xff; 4096], None).to_json().unwrap()
    );
    let unauth = e.connect(NOW).unwrap();
    assert_eq!(
        e.handle_protobuf(unauth, &request.encode_to_vec(), NOW)
            .kind(),
        "error"
    );
    e.clients.get_mut(&f.ids[0]).unwrap().bucket.units = 0;
    assert_eq!(
        e.handle_protobuf(s, b"bad", NOW).to_json().unwrap()["code"],
        "rate_limited"
    );
}
#[test]
fn binary_malformed_version_direction_and_ids_are_rejected() {
    use protocol::ProtobufMessage as _;
    let f = fixture();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let s = auth(&mut e, &f.keys[0], NOW);
    for raw in [
        vec![],
        vec![0xff],
        pb::Frame {
            version: 2,
            body: None,
        }
        .encode_to_vec(),
    ] {
        assert_eq!(
            e.handle_protobuf(s, &raw, NOW).to_json().unwrap()["code"],
            "invalid_frame"
        );
    }
    let response =
        pb::Frame::from_json(json!({"version":1,"type":"result","request_id":"illegal"})).unwrap();
    assert_eq!(
        e.handle_protobuf(s, &response.encode_to_vec(), NOW)
            .to_json()
            .unwrap()["code"],
        "invalid_direction"
    );
    let mut request =
        pb::Frame::from_json(json!({"version":1,"type":"poll","id":"p","topic":"local/events"}))
            .unwrap();
    if let Some(pb::Body::Poll(ref mut p)) = request.body {
        p.id.clear();
    }
    assert_eq!(
        e.handle_protobuf(s, &request.encode_to_vec(), NOW)
            .to_json()
            .unwrap()["code"],
        "invalid_frame"
    );
    let before = e.next_message;
    let payload = Payload::event(protocol::payload::Event {
        catalog: 1,
        catalog_version: 1,
        category: 12,
        action: 10,
        status: 7,
        parameters: vec![],
    });
    let request = pb::Frame {
        version: 1,
        body: Some(pb::Body::Publish(pb::Publish {
            id: "event".into(),
            topic: "local/events".into(),
            payload: payload.clone(),
            expires: None,
        })),
    };
    assert_eq!(
        e.handle_protobuf(s, &request.encode_to_vec(), NOW).kind(),
        "result"
    );
    assert_eq!(e.next_message, before + 1);
    let slot = e.topics["local/events"].messages.values().last().unwrap();
    assert_eq!(
        e.records
            .get(slot)
            .unwrap()
            .payload()
            .payload(&e.content_types, &e.payloads),
        payload
    );
}

#[test]
fn recycled_records_preserve_ownership_content_types_and_ownership_isolation() {
    let f = fixture();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let a = auth(&mut e, &f.keys[0], NOW);
    let b = auth(&mut e, &f.keys[1], NOW);
    let first = send(
        &mut e,
        a,
        json!({"version":1,"type":"publish","id":"a","topic":"local/events","payload":{"kind":"bytes","data":"YQ==","content_type":"type/old"},"expires":{"time":protocol::timestamp(NOW+1000).unwrap()}}),
        NOW,
    );
    let first_id = Id::parse(first["message_id"].as_str().unwrap()).unwrap();
    let old_slot = e.index[&first_id];
    let survivor = send(
        &mut e,
        b,
        json!({"version":1,"type":"publish","id":"b","topic":"other","payload":{"kind":"bytes","data":"Yg==","content_type":"type/survivor"}}),
        NOW,
    );
    let _page = poll(&mut e, a, None, 20, NOW);
    assert_eq!(e.content_types.len(), 2);
    e.sweep(NOW + 1000);
    assert_eq!(e.content_types.len(), 1);
    // Recycle both the storage slot and the expired Message-ID; old delivery must stay invalid.
    e.message_ids = crate::ids::IdPool::at(0);
    let new = send(
        &mut e,
        a,
        json!({"version":1,"type":"publish","id":"new","topic":"local/events","payload":{"kind":"bytes","data":"Yw==","content_type":"type/new"}}),
        NOW + 1000,
    );
    assert_eq!(new["message_id"], first["message_id"]);
    assert_eq!(e.index[&first_id], old_slot);

    assert_eq!(
        get(
            &mut e,
            b,
            survivor["message_id"].as_str().unwrap(),
            NOW + 1000
        )["message"]["payload"]["content_type"],
        "type/survivor"
    );
    assert_eq!(
        get(&mut e, b, new["message_id"].as_str().unwrap(), NOW + 1000)["code"],
        "not_found"
    );
    poll(&mut e, a, None, 20, NOW + 1000);

    e.sweep(NOW + 4_000_000);
    assert_eq!(e.records.len(), 0);
    assert_eq!(e.content_types.len(), 0);
    assert!(e.index.is_empty());
    assert!(e.own.values().all(OrderedIndex::is_empty));
    assert!(e.expiry.is_empty());
    assert_eq!(e.active_bytes, 0);
}
#[test]
fn rejected_publish_allocates_no_record_or_interned_metadata() {
    let mut f = fixture();
    f.cfg.limits.max_active_messages = 1;
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let a = auth(&mut e, &f.keys[0], NOW);
    publish(&mut e, a, json!(1), NOW);
    let before = e.storage_stats();
    let response = send(
        &mut e,
        a,
        json!({"version":1,"type":"publish","id":"reject","topic":"local/events","payload":{"kind":"bytes","data":"ZA==","content_type":"must/not/intern"}}),
        NOW,
    );
    assert_eq!(response["code"], "resource_exhausted");
    let after = e.storage_stats();
    assert_eq!(after.active_records, before.active_records);
    assert_eq!(after.record_capacity, before.record_capacity);
    assert_eq!(after.interned_content_types, before.interned_content_types);
}

#[test]
fn expiration_churn_reuses_arena_and_preserves_cursor_and_metadata() {
    let mut f = fixture();
    f.cfg.limits.client_burst = 10000;
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let a = auth(&mut e, &f.keys[0], NOW);
    let mut after = None;
    for round in 0..3 {
        let now = NOW + round * 2000;
        for i in 0..300 {
            let p = send(
                &mut e,
                a,
                json!({"version":1,"type":"publish","id":"churn","topic":"local/events","payload":{"kind":"bytes","data":"eA==","content_type":format!("test/{round}/{i}")},"expires":{"time":protocol::timestamp(now+1000).unwrap()}}),
                now,
            );
            assert_eq!(p["type"], "result", "{p}");
        }
        assert_eq!(e.storage_stats().record_capacity, 512);
        assert_eq!(e.content_types.len(), 300);
        let page = poll(&mut e, a, after.as_deref(), 1, now);
        assert_eq!(page["items"].as_array().unwrap().len(), 1);
        assert_eq!(
            page["items"][0]["message"]["payload"]["content_type"],
            format!("test/{round}/0")
        );
        while e.sweep(now + 1000).more {}
        let empty = poll(&mut e, a, None, 20, now + 1000);
        assert_eq!(empty["items"], json!([]));
        after = Some(empty["next_cursor"].as_str().unwrap().to_owned());
        assert_eq!(e.records.len(), 0);
        assert!(e.index.is_empty());
        assert_eq!(e.content_types.len(), 0);
        assert_eq!(e.active_bytes, 0);
    }
}

#[test]
fn shared_time_base_preserves_wrap_and_long_ttl_creation_timestamps() {
    let mut f = fixture();
    f.cfg.topics[0].policy.as_mut().unwrap().max_ttl_seconds = 6_000_000;
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let s = auth(&mut e, &f.keys[0], NOW);
    for ttl in [300u64, (1u64 << 32) - 1, 1u64 << 32, (1u64 << 32) + 1000] {
        let time = protocol::timestamp(NOW + ttl).unwrap();
        let r = send(
            &mut e,
            s,
            json!({"version":1,"type":"publish","id":"time","topic":"local/events","payload":1,"expires":{"time":time}}),
            NOW,
        );
        assert_eq!(r["type"], "result", "{r}");
        assert_eq!(
            get(&mut e, s, r["message_id"].as_str().unwrap(), NOW)["created_at"],
            protocol::timestamp(NOW).unwrap()
        );
    }
    assert_eq!(e.storage_stats().extended_time_entries, 0);
    e.sweep(NOW + 6_000_001_000);
    assert_eq!(e.records.len(), 0);
}
#[test]
fn maintenance_releases_empty_chunks_and_preserves_surviving_records() {
    let mut f = fixture();
    f.cfg.limits.client_burst = 10000;
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let s = auth(&mut e, &f.keys[0], NOW);
    let mut survivor = String::new();
    for i in 0..800 {
        let expiry = protocol::timestamp(NOW + if i == 300 { 100_000 } else { 1000 }).unwrap();
        let r = send(
            &mut e,
            s,
            json!({"version":1,"type":"publish","id":"gc","topic":"local/events","payload":i,"expires":{"time":expiry}}),
            NOW,
        );
        assert_eq!(r["type"], "result");
        if i == 300 {
            survivor = r["message_id"].as_str().unwrap().into();
        }
    }
    let before = e.storage_stats();
    assert_eq!(before.record_capacity, 1024);
    for _ in 0..16 {
        e.maintenance(NOW + 1001);
    }
    let after = e.storage_stats();
    assert_eq!(after.active_records, 1);
    assert_eq!(after.record_capacity, 256);
    assert!(after.arena_allocated_bytes < before.arena_allocated_bytes / 2);
    assert!(after.id_index_capacity < before.id_index_capacity);
    assert_eq!(
        get(&mut e, s, &survivor, NOW + 1001)["message"]["payload"],
        300
    );
    publish(&mut e, s, json!("after trim"), NOW + 1001);
    assert_eq!(e.records.len(), 2);
    for _ in 0..16 {
        e.maintenance(NOW + 400_000);
    }
    let empty = e.storage_stats();
    assert_eq!(empty.active_records, 0);
    assert_eq!(empty.arena_allocated_bytes, 0);
    assert_eq!(empty.id_index_capacity, 0);
    assert_eq!(empty.expiry_data_bytes, 0);
    assert_eq!(empty.ordered_data_bytes, 0);
}
#[test]
fn producer_snapshot_uses_its_own_order_across_other_producers() {
    let f = fixture();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let a = auth(&mut e, &f.keys[0], NOW);
    let b = auth(&mut e, &f.keys[1], NOW);
    for i in 0..5 {
        publish(&mut e, b, json!(i), NOW);
    }
    publish(&mut e, a, json!(10), NOW);
    publish(&mut e, b, json!(20), NOW);
    publish(&mut e, a, json!(30), NOW);
    let first = send(
        &mut e,
        a,
        json!({"version":1,"type":"list_messages","id":"page","limit":1}),
        NOW,
    );
    let cursor = first["next_cursor"].as_str().unwrap();
    publish(&mut e, a, json!(40), NOW);
    let page = send(
        &mut e,
        a,
        json!({"version":1,"type":"list_messages","id":"next","limit":1,"cursor":cursor}),
        NOW,
    );
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert!(page.get("next_cursor").is_none());
}

#[test]
fn numeric_acl_reports_exact_masks_and_updates_only_target_pair() {
    let mut f = fixture();
    f.cfg.topics[0].acl.as_mut().unwrap()[0].allow = vec![Action::InspectOwn, Action::Publish];
    let mut engine = Engine::new(f.cfg, NOW).unwrap();
    let session = auth(&mut engine, &f.keys[0], NOW);
    let other_session = auth(&mut engine, &f.keys[1], NOW);
    let permissions = send(
        &mut engine,
        session,
        json!({"version":1,"type":"get_permissions","id":"rights","topic":"local/events"}),
        NOW,
    );
    assert_eq!(permissions["actions"], json!(["publish", "inspect_own"]));
    assert_eq!(
        poll(&mut engine, session, None, 1, NOW)["code"],
        "forbidden"
    );
    assert!(engine.allowed(&f.ids[0], "other", Action::Subscribe));
    assert!(engine.allowed(&f.ids[1], "local/events", Action::Subscribe));
    assert!(!engine.allowed("missing-client", "local/events", Action::Publish));
    let revision = engine.clients[&f.ids[0]].revision;
    assert!(
        engine
            .set_acl(&f.ids[0], "missing-topic", vec![Action::Publish])
            .is_err()
    );
    assert_eq!(engine.clients[&f.ids[0]].revision, revision);
    assert!(engine.authenticated(session, NOW));
    engine
        .set_acl(&f.ids[0], "local/events", vec![Action::Subscribe])
        .unwrap();
    assert!(!engine.authenticated(session, NOW));
    assert!(engine.authenticated(other_session, NOW));
    let session = auth(&mut engine, &f.keys[0], NOW);
    let permissions = send(
        &mut engine,
        session,
        json!({"version":1,"type":"get_permissions","id":"rights","topic":"local/events"}),
        NOW,
    );
    assert_eq!(permissions["actions"], json!(["subscribe"]));
    assert_eq!(
        publish(&mut engine, session, json!("denied"), NOW)["code"],
        "forbidden"
    );
    assert!(!engine.allowed(&f.ids[0], "local/events", Action::InspectOwn));
    assert!(engine.allowed(&f.ids[0], "other", Action::Publish));
    engine.set_acl(&f.ids[0], "local/events", vec![]).unwrap();
    let session = auth(&mut engine, &f.keys[0], NOW);
    let permissions = send(
        &mut engine,
        session,
        json!({"version":1,"type":"get_permissions","id":"rights","topic":"local/events"}),
        NOW,
    );
    assert_eq!(permissions["actions"], json!([]));
}

#[test]
fn direct_output_matches_generated_codec_across_pages_and_errors() {
    use bytes::BytesMut;
    let mut f = fixture();
    f.cfg.limits.max_response_bytes = 16384;
    let mut typed = Engine::new(f.cfg.clone(), NOW).unwrap();
    let mut direct = Engine::new(f.cfg, NOW).unwrap();
    direct.generation = typed.generation.clone();
    let typed_session = auth(&mut typed, &f.keys[0], NOW);
    let direct_session = auth(&mut direct, &f.keys[0], NOW);
    let mut bytes = BytesMut::new();
    let mut exchange = |body: pb::Body| {
        let raw = pb::Frame {
            version: 1,
            body: Some(body),
        }
        .encode_to_vec();
        let expected = typed.handle_protobuf(typed_session, &raw, NOW);
        let kind = direct.handle_protobuf_into(direct_session, &raw, NOW, &mut bytes);
        assert_eq!(kind, expected.kind());
        assert_eq!(bytes.as_ref(), expected.encode_to_vec());
        let decoded = pb::Frame::decode(bytes.as_ref()).unwrap();
        assert_eq!(decoded, expected);
        decoded
    };
    for i in 0..51 {
        let payload = if i % 2 == 0 {
            protocol::payload::Payload::event(protocol::payload::Event {
                catalog: 1,
                catalog_version: 1,
                category: 12,
                action: 10,
                status: 7,
                parameters: if i == 0 {
                    vec![]
                } else {
                    vec![
                        protocol::payload::Parameter {
                            value: Some(protocol::payload::ParameterValue::I64(i64::MIN)),
                        },
                        protocol::payload::Parameter {
                            value: Some(protocol::payload::ParameterValue::F64(-0.0)),
                        },
                        protocol::payload::Parameter {
                            value: Some(protocol::payload::ParameterValue::Bool(false)),
                        },
                    ]
                },
            })
        } else {
            protocol::payload::Payload::bytes(
                vec![i as u8; 4096],
                Some("application/octet-stream".into()),
            )
        };
        exchange(pb::Body::Publish(pb::Publish {
            id: "pub".into(),
            topic: "local/events".into(),
            payload,
            expires: None,
        }));
    }
    let mut after = None;
    let mut received = 0;
    loop {
        let frame = exchange(pb::Body::Poll(pb::Poll {
            id: "poll".into(),
            topic: "local/events".into(),
            after,
            limit: Some(100),
        }));
        let Some(pb::Body::Messages(page)) = frame.body else {
            panic!("Expected messages")
        };
        received += page.items.len();
        after = Some(page.next_cursor);
        if !page.has_more {
            break;
        }
    }
    assert_eq!(received, 51);
    exchange(pb::Body::Poll(pb::Poll {
        id: "empty".into(),
        topic: "local/events".into(),
        after,
        limit: Some(100),
    }));
    exchange(pb::Body::Poll(pb::Poll {
        id: "invalid".into(),
        topic: "local/events".into(),
        after: Some("invalid".into()),
        limit: Some(100),
    }));
    exchange(pb::Body::Poll(pb::Poll {
        id: "unknown".into(),
        topic: "missing".into(),
        after: None,
        limit: Some(100),
    }));
}

#[test]
fn direct_poll_matches_generated_encoder_at_length_and_page_budget_boundaries() {
    use protocol::payload::{Event, Parameter, ParameterValue};

    let mut f = fixture();
    let event = |parameters| {
        Payload::event(Event {
            catalog: 1,
            catalog_version: 1,
            category: 12,
            action: 10,
            status: 7,
            parameters,
        })
    };
    let mut payloads = vec![
        event(Vec::new()),
        event(
            [
                ParameterValue::I64(i64::MIN),
                ParameterValue::U64(u64::MAX),
                ParameterValue::F64(-0.0),
            ]
            .into_iter()
            .map(|value| Parameter { value: Some(value) })
            .collect(),
        ),
    ];
    payloads.extend(
        [126, 127, 128, 129, 4096, 4096, 4096, 4096]
            .into_iter()
            .map(|length| {
                Payload::bytes(vec![0xa5; length], Some("application/octet-stream".into()))
            }),
    );
    let cutoff = payloads.len();
    payloads.push(Payload::bytes(Vec::new(), None));

    let mut seeded = |budget| {
        f.cfg.limits.max_response_bytes = budget;
        let mut engine = Engine::new(f.cfg.clone(), NOW).unwrap();
        let session = auth(&mut engine, &f.keys[0], NOW);
        for payload in &payloads {
            engine
                .publish_typed(0, "seed", "local/events", payload.clone(), None, NOW)
                .unwrap();
        }
        (engine, session)
    };
    let compare = |engine: &mut Engine, session, after| {
        let raw = pb::Frame {
            version: 1,
            body: Some(pb::Body::Poll(pb::Poll {
                id: "poll".into(),
                topic: "local/events".into(),
                after,
                limit: Some(100),
            })),
        }
        .encode_to_vec();
        let expected = engine.handle_protobuf(session, &raw, NOW);
        let mut direct = bytes::BytesMut::new();
        assert_eq!(
            engine.handle_protobuf_into(session, &raw, NOW, &mut direct),
            expected.kind(),
        );
        assert_eq!(direct.as_ref(), expected.encode_to_vec());
        assert!(direct.len() <= engine.config().limits.max_response_bytes);
        let Some(pb::Body::Messages(page)) = expected.body else {
            panic!("Expected messages, got {expected:?}");
        };
        page
    };

    let (mut reference, session) = seeded(65536);
    let full = compare(&mut reference, session, None);
    assert_eq!(full.items.len(), payloads.len());
    for (item, payload) in full.items.iter().zip(&payloads) {
        assert_eq!(&item.message.payload, payload);
    }
    assert!(!full.has_more);
    assert!(full.encoded_len() >= 16384);

    // Use generated item lengths as the oracle for the poll selection policy.
    let exact_budget = 4096
        + full.items[..cutoff]
            .iter()
            .map(|item| item.encoded_len() + 8)
            .sum::<usize>();
    for (budget, accepted) in [
        (exact_budget - 1, cutoff - 1),
        (exact_budget, cutoff),
        (exact_budget + 1, cutoff),
    ] {
        let (mut engine, session) = seeded(budget);
        let first = compare(&mut engine, session, None);
        assert_eq!(first.items, full.items[..accepted], "budget {budget}");
        assert_eq!(first.next_cursor, crate::cursor::encode(accepted as u64));
        assert!(first.has_more);
        let second = compare(&mut engine, session, Some(first.next_cursor));
        assert_eq!(second.items, full.items[accepted..], "budget {budget}");
        assert_eq!(second.next_cursor, full.next_cursor);
        assert!(!second.has_more);
    }
}

#[test]
fn expired_index_scans_are_bounded_and_empty_pages_advance_both_cursors() {
    let mut f = fixture();
    f.cfg.limits.max_active_messages = 3000;
    f.cfg.limits.max_topic_active_messages = Some(f.cfg.limits.max_active_messages);
    f.cfg.limits.max_publisher_active_messages = Some(f.cfg.limits.max_active_messages);
    f.cfg.limits.client_burst = 10000;
    f.cfg.topics[0].policy.as_mut().unwrap().default_ttl_seconds = 1;
    let producer = f.ids[0].clone();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    for i in 0..2000 {
        e.publish_typed(
            0,
            "old",
            "local/events",
            Payload::bytes(vec![i as u8], None),
            None,
            NOW,
        )
        .unwrap();
    }
    let now = NOW + 1000;
    e.cleanup_step(now, 0);
    let fresh = e
        .publish_typed(
            0,
            "fresh",
            "local/events",
            Payload::bytes(vec![7], None),
            None,
            now,
        )
        .unwrap();
    let mut cursor = None;
    let mut pages = 0;
    let mut messages = Vec::new();
    loop {
        let page = e
            .poll_typed(
                session,
                "poll",
                "local/events",
                cursor.as_deref(),
                Some(100),
                now,
                true,
                None,
            )
            .unwrap();
        if pages == 0 {
            assert!(page.items.is_empty());
            assert!(page.has_more);
            assert_eq!(
                crate::cursor::decode(&page.next_cursor, 2001),
                Some(READ_SCAN_LIMIT as u64)
            );
        }
        if pages == 0 {
            let mut encoded = bytes::BytesMut::new();
            let direct = e
                .poll_typed(
                    session,
                    "poll",
                    "local/events",
                    None,
                    Some(100),
                    now,
                    true,
                    Some(&mut encoded),
                )
                .unwrap();
            assert!(direct.items.is_empty());
            assert_eq!(
                encoded.as_ref(),
                pb::Frame {
                    version: 1,
                    body: Some(pb::Body::Messages(page.clone()))
                }
                .encode_to_vec()
            );
        }
        messages.extend(page.items.into_iter().map(|item| item.message.id));
        pages += 1;
        assert!(pages < 12);
        cursor = Some(page.next_cursor);
        if !page.has_more {
            break;
        }
    }
    assert_eq!(messages, vec![fresh.message_id.unwrap()]);

    let mut list_cursor = None;
    let mut listed = 0;
    let mut pages = 0;
    loop {
        let page = e
            .list(&producer, "list", None, Some(100), list_cursor, now)
            .unwrap();
        if pages == 0 {
            assert!(page["items"].as_array().unwrap().is_empty());
            assert!(page.get("next_cursor").is_some());
        }
        listed += page["items"].as_array().unwrap().len();
        pages += 1;
        assert!(pages < 12);
        list_cursor = page["next_cursor"].as_str().map(str::to_owned);
        if list_cursor.is_none() {
            break;
        }
    }
    assert_eq!(listed, 1);
    assert_eq!(e.records.len(), 2001);
    let capped = e.cleanup_step(now, usize::MAX);
    assert_eq!(capped.work, MAINTENANCE_CLEANUP_UNITS);
    assert_eq!(capped.records, MAINTENANCE_CLEANUP_UNITS);
    assert!(capped.more);
    assert_eq!(e.storage_stats().active_records, 1);
}

fn admin_data(engine: &mut Engine, command: crate::admin::Command) -> Value {
    match engine.admin(command, NOW).unwrap() {
        crate::admin::ResultBody::Data(value) => value,
        _ => panic!("Expected admin data"),
    }
}
#[test]
fn admin_registration_acl_disable_and_reconnect_preserve_owned_content() {
    use crate::admin::Command as A;
    let mut f = fixture();
    f.cfg.limits.max_parallel_requests_per_client = 1;
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let key = crypto::generate_key().unwrap();
    let public = crypto::encode(&crypto::public_key(&key).unwrap());
    let added = admin_data(
        &mut e,
        A::ClientAdd {
            public_key: public.clone(),
            valid_until: None,
            display_name: Some("new-agent".into()),
        },
    );
    let id = added["client_id"].as_str().unwrap().to_owned();
    let a = auth(&mut e, &key, NOW);
    let b = auth(&mut e, &key, NOW);
    let permits = e.request_semaphore(a, NOW).unwrap();
    let permit = permits.clone().try_acquire_owned().unwrap();
    assert!(
        e.request_semaphore(b, NOW)
            .unwrap()
            .try_acquire_owned()
            .is_err()
    );
    drop(permit);
    assert_eq!(poll(&mut e, a, None, 1, NOW)["code"], "forbidden");
    admin_data(
        &mut e,
        A::AclSet {
            client: id.clone(),
            topic: "local/events".into(),
            allow: vec![Action::Publish, Action::Subscribe, Action::InspectOwn],
        },
    );
    assert!(!e.authenticated(a, NOW));
    assert!(!e.authenticated(b, NOW));
    let session = auth(&mut e, &key, NOW);
    let msg = publish(&mut e, session, json!("owned"), NOW);
    let _page = poll(&mut e, session, None, 1, NOW);
    admin_data(
        &mut e,
        A::ClientDisable {
            client: id.clone(),
            disabled: true,
        },
    );
    assert!(!e.authenticated(session, NOW));
    let unauth = e.connect(NOW).unwrap();
    let token = crypto::token_id(&crypto::public_key(&key).unwrap());
    assert_eq!(
        send(
            &mut e,
            unauth,
            json!({"version":1,"type":"auth_hello","id":"h","token_id":token}),
            NOW
        )["code"],
        "unauthorized"
    );
    admin_data(
        &mut e,
        A::ClientDisable {
            client: id.clone(),
            disabled: false,
        },
    );
    let session = auth(&mut e, &key, NOW);
    assert_eq!(
        get(&mut e, session, msg["message_id"].as_str().unwrap(), NOW)["message"]["payload"],
        "owned"
    );

    assert_eq!(
        e.admin(
            A::ClientAdd {
                public_key: public,
                valid_until: None,
                display_name: None
            },
            NOW
        )
        .err()
        .unwrap()
        .code,
        "conflict"
    );
    assert_eq!(e.clients.len(), 3);
    let page = admin_data(
        &mut e,
        A::ClientsList {
            after: None,
            limit: Some(2),
        },
    );
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    let last = admin_data(
        &mut e,
        A::ClientsList {
            after: Some(page["next_after"].as_u64().unwrap() as u16),
            limit: Some(2),
        },
    );
    assert_eq!(last["items"][0]["client_id"], id);
}

#[test]
fn admin_export_uses_current_keys_disabled_flags_and_acls_without_baseline_rewrite() {
    use crate::admin::Command as A;
    let f = fixture();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    let old_token = crypto::token_id(&crypto::public_key(&f.keys[0]).unwrap());
    admin_data(&mut e, A::KeyRevoke { token: old_token });
    assert!(!e.authenticated(session, NOW));
    admin_data(
        &mut e,
        A::AclSet {
            client: f.ids[0].clone(),
            topic: "local/events".into(),
            allow: vec![Action::Publish],
        },
    );
    admin_data(
        &mut e,
        A::ClientDisable {
            client: f.ids[1].clone(),
            disabled: true,
        },
    );
    let crate::admin::ResultBody::Configuration(no_keys) = e.admin(A::Export {}, NOW).unwrap()
    else {
        panic!("snapshot")
    };
    no_keys.validate().unwrap();
    assert!(no_keys.clients[0].keys.is_empty());
    let new_key = crypto::generate_key().unwrap();
    let new_public = crypto::encode(&crypto::public_key(&new_key).unwrap());
    admin_data(
        &mut e,
        A::KeyAdd {
            client: f.ids[0].clone(),
            public_key: new_public.clone(),
            valid_until: None,
        },
    );
    let session = auth(&mut e, &new_key, NOW);
    assert_eq!(e.client_id(session, NOW).unwrap(), f.ids[0]);
    let crate::admin::ResultBody::Configuration(config) = e.admin(A::Export {}, NOW).unwrap()
    else {
        panic!("snapshot")
    };
    config.validate().unwrap();
    assert_eq!(config.clients[0].keys[0].public_key, new_public);
    assert!(config.clients[1].disabled);
    assert_ne!(e.config.clients[0].keys[0].public_key, new_public);
    let json = serde_json::to_value(&config).unwrap();
    let restored_cfg: Config = serde_json::from_value(json).unwrap();
    let mut restored = Engine::new(restored_cfg, NOW).unwrap();
    let session = auth(&mut restored, &new_key, NOW);
    assert!(restored.allowed(&f.ids[0], "local/events", Action::Publish));
    assert!(!restored.allowed(&f.ids[0], "local/events", Action::Subscribe));
    assert_eq!(restored.client_id(session, NOW).unwrap(), f.ids[0]);
    assert!(restored.clients[&f.ids[1]].disabled);
}

#[test]
fn admin_invalid_mutations_are_atomic_and_message_channel_has_no_admin_operation() {
    use crate::admin::Command as A;
    let f = fixture();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    let clients = e.clients.len();
    let keys = e.keys.len();
    let revision = e.clients[&f.ids[0]].revision;
    assert!(
        e.admin(
            A::ClientAdd {
                public_key: "bad".into(),
                valid_until: None,
                display_name: None
            },
            NOW
        )
        .is_err()
    );
    assert!(
        e.admin(
            A::ClientAdd {
                public_key: crypto::encode(
                    &crypto::public_key(&crypto::generate_key().unwrap()).unwrap()
                ),
                valid_until: Some(protocol::timestamp(NOW).unwrap()),
                display_name: None
            },
            NOW
        )
        .is_err()
    );
    assert!(
        e.admin(
            A::AclSet {
                client: f.ids[0].clone(),
                topic: "local/events".into(),
                allow: vec![Action::Publish, Action::Publish]
            },
            NOW
        )
        .is_err()
    );
    assert!(
        e.admin(
            A::ClientDisable {
                client: "missing".into(),
                disabled: true
            },
            NOW
        )
        .is_err()
    );
    assert_eq!(e.clients.len(), clients);
    assert_eq!(e.keys.len(), keys);
    assert_eq!(e.clients[&f.ids[0]].revision, revision);
    assert!(e.authenticated(session, NOW));
    assert_eq!(
        send(
            &mut e,
            session,
            json!({"version":1,"type":"clients_list"}),
            NOW
        )["code"],
        "unknown_type"
    );
}

#[test]
fn admin_revocation_invalidates_pending_challenges_even_after_explicit_reenrollment() {
    use crate::admin::Command as A;
    let f = fixture();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let public = crypto::public_key(&f.keys[0]).unwrap();
    let token = crypto::token_id(&public);
    let session = e.connect(NOW).unwrap();
    let challenge = send(
        &mut e,
        session,
        json!({"version":1,"type":"auth_hello","id":"h","token_id":token}),
        NOW,
    );
    let nonce = crypto::decode(challenge["nonce"].as_str().unwrap()).unwrap();
    let signature = crypto::sign(
        &f.keys[0],
        &crypto::auth_bytes(&nonce, &public, &e.config.service.id),
    )
    .unwrap();
    admin_data(&mut e, A::KeyRevoke { token });
    admin_data(
        &mut e,
        A::KeyAdd {
            client: f.ids[0].clone(),
            public_key: crypto::encode(&public),
            valid_until: None,
        },
    );
    assert_eq!(
        send(
            &mut e,
            session,
            json!({"version":1,"type":"auth_proof","id":"p","auth_id":"h","signature":signature}),
            NOW
        )["code"],
        "unauthorized"
    );
    let current = auth(&mut e, &f.keys[0], NOW);
    assert!(e.authenticated(current, NOW));
}

#[test]
fn admin_export_after_committed_rotation_contains_only_the_new_key() {
    use crate::admin::Command as A;
    let f = fixture();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    let new = crypto::generate_key().unwrap();
    let (offer, ready, secret) = rotation(&mut e, session, NOW);
    let commit = commit_request(&e, &f.ids[0], &f.keys[0], &new, &offer, &ready, &secret);
    assert_eq!(
        send(&mut e, session, commit, NOW)["type"],
        "rotation_complete"
    );
    let crate::admin::ResultBody::Configuration(snapshot) = e.admin(A::Export {}, NOW).unwrap()
    else {
        panic!("snapshot")
    };
    assert_eq!(snapshot.clients[0].keys.len(), 1);
    assert_eq!(
        snapshot.clients[0].keys[0].public_key,
        crypto::encode(&crypto::public_key(&new).unwrap())
    );
    let mut restarted = Engine::new(*snapshot, NOW).unwrap();
    let session = auth(&mut restarted, &new, NOW);
    assert_eq!(restarted.client_id(session, NOW).unwrap(), f.ids[0]);
}

#[cfg(unix)]
struct SecurityDirectory(std::path::PathBuf);
#[cfg(unix)]
impl SecurityDirectory {
    fn configure(config: &mut Config, capacity: usize, compact: u64) -> Self {
        use std::os::unix::fs::DirBuilderExt;
        let path = std::env::temp_dir().join(format!(
            "mne-{:016x}",
            u64::from_le_bytes(crypto::random().unwrap())
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .unwrap();
        config.security_storage = Some(SecurityStorage {
            minimum_sequence: None,
            sync: false,
            directory: path.clone(),
            queue_capacity: capacity,
            compact_after_records: compact,
        });
        Self(path)
    }
}
#[cfg(unix)]
impl Drop for SecurityDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[cfg(unix)]
fn wait_security(e: &Engine, condition: impl Fn(&crate::security::Status) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if condition(&e.security_status()) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "security writer timed out: {}",
            serde_json::to_string(&e.security_status()).unwrap()
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
#[cfg(unix)]
#[test]
fn security_journal_restores_admin_changes_and_rotation_without_messages_or_sessions() {
    use crate::admin::Command as A;
    let mut f = fixture();
    let _directory = SecurityDirectory::configure(&mut f.cfg, 32, 100);
    let mut e = Engine::new(f.cfg.clone(), NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    let message = publish(&mut e, session, json!("volatile"), NOW);
    assert_eq!(message["type"], "result");
    let new = crypto::generate_key().unwrap();
    let (offer, ready, secret) = rotation(&mut e, session, NOW);
    let request = commit_request(&e, &f.ids[0], &f.keys[0], &new, &offer, &ready, &secret);
    assert_eq!(
        send(&mut e, session, request, NOW)["type"],
        "rotation_complete"
    );
    e.admin(
        A::ClientDisable {
            client: f.ids[1].clone(),
            disabled: true,
        },
        NOW,
    )
    .unwrap();
    let extra = crypto::generate_key().unwrap();
    let public = crypto::public_key(&extra).unwrap();
    let client = crypto::initial_client_id(&public);
    e.admin(
        A::ClientAdd {
            public_key: crypto::encode(&public),
            valid_until: None,
            display_name: Some("persisted".into()),
        },
        NOW,
    )
    .unwrap();
    e.admin(
        A::AclSet {
            client: client.clone(),
            topic: "local/events".into(),
            allow: vec![Action::Subscribe],
        },
        NOW,
    )
    .unwrap();
    let additional = crypto::generate_key().unwrap();
    let additional_public = crypto::public_key(&additional).unwrap();
    e.admin(
        A::KeyAdd {
            client: client.clone(),
            public_key: crypto::encode(&additional_public),
            valid_until: None,
        },
        NOW,
    )
    .unwrap();
    e.admin(
        A::KeyRevoke {
            token: crypto::token_id(&public),
        },
        NOW,
    )
    .unwrap();
    assert_eq!(e.security_status().submitted_sequence, 6);
    let session = auth(&mut e, &additional, NOW);
    let _incomplete = rotation(&mut e, session, NOW);
    assert!(!e.rotations.is_empty());
    drop(e); // Normal drop drains queued records.
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    assert!(e.sessions.is_empty());
    assert!(e.clients[&f.ids[1]].disabled);
    assert_eq!(
        e.clients[&client].display_name.as_deref(),
        Some("persisted")
    );
    assert!(
        !e.keys
            .contains_key(&crypto::token_id(&crypto::public_key(&f.keys[0]).unwrap()))
    );
    assert!(!e.keys.contains_key(&crypto::token_id(&public)));
    let s = auth(&mut e, &new, NOW);
    assert_eq!(e.client_id(s, NOW).unwrap(), f.ids[0]);
    let s = auth(&mut e, &additional, NOW);
    assert_eq!(e.client_id(s, NOW).unwrap(), client);
    assert!(e.allowed(&client, "local/events", Action::Subscribe));
    assert!(!e.allowed(&client, "local/events", Action::Publish));
    assert_eq!(e.records.len(), 0);
    assert_eq!(e.rotations.len(), 0);
    assert_eq!(e.security_status().durable_sequence, 6);
    assert_eq!(e.security_status().snapshot_sequence, 6);
}
#[cfg(unix)]
#[test]
fn stalled_compaction_keeps_ram_changes_nonblocking_and_rejects_full_queue_before_mutation() {
    use crate::admin::Command as A;
    let mut f = fixture();
    let directory = SecurityDirectory::configure(&mut f.cfg, 1, 1);
    let mut e = Engine::new(f.cfg.clone(), NOW).unwrap();
    // Block checkpoint replacement without depending on filesystem speed or root permission checks.
    let temporary = directory.0.join("snapshot.tmp");
    std::fs::create_dir(&temporary).unwrap();
    e.admin(
        A::ClientDisable {
            client: f.ids[0].clone(),
            disabled: true,
        },
        NOW,
    )
    .unwrap();
    wait_security(&e, |status| status.last_error.is_some());
    assert_eq!(e.security_status().durable_sequence, 1);
    e.admin(
        A::ClientDisable {
            client: f.ids[0].clone(),
            disabled: false,
        },
        NOW,
    )
    .unwrap();
    assert!(!e.clients[&f.ids[0]].disabled);
    assert_eq!(e.security_status().submitted_sequence, 2);
    assert_eq!(e.security_status().durable_sequence, 1);
    assert_eq!(
        e.admin(
            A::ClientDisable {
                client: f.ids[0].clone(),
                disabled: true
            },
            NOW
        )
        .err()
        .unwrap()
        .code,
        "security_storage_busy"
    );
    assert!(!e.clients[&f.ids[0]].disabled);
    let old = e
        .acl
        .actions(e.clients[&f.ids[0]].slot, e.topics["local/events"].slot);
    assert!(e.set_acl(&f.ids[0], "local/events", vec![]).is_err());
    assert_eq!(
        e.acl
            .actions(e.clients[&f.ids[0]].slot, e.topics["local/events"].slot),
        old
    );
    let token = crypto::token_id(&crypto::public_key(&f.keys[0]).unwrap());
    assert!(e.revoke_key(&token).is_err());
    assert!(e.keys.contains_key(&token));
    std::fs::remove_dir(&temporary).unwrap();
    wait_security(&e, |status| {
        status.durable_sequence == 2 && status.last_error.is_none()
    });
    drop(e);
    let e = Engine::new(f.cfg, NOW).unwrap();
    assert!(!e.clients[&f.ids[0]].disabled);
    assert_eq!(e.security_status().durable_sequence, 2);
}

#[test]
fn message_image_is_independent_of_later_owner_mutation_and_enforces_capture_limits() {
    use crate::message_image::{ImageLimits, MessageImage};
    let f = fixture();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    let published = publish(&mut e, session, json!("snapshot-marker"), NOW);
    let id = published["message_id"].as_str().unwrap();
    assert!(
        e.capture_message_image(ImageLimits {
            max_bytes: 0,
            ..ImageLimits::default()
        })
        .is_err()
    );
    let image = e.capture_message_image(ImageLimits::default()).unwrap();
    while e.sweep(NOW + 400_000).more {}
    assert_eq!(e.records.len(), 0);
    assert_eq!(image.message_count(), 1);
    let mut bytes = Vec::new();
    image.write_to(&mut bytes).unwrap();
    let restored = MessageImage::read_from(bytes.as_slice(), ImageLimits::default()).unwrap();
    assert_eq!(restored.owner(), "local");
    assert_eq!(restored.message_count(), 1);
    let stored = restored.message(id).unwrap();
    assert_eq!(stored.payload.to_json().unwrap(), json!("snapshot-marker"));
}

#[cfg(unix)]
fn content_fixture(f: &mut Fixture, mode: ContentEncryption) -> SecurityDirectory {
    let directory = SecurityDirectory::configure(&mut f.cfg, 128, 4096);
    f.cfg.security_storage.as_mut().unwrap().directory = directory.0.join("security");
    for topic in &mut f.cfg.topics {
        if topic.owner_id == f.cfg.service.id {
            topic.storage = crate::config::TopicStorage::Manual {
                encrypted: mode != ContentEncryption::None,
            };
        }
    }
    f.cfg.content_storage = Some(ContentStorage {
        store_id: None,
        directory: directory.0.join("content"),
        encryption: mode,
        max_snapshot_bytes: 128 * 1024 * 1024,
        autosave_retry: Default::default(),
    });
    directory
}
#[cfg(unix)]
fn wait_content(e: &Engine, condition: impl Fn(&crate::content::Status) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let status = e.content_status();
        if condition(&status) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "content writer timeout: {}",
            serde_json::to_string(&status).unwrap()
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
#[cfg(unix)]
#[test]
fn full_content_restore_preserves_messages_bindings_policies_and_counter_progress() {
    use crate::admin::Command as A;
    let mut f = fixture();
    let directory = content_fixture(&mut f, ContentEncryption::None);
    let mut e = Engine::new(f.cfg.clone(), NOW).unwrap();
    let first = auth(&mut e, &f.keys[0], NOW);
    let second = auth(&mut e, &f.keys[1], NOW);
    let message = publish(&mut e, first, json!("durable content"), NOW);
    let mid = message["message_id"].as_str().unwrap().to_owned();
    let page = poll(&mut e, second, None, 20, NOW);
    let cursor = page["next_cursor"].as_str().unwrap().to_owned();

    let extra = crypto::generate_key().unwrap();
    let public = crypto::public_key(&extra).unwrap();
    e.admin(
        A::ClientAdd {
            public_key: crypto::encode(&public),
            valid_until: None,
            display_name: None,
        },
        NOW,
    )
    .unwrap();
    let revision = e.save_content().unwrap();
    wait_content(&e, |status| {
        !status.save_in_progress && status.saved_revision == revision
    });
    assert!(!e.content_status().dirty);
    let old_names = e.client_names.clone();
    let reserved = e.message_ids.reserved_position();
    // More recent security state must remain authoritative after loading older content.
    e.set_acl(&f.ids[0], "local/events", vec![]).unwrap();
    drop(e);
    let mut cfg = f.cfg.clone();
    cfg.topics.clear();
    let mut e = Engine::new(cfg, NOW + 1).unwrap();
    assert!(e.sessions.is_empty());
    assert_eq!(&e.client_names[..old_names.len()], old_names);
    assert!(e.content_status().loaded);
    assert_eq!(e.records.len(), 1);

    assert!(!e.allowed(&f.ids[0], "local/events", Action::Publish));
    assert_eq!(e.topics["local/events"].policy.default_ttl_seconds, 300);
    let reader = auth(&mut e, &f.keys[1], NOW + 1);
    let repeated = poll(&mut e, reader, None, 20, NOW + 1);

    assert_eq!(repeated["items"][0]["message"]["producer_id"], f.ids[0]);
    assert_eq!(
        repeated["items"][0]["message"]["payload"],
        "durable content"
    );
    assert_eq!(
        poll(&mut e, reader, Some(&cursor), 20, NOW + 1)["items"],
        json!([])
    );

    let produced = publish(&mut e, reader, json!("new"), NOW + 1);
    assert_eq!(produced["type"], "result");
    assert_ne!(produced["message_id"], mid);
    assert_eq!(
        produced["message_id"],
        crate::ids::Id::from_index(reserved).to_string()
    );
    assert_eq!(e.topics["local/events"].head, 2);
    assert_eq!(e.next_message, 2);
    assert!(directory.0.join("content/state.bin").exists());
}
#[cfg(unix)]
#[test]
fn content_restore_resumes_past_a_synchronously_refilled_range() {
    let mut f = fixture();
    let _directory = content_fixture(&mut f, ContentEncryption::None);
    let mut e = Engine::new(f.cfg.clone(), NOW).unwrap();
    e.message_ids = crate::ids::IdPool::at(crate::ids::SPACE - 1);
    let session = auth(&mut e, &f.keys[0], NOW);
    assert_eq!(
        publish(&mut e, session, json!("last"), NOW)["message_id"],
        "zzzzzzzz"
    );
    assert_eq!(
        publish(&mut e, session, json!("first"), NOW)["message_id"],
        "00000000"
    );
    let reserved = e.message_ids.reserved_position();
    assert_eq!(reserved, 1024);
    let revision = e.save_content().unwrap();
    wait_content(&e, |status| {
        !status.save_in_progress && status.saved_revision == revision
    });
    drop(e);

    let mut restored = Engine::new(f.cfg, NOW + 1).unwrap();
    assert_eq!(restored.records.len(), 2);
    let session = auth(&mut restored, &f.keys[0], NOW + 1);
    assert_eq!(
        publish(&mut restored, session, json!("after restore"), NOW + 1)["message_id"],
        crate::ids::Id::from_index(reserved).to_string()
    );
    assert_eq!(
        get(&mut restored, session, "zzzzzzzz", NOW + 1)["message"]["payload"],
        "last"
    );
    assert_eq!(
        get(&mut restored, session, "00000000", NOW + 1)["message"]["payload"],
        "first"
    );
}

#[cfg(unix)]
#[test]
fn restore_removes_expired_messages_without_reusing_positions() {
    use crate::content::ContentSecret;
    let mut f = fixture();
    let _directory = content_fixture(&mut f, ContentEncryption::KeyStdin);
    let secret = || Some(ContentSecret::Key(Zeroizing::new([7; 32])));
    let mut e = Engine::new_with_content_secret(f.cfg.clone(), NOW, secret()).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    publish(&mut e, session, json!("expires"), NOW);
    poll(&mut e, session, None, 20, NOW);
    let revision = e.save_content().unwrap();
    wait_content(&e, |status| status.saved_revision == revision);
    drop(e);
    assert!(Engine::new(f.cfg.clone(), NOW).is_err());
    let mut e = Engine::new_with_content_secret(f.cfg.clone(), NOW + 400_000, secret()).unwrap();

    assert_eq!(e.topics["local/events"].head, 1);
    let session = auth(&mut e, &f.keys[0], NOW + 400_000);
    assert_eq!(
        publish(&mut e, session, json!("after"), NOW + 400_000)["type"],
        "result"
    );
    assert_eq!(e.topics["local/events"].head, 2);
}
#[cfg(unix)]
#[test]
fn content_save_errors_keep_the_previous_file_and_autosave_retries_a_dirty_state() {
    let mut f = fixture();
    let directory = content_fixture(&mut f, ContentEncryption::None);
    f.cfg.topics[0].storage = TopicStorage::Interval {
        seconds: 1,
        encrypted: false,
    };
    f.cfg.topics[1].storage = TopicStorage::Ram {};
    let mut e = Engine::new(f.cfg.clone(), NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    publish(&mut e, session, json!("first"), NOW);
    e.content.as_mut().unwrap().force_due();
    e.maintenance(NOW);
    wait_content(&e, |status| !status.save_in_progress && !status.dirty);
    let file = directory.0.join("content/state.bin");
    let original = std::fs::read(&file).unwrap();
    let temporary = directory.0.join("content/state.tmp");
    std::fs::create_dir(&temporary).unwrap();
    publish(&mut e, session, json!("second"), NOW);
    e.save_content().unwrap();
    wait_content(&e, |status| {
        !status.save_in_progress && status.last_error.is_some()
    });
    assert_eq!(std::fs::read(&file).unwrap(), original);
    assert!(e.content_status().dirty);
    e.content.as_ref().unwrap().reserve().unwrap();
    assert_eq!(e.save_content().err().unwrap().code, "content_save_busy");
    e.content.as_ref().unwrap().cancel();
    std::fs::remove_dir(&temporary).unwrap();
    e.content.as_mut().unwrap().force_due();
    e.maintenance(NOW);
    wait_content(&e, |status| {
        !status.save_in_progress && !status.dirty && status.last_error.is_none()
    });
    drop(e);
    let e = Engine::new(f.cfg, NOW).unwrap();
    assert_eq!(e.records.len(), 2);
}
#[cfg(unix)]
#[test]
fn failed_content_restore_never_grants_a_missing_identity_or_silently_drops_active_records() {
    use crate::admin::Command as A;
    let mut f = fixture();
    let directory = content_fixture(&mut f, ContentEncryption::None);
    let mut e = Engine::new(f.cfg.clone(), NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    publish(&mut e, session, json!("one"), NOW);
    publish(&mut e, session, json!("two"), NOW);
    let extra = crypto::generate_key().unwrap();
    e.admin(
        A::ClientAdd {
            public_key: crypto::encode(&crypto::public_key(&extra).unwrap()),
            valid_until: None,
            display_name: None,
        },
        NOW,
    )
    .unwrap();
    let revision = e.save_content().unwrap();
    wait_content(&e, |status| status.saved_revision == revision);
    drop(e);
    let mut limited = f.cfg.clone();
    limited.limits.max_active_messages = 1;
    assert!(Engine::new(limited, NOW).is_err());
    let mut missing = f.cfg.clone();
    missing.security_storage.as_mut().unwrap().directory = directory.0.join("fresh-security");
    assert!(Engine::new(missing, NOW).is_err());
    let path = directory.0.join("content/state.bin");
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[32] ^= 1;
    std::fs::write(path, bytes).unwrap();
    assert!(Engine::new(f.cfg, NOW).is_err());
}

#[cfg(unix)]
#[cfg(unix)]
fn topic_publish(e: &mut Engine, session: u64, topic: &str, value: &str, now: u64) -> Value {
    send(
        e,
        session,
        json!({"version":1,"type":"publish","id":"topic-save","topic":topic,"payload":value}),
        now,
    )
}
#[cfg(unix)]
fn topic_poll(e: &mut Engine, session: u64, topic: &str, now: u64) -> Value {
    send(
        e,
        session,
        json!({"version":1,"type":"poll","id":"topic-poll","topic":topic,"limit":20}),
        now,
    )
}
#[cfg(unix)]
#[test]
fn topic_policies_isolate_ram_manual_interval_and_encrypted_images() {
    use crate::content::ContentSecret;
    let mut f = fixture();
    let directory = content_fixture(&mut f, ContentEncryption::KeyStdin);
    f.cfg.topics[0].storage = TopicStorage::Manual { encrypted: false };
    f.cfg.topics[1].storage = TopicStorage::Ram {};
    let mut automatic = f.cfg.topics[0].clone();
    automatic.name = "automatic".into();
    automatic.storage = TopicStorage::Interval {
        seconds: 60,
        encrypted: true,
    };
    f.cfg.topics.push(automatic);
    let secret = || Some(ContentSecret::Key(Zeroizing::new([0x23; 32])));
    let mut e = Engine::new_with_content_secret(f.cfg.clone(), NOW, secret()).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    assert_eq!(
        topic_publish(&mut e, session, "local/events", "manual-marker", NOW)["type"],
        "result"
    );
    assert_eq!(
        topic_publish(&mut e, session, "automatic", "encrypted-marker", NOW)["type"],
        "result"
    );
    assert_eq!(
        topic_publish(&mut e, session, "other", "ram-marker", NOW)["type"],
        "result"
    );
    let revision = e.save_content().unwrap();
    wait_content(&e, |status| {
        !status.save_in_progress && status.saved_revision == revision
    });
    assert!(!e.content_status().dirty);
    let files = || {
        std::fs::read_dir(directory.0.join("content"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("topic-")
            })
            .collect::<Vec<_>>()
    };
    let saved = files();
    assert_eq!(saved.len(), 2);
    let plaintext = saved
        .iter()
        .find(|path| std::fs::read(path).unwrap().starts_with(b"MYNCP001"))
        .unwrap();
    let encrypted = saved
        .iter()
        .find(|path| std::fs::read(path).unwrap().starts_with(b"MYNENC01"))
        .unwrap();
    assert!(
        std::fs::read(plaintext)
            .unwrap()
            .windows(13)
            .any(|bytes| bytes == b"manual-marker")
    );
    assert!(
        !std::fs::read(encrypted)
            .unwrap()
            .windows(16)
            .any(|bytes| bytes == b"encrypted-marker")
    );
    let original = std::fs::read(plaintext).unwrap();
    topic_publish(&mut e, session, "local/events", "manual-unsaved", NOW + 1);
    topic_publish(&mut e, session, "automatic", "automatic-new", NOW + 1);
    e.content.as_mut().unwrap().force_due();
    e.maintenance(NOW + 1);
    wait_content(&e, |status| {
        !status.save_in_progress && !status.topics["automatic"].dirty
    });
    assert!(e.content_status().topics["local/events"].dirty);
    assert_eq!(std::fs::read(plaintext).unwrap(), original);
    assert_eq!(files().len(), 2);
    drop(e);
    let mut e = Engine::new_with_content_secret(f.cfg, NOW + 2, secret()).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW + 2);
    let manual = topic_poll(&mut e, session, "local/events", NOW + 2);
    assert_eq!(manual["items"].as_array().unwrap().len(), 1);
    assert_eq!(manual["items"][0]["message"]["payload"], "manual-marker");
    assert_eq!(
        topic_poll(&mut e, session, "automatic", NOW + 2)["items"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        topic_poll(&mut e, session, "other", NOW + 2)["items"],
        json!([])
    );
    assert_eq!(e.next_message, 5); // Includes RAM/unsaved producer positions, preventing position reuse.
}
#[cfg(unix)]
#[test]
fn explicit_topic_saves_preserve_unselected_snapshots_and_reject_invalid_groups() {
    let mut f = fixture();
    let _directory = content_fixture(&mut f, ContentEncryption::None);
    let mut e = Engine::new(f.cfg.clone(), NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    topic_publish(&mut e, session, "local/events", "old-a", NOW);
    topic_publish(&mut e, session, "other", "old-b", NOW);
    let revision = e.save_content().unwrap();
    wait_content(&e, |s| !s.save_in_progress && s.saved_revision == revision);
    topic_publish(&mut e, session, "local/events", "new-a", NOW);
    topic_publish(&mut e, session, "other", "new-b", NOW);
    for group in [
        vec![],
        vec!["missing".into()],
        vec!["other".into(), "other".into()],
    ] {
        assert_eq!(
            e.save_content_topics(Some(&group)).unwrap_err().code,
            "invalid_content_topics"
        );
        assert!(!e.content_status().save_in_progress);
    }
    let revision = e.save_content_topics(Some(&["other".into()])).unwrap();
    wait_content(&e, |s| !s.save_in_progress && s.saved_revision == revision);
    assert!(e.content_status().topics["local/events"].dirty);
    assert!(!e.content_status().topics["other"].dirty);
    drop(e);
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    assert_eq!(
        topic_poll(&mut e, session, "local/events", NOW)["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        topic_poll(&mut e, session, "other", NOW)["items"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}
#[cfg(unix)]
#[test]
fn ram_is_default_and_saved_topic_to_ram_requires_offline_deletion() {
    let mut f = fixture();
    assert!(
        f.cfg
            .topics
            .iter()
            .all(|topic| topic.storage == TopicStorage::Ram {})
    );
    let directory = content_fixture(&mut f, ContentEncryption::None);
    let mut e = Engine::new(f.cfg.clone(), NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    topic_publish(&mut e, session, "local/events", "discard-me", NOW);
    let revision = e.save_content().unwrap();
    wait_content(&e, |s| !s.save_in_progress && s.saved_revision == revision);
    drop(e);
    f.cfg.topics[0].storage = TopicStorage::Ram {};
    // CFG edits alone cannot undo a saved online transition.
    let e = Engine::new(f.cfg.clone(), NOW).unwrap();
    assert_eq!(e.records.len(), 1);
    assert!(e.config.topics[0].storage.persistent());
    assert!(crate::content::offline::OfflineStorage::open(f.cfg.clone(), None).is_err());
    drop(e);
    let mut maintenance =
        crate::content::offline::OfflineStorage::open(f.cfg.clone(), None).unwrap();
    maintenance.topic_to_ram("local/events").unwrap();
    drop(maintenance);
    let e = Engine::new(f.cfg, NOW).unwrap();
    assert_eq!(e.records.len(), 0);
    assert_eq!(e.topics["local/events"].head, 0);
    assert!(!e.content_status().topics.contains_key("local/events"));
    assert_eq!(
        std::fs::read_dir(directory.0.join("content"))
            .unwrap()
            .filter(|entry| entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("topic-"))
            .count(),
        1
    );
}
#[cfg(unix)]
#[test]
fn group_save_failure_preserves_all_committed_topics_and_cleans_uncommitted_images() {
    let mut f = fixture();
    let directory = content_fixture(&mut f, ContentEncryption::None);
    let mut e = Engine::new(f.cfg.clone(), NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    for topic in ["local/events", "other"] {
        topic_publish(&mut e, session, topic, "before", NOW);
    }
    let revision = e.save_content().unwrap();
    wait_content(&e, |s| !s.save_in_progress && s.saved_revision == revision);
    let state = std::fs::read(directory.0.join("content/state.bin")).unwrap();
    for topic in ["local/events", "other"] {
        topic_publish(&mut e, session, topic, "after", NOW);
    }
    std::fs::create_dir(directory.0.join("content/state.tmp")).unwrap();
    e.save_content().unwrap();
    wait_content(&e, |s| !s.save_in_progress && s.last_error.is_some());
    assert_eq!(
        std::fs::read(directory.0.join("content/state.bin")).unwrap(),
        state
    );
    assert_eq!(
        std::fs::read_dir(directory.0.join("content"))
            .unwrap()
            .filter(|entry| entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("topic-"))
            .count(),
        2
    );
    drop(e);
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    for topic in ["local/events", "other"] {
        assert_eq!(
            topic_poll(&mut e, session, topic, NOW)["items"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
}
#[cfg(unix)]
#[test]
fn topic_storage_configuration_is_strict_and_corrupt_or_missing_topic_files_abort_start() {
    let mut f = fixture();
    let directory = content_fixture(&mut f, ContentEncryption::None);
    let mut invalid = f.cfg.clone();
    invalid.content_storage = None;
    assert!(invalid.validate().is_err());
    invalid = f.cfg.clone();
    invalid.topics[0].storage = TopicStorage::Manual { encrypted: true };
    assert!(invalid.validate().is_err());
    for seconds in [0, 2_592_001, u64::MAX] {
        invalid = f.cfg.clone();
        invalid.topics[0].storage = TopicStorage::Interval {
            seconds,
            encrypted: false,
        };
        assert!(invalid.validate().is_err());
    }
    let mut value = serde_json::to_value(&f.cfg).unwrap();
    value["content_storage"]["save_interval_seconds"] = json!(60);
    assert!(serde_json::from_value::<Config>(value).is_err());
    assert!(serde_json::from_value::<TopicStorage>(json!({"mode":"manual","seconds":1})).is_err());
    assert!(
        serde_json::from_value::<TopicStorage>(json!({"mode":"ram","encrypted":true})).is_err()
    );
    let mut e = Engine::new(f.cfg.clone(), NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    topic_publish(&mut e, session, "local/events", "stored", NOW);
    let revision = e.save_content().unwrap();
    wait_content(&e, |s| !s.save_in_progress && s.saved_revision == revision);
    drop(e);
    let path = std::fs::read_dir(directory.0.join("content"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("topic-")
        })
        .unwrap();
    let original = std::fs::read(&path).unwrap();
    let mut corrupt = original.clone();
    corrupt[0] ^= 1;
    std::fs::write(&path, corrupt).unwrap();
    assert!(Engine::new(f.cfg.clone(), NOW).is_err());
    std::fs::write(&path, &original).unwrap();
    assert!(Engine::new(f.cfg.clone(), NOW).is_ok());
    std::fs::remove_file(&path).unwrap();
    assert!(Engine::new(f.cfg, NOW).is_err());
}

#[cfg(unix)]
#[test]
fn legacy_owner_checkpoint_requires_validated_offline_migration() {
    use crate::content::{Checkpoint, checkpoint::TopicState};
    let mut f = fixture();
    let directory = content_fixture(&mut f, ContentEncryption::None);
    let mut e = Engine::new(f.cfg.clone(), NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    topic_publish(&mut e, session, "local/events", "legacy-data", NOW);
    let _page = topic_poll(&mut e, session, "local/events", NOW);
    let checkpoint = Checkpoint {
        image: e.capture_message_image(Default::default()).unwrap(),
        revision: 8,
        next_message: e.next_message,
        reserved_ids: e.message_ids.reserved_position(),
        cutoff: NOW,
        produced: e
            .client_names
            .iter()
            .map(|name| e.clients[name].produced)
            .collect(),
        topics: e
            .topic_names
            .iter()
            .map(|name| TopicState {
                head: e.topics[name].head,
                policy: e.topics[name].policy.clone(),
            })
            .collect(),
    };
    drop(e);
    let path = directory.0.join("content/state.bin");
    std::fs::write(
        &path,
        checkpoint.write(128 * 1024 * 1024).unwrap().as_slice(),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    let mut unspecified = f.cfg.clone();
    for topic in &mut unspecified.topics {
        topic.storage = TopicStorage::Ram {};
    }
    let original = std::fs::read(&path).unwrap();
    assert!(Engine::new(f.cfg.clone(), NOW).is_err());
    assert!(
        crate::content::offline::OfflineStorage::migrate_legacy(unspecified, None, false).is_err()
    );
    let content_lock = directory.0.join("content/lock");
    let lock_before = std::fs::read(&content_lock).unwrap();
    let journal = f
        .cfg
        .security_storage
        .as_ref()
        .unwrap()
        .directory
        .join("journal");
    let original_journal = std::fs::read(&journal).unwrap();
    let mut torn = original_journal.clone();
    torn.push(b'M');
    std::fs::write(&journal, &torn).unwrap();
    let error = crate::content::offline::OfflineStorage::migrate_legacy(f.cfg.clone(), None, true)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Incomplete final security journal frame")
    );
    assert_eq!(std::fs::read(&journal).unwrap(), torn);
    assert_eq!(std::fs::read(&content_lock).unwrap(), lock_before);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    std::fs::write(&journal, &original_journal).unwrap();
    let preview =
        crate::content::offline::OfflineStorage::migrate_legacy(f.cfg.clone(), None, true).unwrap();
    assert_eq!(preview["topics"][0]["messages"], 1);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    crate::content::offline::OfflineStorage::migrate_legacy(f.cfg.clone(), None, false).unwrap();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    assert!(std::fs::read(&path).unwrap().starts_with(b"MYNTOP01"));

    let session = auth(&mut e, &f.keys[0], NOW);
    assert_eq!(
        topic_poll(&mut e, session, "local/events", NOW)["items"][0]["message"]["payload"],
        "legacy-data"
    );
}

#[path = "engine_functional_tests.rs"]
mod functional;

#[cfg(unix)]
#[test]
fn restart_counter_is_durable_before_use_wraps_and_fails_closed_on_write_errors() {
    let mut f = fixture();
    let directory = SecurityDirectory::configure(&mut f.cfg, 128, 4096);
    let e = Engine::new(f.cfg.clone(), NOW).unwrap();
    assert_eq!(e.restart_counter, 1);
    assert!(Engine::new(f.cfg.clone(), NOW).is_err());
    drop(e);
    let e = Engine::new(f.cfg.clone(), NOW).unwrap();
    assert_eq!(e.restart_counter, 2);
    drop(e);
    let snapshot = directory.0.join("snapshot");
    let original = std::fs::read(&snapshot).unwrap();
    let mut value: Value = serde_json::from_slice(&original[80..]).unwrap();
    value["state"]["restart_counter"] = json!(u32::MAX);
    let payload = serde_json::to_vec(&value).unwrap();
    let mut bytes = original[..80].to_vec();
    bytes[4..8].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    let hash = crypto::sha256(&bytes[..16]);
    bytes[16..48].copy_from_slice(&hash);
    bytes[48..80].copy_from_slice(&crypto::sha256(&payload));
    bytes.extend_from_slice(&payload);
    std::fs::write(&snapshot, bytes).unwrap();
    let e = Engine::new(f.cfg.clone(), NOW).unwrap();
    assert_eq!(e.restart_counter, 0);
    drop(e);
    let durable = std::fs::read(&snapshot).unwrap();
    std::fs::create_dir(directory.0.join("snapshot.tmp")).unwrap();
    assert!(Engine::new(f.cfg.clone(), NOW).is_err());
    assert_eq!(std::fs::read(&snapshot).unwrap(), durable);
    std::fs::remove_dir(directory.0.join("snapshot.tmp")).unwrap();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    assert_eq!(e.restart_counter, 1);
    auth(&mut e, &f.keys[0], NOW);
}

#[cfg(unix)]
#[test]
fn online_ram_to_storage_requires_save_and_includes_preexisting_messages() {
    let mut f = fixture();
    let _directory = content_fixture(&mut f, ContentEncryption::None);
    for topic in &mut f.cfg.topics {
        topic.storage = TopicStorage::Ram {};
    }
    let mut e = Engine::new(f.cfg.clone(), NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    publish(&mut e, session, json!("existing before activation"), NOW);
    e.enable_topic_storage("local/events", TopicStorage::Manual { encrypted: false })
        .unwrap();
    assert!(e.content_status().topics["local/events"].dirty);
    let page = e.admin_topics_list(None, Some(1), NOW).unwrap();
    assert_eq!(page["items"][0]["storage"]["mode"], "manual");
    assert_eq!(page["next_after"], "local/events");
    assert_eq!(
        e.admin_topics_list(Some("local/events".into()), Some(1), NOW)
            .unwrap()["items"][0]["storage"]["mode"],
        "ram"
    );
    assert!(
        e.enable_topic_storage("local/events", TopicStorage::Ram {})
            .is_err()
    );
    assert!(
        e.enable_topic_storage(
            "local/events",
            TopicStorage::Interval {
                seconds: 1,
                encrypted: false
            }
        )
        .is_err()
    );
    drop(e);
    let mut e = Engine::new(f.cfg.clone(), NOW).unwrap();
    assert_eq!(e.records.len(), 0);
    assert_eq!(e.config.topics[0].storage, TopicStorage::Ram {});
    let session = auth(&mut e, &f.keys[0], NOW);
    publish(&mut e, session, json!("existing"), NOW);
    e.enable_topic_storage("local/events", TopicStorage::Manual { encrypted: false })
        .unwrap();
    let revision = e.save_content().unwrap();
    wait_content(&e, |status| {
        !status.save_in_progress && status.saved_revision == revision
    });
    drop(e);
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    assert!(e.config.topics[0].storage.persistent());
    assert_eq!(
        e.admin_topic("local/events", NOW).unwrap()["storage"]["mode"],
        "manual"
    );
    let session = auth(&mut e, &f.keys[0], NOW);
    assert_eq!(
        poll(&mut e, session, None, 20, NOW)["items"][0]["message"]["payload"],
        "existing"
    );
}

#[cfg(unix)]
#[test]
fn offline_backup_restore_rekey_preserves_security_and_failed_commit_keeps_old_secret() {
    use crate::content::{ContentSecret, offline::OfflineStorage};
    let mut f = fixture();
    let directory = content_fixture(&mut f, ContentEncryption::KeyStdin);
    let secret = || Some(ContentSecret::Key(Zeroizing::new([7; 32])));
    let mut e = Engine::new_with_content_secret(f.cfg.clone(), NOW, secret()).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    publish(&mut e, session, json!("backed-up"), NOW);
    let revision = e.save_content().unwrap();
    wait_content(&e, |s| !s.save_in_progress && s.saved_revision == revision);
    assert!(OfflineStorage::open(f.cfg.clone(), secret()).is_err());
    drop(e);
    let offline = OfflineStorage::open(f.cfg.clone(), secret()).unwrap();
    let backup = directory.0.join("backup");
    offline.backup(&backup).unwrap();
    assert!(offline.backup(&backup).is_err());
    drop(offline);
    let mut e = Engine::new_with_content_secret(f.cfg.clone(), NOW, secret()).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    publish(&mut e, session, json!("newer"), NOW);
    let revision = e.save_content().unwrap();
    wait_content(&e, |s| !s.save_in_progress && s.saved_revision == revision);
    e.set_acl(&f.ids[0], "local/events", vec![]).unwrap();
    drop(e);
    let security = std::fs::read(directory.0.join("security/snapshot")).unwrap();
    let journal = std::fs::read(directory.0.join("security/journal")).unwrap();
    let mut offline = OfflineStorage::open(f.cfg.clone(), secret()).unwrap();
    let before_restore = std::fs::read(directory.0.join("content/state.bin")).unwrap();
    std::fs::create_dir(directory.0.join("content/state.tmp")).unwrap();
    assert!(offline.restore(&backup).is_err());
    assert_eq!(
        std::fs::read(directory.0.join("content/state.bin")).unwrap(),
        before_restore
    );
    std::fs::remove_dir(directory.0.join("content/state.tmp")).unwrap();
    offline.restore(&backup).unwrap();

    std::fs::create_dir(directory.0.join("content/state.tmp")).unwrap();
    assert!(
        offline
            .rekey(ContentSecret::Key(Zeroizing::new([9; 32])))
            .is_err()
    );
    drop(offline);
    std::fs::remove_dir(directory.0.join("content/state.tmp")).unwrap();
    let mut offline = OfflineStorage::open(f.cfg.clone(), secret()).unwrap();
    offline
        .rekey(ContentSecret::Key(Zeroizing::new([9; 32])))
        .unwrap();
    drop(offline);
    assert_eq!(
        std::fs::read(directory.0.join("security/snapshot")).unwrap(),
        security
    );
    assert_eq!(
        std::fs::read(directory.0.join("security/journal")).unwrap(),
        journal
    );
    assert!(OfflineStorage::open(f.cfg.clone(), secret()).is_err());
    let e = Engine::new_with_content_secret(
        f.cfg,
        NOW,
        Some(ContentSecret::Key(Zeroizing::new([9; 32]))),
    )
    .unwrap();
    assert_eq!(e.records.len(), 1);
    assert!(!e.allowed(&f.ids[0], "local/events", Action::Publish));
}

#[cfg(unix)]
#[test]
fn offline_restore_into_empty_storage_keeps_security_counter_and_validates_owner() {
    use crate::content::offline::OfflineStorage;
    let mut f = fixture();
    let directory = content_fixture(&mut f, ContentEncryption::None);
    let mut e = Engine::new(f.cfg.clone(), NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    publish(&mut e, session, json!("recovery"), NOW);
    let revision = e.save_content().unwrap();
    wait_content(&e, |s| !s.save_in_progress && s.saved_revision == revision);
    drop(e);
    let backup = directory.0.join("backup");
    let source = OfflineStorage::open(f.cfg.clone(), None).unwrap();
    source.backup(&backup).unwrap();
    drop(source);
    let sec = std::fs::read(directory.0.join("security/snapshot")).unwrap();
    f.cfg.content_storage.as_mut().unwrap().directory = directory.0.join("new-content");
    let mut target = OfflineStorage::open_for_restore(f.cfg.clone(), None).unwrap();
    target.restore(&backup).unwrap();
    drop(target);
    assert_eq!(
        std::fs::read(directory.0.join("security/snapshot")).unwrap(),
        sec
    );
    let e = Engine::new(f.cfg, NOW).unwrap();
    assert_eq!(e.records.len(), 1);
    assert_eq!(e.restart_counter, 2);
}

#[cfg(unix)]
#[test]
fn offline_restore_rejects_a_corrupt_backup_manifest_before_changing_the_target() {
    use crate::content::offline::OfflineStorage;
    let mut f = fixture();
    let directory = content_fixture(&mut f, ContentEncryption::None);
    let mut e = Engine::new(f.cfg.clone(), NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    publish(&mut e, session, json!("kept"), NOW);
    let revision = e.save_content().unwrap();
    wait_content(&e, |s| !s.save_in_progress && s.saved_revision == revision);
    drop(e);
    let store = OfflineStorage::open(f.cfg.clone(), None).unwrap();
    let backup = directory.0.join("backup");
    store.backup(&backup).unwrap();
    drop(store);
    let mut store = OfflineStorage::open(f.cfg, None).unwrap();
    let state = std::fs::read(directory.0.join("content/state.bin")).unwrap();
    let saved = std::fs::read(backup.join("state.bin")).unwrap();
    std::fs::write(backup.join("state.bin"), b"corrupt").unwrap();
    assert!(store.restore(&backup).is_err());
    assert_eq!(
        std::fs::read(directory.0.join("content/state.bin")).unwrap(),
        state
    );

    std::fs::write(backup.join("state.bin"), saved).unwrap();
    store.restore(&backup).unwrap();
}

fn create_runtime_topic(e: &mut Engine, topic: &str, ttl: u64) -> Value {
    admin_data(
        e,
        crate::admin::Command::TopicCreate {
            topic: topic.into(),
            policy: Policy {
                default_ttl_seconds: ttl,
                max_ttl_seconds: ttl * 2,
            },
        },
    )
}
#[test]
fn runtime_topics_default_deny_pagination_validation_and_policy_affect_only_new_messages() {
    use crate::admin::Command as A;
    let f = fixture();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    assert_eq!(
        create_runtime_topic(&mut e, "jobs", 10)["storage"]["mode"],
        "ram"
    );
    let publish_job =
        json!({"version":1,"type":"publish","id":"j","topic":"jobs","payload":"work"});
    assert_eq!(
        send(&mut e, session, publish_job.clone(), NOW)["code"],
        "forbidden"
    );
    assert_eq!(
        e.admin(
            A::TopicCreate {
                topic: "jobs".into(),
                policy: Policy {
                    default_ttl_seconds: 1,
                    max_ttl_seconds: 2
                }
            },
            NOW
        )
        .err()
        .unwrap()
        .code,
        "conflict"
    );
    for name in ["", &"x".repeat(257)] {
        assert!(
            e.admin(
                A::TopicCreate {
                    topic: name.into(),
                    policy: Policy {
                        default_ttl_seconds: 1,
                        max_ttl_seconds: 2
                    }
                },
                NOW
            )
            .is_err()
        );
    }
    assert_eq!(e.topics.len(), 3);
    let page = admin_data(
        &mut e,
        A::TopicsList {
            after: None,
            limit: Some(1),
        },
    );
    assert_eq!(page["items"][0]["topic"], "jobs");
    let page = admin_data(
        &mut e,
        A::TopicsList {
            after: Some("jobs".into()),
            limit: Some(2),
        },
    );
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    assert!(page["next_after"].is_null());
    assert!(
        e.admin(
            A::TopicsList {
                after: None,
                limit: Some(0)
            },
            NOW
        )
        .is_err()
    );
    e.set_acl(&f.ids[0], "jobs", vec![Action::Publish, Action::Subscribe])
        .unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    let first = send(&mut e, session, publish_job.clone(), NOW);
    assert_eq!(first["type"], "result");
    let expires = first["expires"].clone();
    assert!(
        e.admin(
            A::TopicPolicy {
                topic: "jobs".into(),
                policy: Policy {
                    default_ttl_seconds: 3,
                    max_ttl_seconds: 2
                }
            },
            NOW
        )
        .is_err()
    );
    assert_eq!(e.topics["jobs"].policy.default_ttl_seconds, 10);
    admin_data(
        &mut e,
        A::TopicPolicy {
            topic: "jobs".into(),
            policy: Policy {
                default_ttl_seconds: 2,
                max_ttl_seconds: 4,
            },
        },
    );
    let second = send(&mut e, session, publish_job, NOW);
    assert_ne!(first["expires"], second["expires"]);
    let original = get(&mut e, session, first["message_id"].as_str().unwrap(), NOW);
    assert_eq!(original["message"]["expires"], expires, "{original}");
    assert_eq!(
        e.admin(
            A::TopicDelete {
                topic: "jobs".into()
            },
            NOW
        )
        .err()
        .unwrap()
        .code,
        "topic_not_empty"
    );
}
#[test]
fn deleting_empty_topic_remaps_live_records_acl_images_and_invalidates_sessions() {
    use crate::admin::Command as A;
    let f = fixture();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    let message = send(
        &mut e,
        session,
        json!({"version":1,"type":"publish","id":"p","topic":"other","payload":"keep"}),
        NOW,
    );
    admin_data(
        &mut e,
        A::TopicDelete {
            topic: "local/events".into(),
        },
    );
    assert!(!e.authenticated(session, NOW));
    assert_eq!(e.topic_names, vec!["other"]);
    assert_eq!(e.topics["other"].slot, 0);
    let session = auth(&mut e, &f.keys[0], NOW);
    let page = send(
        &mut e,
        session,
        json!({"version":1,"type":"poll","id":"p","topic":"other"}),
        NOW,
    );
    assert_eq!(page["items"][0]["message"]["payload"], "keep", "{page}");
    assert_eq!(
        get(
            &mut e,
            session,
            message["message_id"].as_str().unwrap(),
            NOW
        )["message"]["topic"],
        "other"
    );
    let image = e.capture_message_image(Default::default()).unwrap();
    assert_eq!(image.message_count(), 1);
    create_runtime_topic(&mut e, "local/events", 30);
    assert!(!e.allowed(&f.ids[0], "local/events", Action::Publish));
    assert!(e.allowed(&f.ids[0], "other", Action::Publish));
    let exported = match e.admin(A::Export {}, NOW).unwrap() {
        crate::admin::ResultBody::Configuration(c) => c,
        _ => panic!(),
    };
    exported.validate().unwrap();
    assert!(
        exported
            .topics
            .iter()
            .find(|t| t.name == "local/events")
            .unwrap()
            .acl
            .as_ref()
            .unwrap()
            .is_empty()
    );
    let expiry = NOW + 3_600_000;
    while e.cleanup_step(expiry, 4096).more {}
    assert_eq!(e.storage_stats().resident_records, 0);
}
#[cfg(unix)]
#[test]
fn unsaved_runtime_topic_acl_does_not_break_restart_or_reappear_on_recreation() {
    use crate::admin::Command as A;
    let mut f = fixture();
    let _directory = SecurityDirectory::configure(&mut f.cfg, 128, 4096);
    {
        let mut e = Engine::new(f.cfg.clone(), NOW).unwrap();
        create_runtime_topic(&mut e, "temporary", 30);
        e.set_acl(&f.ids[0], "temporary", vec![Action::Publish])
            .unwrap();
    }
    let mut e = Engine::new(f.cfg.clone(), NOW).unwrap();
    assert!(!e.topics.contains_key("temporary"));
    create_runtime_topic(&mut e, "temporary", 30);
    assert!(!e.allowed(&f.ids[0], "temporary", Action::Publish));
    e.set_acl(&f.ids[0], "temporary", vec![Action::Publish])
        .unwrap();
    admin_data(
        &mut e,
        A::TopicDelete {
            topic: "temporary".into(),
        },
    );
    create_runtime_topic(&mut e, "temporary", 30);
    assert!(!e.allowed(&f.ids[0], "temporary", Action::Publish));
    drop(e);
    Engine::new(f.cfg, NOW).unwrap();
}
#[cfg(unix)]
#[test]
fn runtime_topic_persistence_policy_and_slot_remap_survive_save_and_restart() {
    use crate::admin::Command as A;
    let mut f = fixture();
    let _directory = content_fixture(&mut f, ContentEncryption::None);
    // An empty RAM topic before the persistent slots exercises schedule and record remapping.
    f.cfg.topics[0].storage = TopicStorage::default();
    let mut e = Engine::new(f.cfg.clone(), NOW).unwrap();
    create_runtime_topic(&mut e, "durable-new", 30);
    e.set_acl(
        &f.ids[0],
        "durable-new",
        vec![Action::Publish, Action::Subscribe],
    )
    .unwrap();
    e.enable_topic_storage("durable-new", TopicStorage::Manual { encrypted: false })
        .unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    assert_eq!(
        send(
            &mut e,
            session,
            json!({"version":1,"type":"publish","id":"p","topic":"durable-new","payload":"saved"}),
            NOW
        )["type"],
        "result"
    );
    assert_eq!(
        e.admin(
            A::TopicDelete {
                topic: "durable-new".into()
            },
            NOW
        )
        .err()
        .unwrap()
        .code,
        "storage_transition_forbidden"
    );
    admin_data(
        &mut e,
        A::TopicDelete {
            topic: "local/events".into(),
        },
    );
    admin_data(
        &mut e,
        A::TopicPolicy {
            topic: "durable-new".into(),
            policy: Policy {
                default_ttl_seconds: 12,
                max_ttl_seconds: 60,
            },
        },
    );
    let revision = e.save_content().unwrap();
    wait_content(&e, |s| !s.save_in_progress && s.saved_revision == revision);
    drop(e);
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    assert_eq!(e.topics["durable-new"].policy.default_ttl_seconds, 12);
    assert!(e.allowed(&f.ids[0], "durable-new", Action::Subscribe));
    let session = auth(&mut e, &f.keys[0], NOW);
    let page = send(
        &mut e,
        session,
        json!({"version":1,"type":"poll","id":"p","topic":"durable-new"}),
        NOW,
    );
    assert_eq!(page["items"][0]["message"]["payload"], "saved", "{page}");
}

#[cfg(unix)]
#[test]
fn topic_changes_are_atomic_while_save_reserved_or_security_queue_full() {
    use crate::admin::Command as A;
    let mut f = fixture();
    let _directory = content_fixture(&mut f, ContentEncryption::None);
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    e.content.as_ref().unwrap().reserve().unwrap();
    for command in [
        A::TopicCreate {
            topic: "blocked".into(),
            policy: Policy {
                default_ttl_seconds: 1,
                max_ttl_seconds: 2,
            },
        },
        A::TopicPolicy {
            topic: "other".into(),
            policy: Policy {
                default_ttl_seconds: 1,
                max_ttl_seconds: 2,
            },
        },
        A::TopicDelete {
            topic: "other".into(),
        },
    ] {
        assert_eq!(
            e.admin(command, NOW).err().unwrap().code,
            "content_save_busy"
        );
    }
    e.content.as_ref().unwrap().cancel();
    let permits: Vec<_> = (0..128).map(|_| e.security_reserve().unwrap()).collect();
    assert_eq!(
        e.admin(
            A::TopicCreate {
                topic: "blocked".into(),
                policy: Policy {
                    default_ttl_seconds: 1,
                    max_ttl_seconds: 2
                }
            },
            NOW
        )
        .err()
        .unwrap()
        .code,
        "security_storage_busy"
    );
    assert!(!e.topics.contains_key("blocked"));
    drop(permits);
    create_runtime_topic(&mut e, "allowed", 10);
}

#[test]
fn topic_slot_remap_keeps_existing_confirmation_references_and_recreated_topic_cursors_safe() {
    use crate::admin::Command as A;
    let f = fixture();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    send(
        &mut e,
        session,
        json!({"version":1,"type":"publish","id":"p","topic":"other","payload":"keep"}),
        NOW,
    );
    let _page = send(
        &mut e,
        session,
        json!({"version":1,"type":"poll","id":"p","topic":"other"}),
        NOW,
    );
    admin_data(
        &mut e,
        A::TopicDelete {
            topic: "local/events".into(),
        },
    );
    let _session = auth(&mut e, &f.keys[0], NOW);

    create_runtime_topic(&mut e, "short", 1);
    e.set_acl(&f.ids[0], "short", vec![Action::Publish, Action::Subscribe])
        .unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    send(
        &mut e,
        session,
        json!({"version":1,"type":"publish","id":"p","topic":"short","payload":"old"}),
        NOW,
    );
    let page = send(
        &mut e,
        session,
        json!({"version":1,"type":"poll","id":"p","topic":"short"}),
        NOW,
    );
    let old_cursor = page["next_cursor"].clone();
    let later = NOW + 2000;
    while e.cleanup_step(later, 4096).more {}
    e.admin(
        A::TopicDelete {
            topic: "short".into(),
        },
        later,
    )
    .unwrap();
    e.admin(
        A::TopicCreate {
            topic: "short".into(),
            policy: Policy {
                default_ttl_seconds: 1,
                max_ttl_seconds: 2,
            },
        },
        later,
    )
    .unwrap();
    e.set_acl(&f.ids[0], "short", vec![Action::Publish, Action::Subscribe])
        .unwrap();
    let session = auth(&mut e, &f.keys[0], later);
    send(
        &mut e,
        session,
        json!({"version":1,"type":"publish","id":"p","topic":"short","payload":"new"}),
        later,
    );
    let page = send(
        &mut e,
        session,
        json!({"version":1,"type":"poll","id":"p","topic":"short","after":old_cursor}),
        later,
    );
    assert_eq!(page["items"][0]["message"]["payload"], "new", "{page}");
}

#[test]
fn equivalent_certificate_identities_are_rejected_in_static_and_runtime_peer_policy() {
    use crate::admin::Command as A;

    for (first, second) in [
        ("Edge.Example", "edge.example"),
        ("edge.example", "EDGE.EXAMPLE."),
        ("2001:db8::1", "2001:0db8:0:0:0:0:0:1"),
    ] {
        let mut cfg = fixture().cfg;
        let peer = Peer {
            service_id: first.into(),
            certificate_identity: None,
            topics: vec!["local/events".into()],
        };
        let duplicate = Peer {
            service_id: "other-peer".into(),
            certificate_identity: Some(second.into()),
            topics: peer.topics.clone(),
        };
        cfg.accepted_peers = vec![peer.clone()];
        let mut e = Engine::new(cfg.clone(), NOW).unwrap();
        cfg.accepted_peers.push(duplicate.clone());
        assert_eq!(
            cfg.validate().unwrap_err(),
            DUPLICATE_PEER_CERTIFICATE_IDENTITY
        );
        let revision = *e.link_changes.borrow();
        let error = e
            .admin(
                A::PeerSet {
                    peer: duplicate.clone(),
                },
                NOW,
            )
            .err()
            .expect("duplicate peer must be rejected");
        assert_eq!(error.code, "invalid_request");
        assert_eq!(error.detail, DUPLICATE_PEER_CERTIFICATE_IDENTITY);
        assert_eq!(*e.link_changes.borrow(), revision);
        assert_eq!(e.config.accepted_peers, vec![peer]);

        let mut distinct = duplicate;
        distinct.certificate_identity = Some("distinct.example".into());
        e.admin(A::PeerSet { peer: distinct }, NOW).unwrap();
    }
}

#[test]
fn large_peer_allowlists_validate_independently_of_topic_order() {
    let mut cfg = fixture().cfg;
    let mut topic = cfg.topics[0].clone();
    topic.acl = Some(Vec::new());
    cfg.topics = (0..10_000)
        .map(|i| crate::config::Topic {
            name: format!("topic/{i}"),
            ..topic.clone()
        })
        .collect();
    let topics = cfg.topics[9750..]
        .iter()
        .map(|t| t.name.clone())
        .collect::<Vec<_>>();
    cfg.accepted_peers = (0..120)
        .map(|i| Peer {
            service_id: format!("peer-{i}"),
            certificate_identity: None,
            topics: topics.clone(),
        })
        .collect();
    cfg.validate().unwrap();
    cfg.topics.reverse();
    cfg.validate().unwrap();
    cfg.accepted_peers[119].topics[249] = "unknown".into();
    assert!(cfg.validate().is_err());
}

#[test]
fn upstream_configuration_validates_endpoint_without_network_access() {
    let mut cfg = fixture().cfg;
    let mut upstream = Upstream {
        service_id: "upstream".into(),
        url: String::new(),
        trust: Trust {
            mode: "ca".into(),
            ca_file: "unused-ca.pem".into(),
            expected_certificate_identity: "upstream.example".into(),
        },
    };
    for url in [
        "wss:///path",
        "wss://",
        "ws://upstream.example",
        "wss://:443",
        "wss://upstream.example:",
        "wss://upstream.example:bad",
        "wss://upstream.example:0",
        "wss://upstream.example:65536",
        "wss://[example]:443",
        "wss://[::1]:65536",
        "wss://person@upstream.example",
        "wss://upstream.example/path#fragment",
    ] {
        upstream.url = url.into();
        cfg.upstream = Some(upstream.clone());
        assert_eq!(
            cfg.validate().unwrap_err(),
            "Invalid upstream WebSocket URL",
            "{url}"
        );
    }
    for (url, port) in [
        ("wss://upstream.invalid", 443),
        ("wss://upstream.invalid:8443/path?q=1", 8443),
        ("wss://127.0.0.1:65535", 65535),
        ("wss://[::1]", 443),
        ("wss://[2001:db8::1]:8443", 8443),
    ] {
        upstream.url = url.into();
        cfg.upstream = Some(upstream.clone());
        cfg.validate().unwrap_or_else(|e| panic!("{url}: {e}"));
        assert_eq!(upstream.endpoint().unwrap().1, port);
    }
}

#[test]
fn runtime_peer_policy_is_validated_paged_and_revisions_are_not_reused() {
    use crate::admin::{Command as A, ResultBody};
    let mut e = Engine::new(fixture().cfg, NOW).unwrap();
    let peer = Peer {
        service_id: "edge".into(),
        certificate_identity: None,
        topics: vec!["local/events".into()],
    };
    e.admin(A::PeerSet { peer: peer.clone() }, NOW).unwrap();
    let revision = e.peer_revisions["edge"];
    let mut invalid = peer.clone();
    invalid.topics = vec!["missing".into()];
    assert!(e.admin(A::PeerSet { peer: invalid }, NOW).is_err());
    assert_eq!(e.config.accepted_peers[0], peer);
    assert_eq!(e.peer_revisions["edge"], revision);
    e.admin(A::PeerSet { peer: peer.clone() }, NOW).unwrap();
    assert_eq!(
        e.peer_revisions["edge"], revision,
        "identical update is not a disconnect"
    );
    assert!(
        e.admin(
            A::PeersList {
                after: None,
                limit: Some(0)
            },
            NOW
        )
        .is_err()
    );
    let ResultBody::Data(page) = e
        .admin(
            A::PeersList {
                after: None,
                limit: Some(1),
            },
            NOW,
        )
        .unwrap()
    else {
        panic!("data")
    };
    assert_eq!(page["items"][0]["peer"]["service_id"], "edge");
    e.admin(
        A::PeerDelete {
            service_id: "edge".into(),
        },
        NOW,
    )
    .unwrap();
    e.admin(A::PeerSet { peer }, NOW).unwrap();
    assert!(e.peer_revisions["edge"] > revision);
    assert!(
        e.admin(
            A::PeerDisable {
                service_id: "unknown".into(),
                disabled: true
            },
            NOW
        )
        .is_err()
    );
    assert!(
        e.admin(
            A::PeerCertificateRevoke {
                fingerprint: "arbitrary".into()
            },
            NOW
        )
        .is_err()
    );
    let fingerprint = crate::admin::certificate_fingerprint(b"certificate");
    e.admin(
        A::PeerCertificateRevoke {
            fingerprint: fingerprint.clone(),
        },
        NOW,
    )
    .unwrap();
    e.admin(
        A::PeerCertificateRevoke {
            fingerprint: fingerprint.clone(),
        },
        NOW,
    )
    .unwrap();
    assert_eq!(e.config.revoked_peer_certificates, vec![fingerprint]);
}

#[cfg(unix)]
#[test]
fn runtime_peer_security_survives_journal_recovery_and_export() {
    use crate::admin::{Command as A, ResultBody};
    for compact in [1, 1000] {
        let mut cfg = fixture().cfg;
        let _directory = SecurityDirectory::configure(&mut cfg, 32, compact);
        let mut e = Engine::new(cfg.clone(), NOW).unwrap();
        let peer = Peer {
            service_id: "edge".into(),
            certificate_identity: Some("edge.local".into()),
            topics: vec!["local/events".into()],
        };
        e.admin(A::PeerSet { peer: peer.clone() }, NOW).unwrap();
        e.admin(
            A::PeerDisable {
                service_id: "edge".into(),
                disabled: true,
            },
            NOW,
        )
        .unwrap();
        let fingerprint = crate::admin::certificate_fingerprint(b"revoked");
        e.admin(
            A::PeerCertificateRevoke {
                fingerprint: fingerprint.clone(),
            },
            NOW,
        )
        .unwrap();
        let ResultBody::Configuration(export) = e.admin(A::Export {}, NOW).unwrap() else {
            panic!("export")
        };
        assert_eq!(export.accepted_peers, vec![peer.clone()]);
        drop(e); // Drain accepted journal records.
        let mut e = Engine::new(cfg.clone(), NOW).unwrap();
        assert_eq!(e.config.accepted_peers, vec![peer]);
        assert_eq!(e.config.disabled_peers, vec!["edge"]);
        assert_eq!(e.config.revoked_peer_certificates, vec![fingerprint]);
        e.admin(
            A::PeerDelete {
                service_id: "edge".into(),
            },
            NOW,
        )
        .unwrap();
        drop(e);
        let e = Engine::new(cfg, NOW).unwrap();
        assert!(e.config.accepted_peers.is_empty());
        assert_eq!(
            e.config.disabled_peers,
            vec!["edge"],
            "removal does not silently lift a service block"
        );
    }
}

#[cfg(unix)]
#[test]
fn runtime_peer_changes_reject_full_security_queue_before_policy_or_revision_changes() {
    use crate::admin::Command as A;
    let mut cfg = fixture().cfg;
    let peer = Peer {
        service_id: "edge".into(),
        certificate_identity: None,
        topics: vec!["local/events".into()],
    };
    cfg.accepted_peers = vec![peer.clone()];
    let _directory = SecurityDirectory::configure(&mut cfg, 1, 1000);
    let mut e = Engine::new(cfg, NOW).unwrap();
    let hold = e.security_reserve().unwrap();
    let mut changed = peer;
    changed.certificate_identity = Some("changed.local".into());
    for command in [
        A::PeerSet { peer: changed },
        A::PeerDelete {
            service_id: "edge".into(),
        },
        A::PeerDisable {
            service_id: "edge".into(),
            disabled: true,
        },
        A::PeerCertificateRevoke {
            fingerprint: crate::admin::certificate_fingerprint(b"leaf"),
        },
    ] {
        let before = serde_json::to_value(&e.config).unwrap();
        assert_eq!(
            e.admin(command, NOW).err().unwrap().code,
            "security_storage_busy"
        );
        assert_eq!(serde_json::to_value(&e.config).unwrap(), before);
        assert!(e.peer_revisions.is_empty());
        assert_eq!(*e.link_changes.borrow(), 0);
    }
    drop(hold);
}

#[cfg(unix)]
#[test]
fn runtime_peer_grants_do_not_prevent_recovery_after_unsaved_ram_topic_disappears() {
    use crate::admin::Command as A;
    let mut cfg = fixture().cfg;
    let _directory = SecurityDirectory::configure(&mut cfg, 32, 1000);
    let mut e = Engine::new(cfg.clone(), NOW).unwrap();
    e.admin(
        A::TopicCreate {
            topic: "transient".into(),
            policy: Policy {
                default_ttl_seconds: 300,
                max_ttl_seconds: 3600,
            },
        },
        NOW,
    )
    .unwrap();
    e.admin(
        A::PeerSet {
            peer: Peer {
                service_id: "edge".into(),
                certificate_identity: None,
                topics: vec!["transient".into()],
            },
        },
        NOW,
    )
    .unwrap();
    assert_eq!(
        e.admin(
            A::TopicDelete {
                topic: "transient".into()
            },
            NOW
        )
        .err()
        .unwrap()
        .code,
        "topic_in_use"
    );
    drop(e);
    let e = Engine::new(cfg, NOW).unwrap();
    assert!(!e.topics.contains_key("transient"));
    assert!(
        e.config.accepted_peers.is_empty(),
        "do not retain a dangling peer grant"
    );
}

#[test]
fn owner_stamps_producer_independent_of_payload_and_poll_encoding() {
    use prost::Message;
    let f = fixture();
    let mut engine = Engine::new(f.cfg, NOW).unwrap();
    let writer = auth(&mut engine, &f.keys[0], NOW);
    let reader = auth(&mut engine, &f.keys[1], NOW);
    let published = publish(
        &mut engine,
        writer,
        json!({"actor_id":2,"producer_id":f.ids[1]}),
        NOW,
    );
    assert_eq!(published["type"], "result");
    let json_page = poll(&mut engine, reader, None, 20, NOW);
    let message = &json_page["items"][0]["message"];
    assert_eq!(message["producer_id"], f.ids[0]);
    assert_eq!(message["payload"]["producer_id"], f.ids[1]);
    let own = get(
        &mut engine,
        writer,
        published["message_id"].as_str().unwrap(),
        NOW,
    );
    assert_eq!(own["message"]["producer_id"], f.ids[0]);
    let mut spoof =
        json!({"version":1,"type":"publish","id":"spoof","topic":"local/events","payload":"x"});
    spoof["producer_id"] = f.ids[1].clone().into();
    assert_eq!(send(&mut engine, writer, spoof, NOW)["type"], "error");
    let query = pb::Frame::from_json(
        json!({"version":1,"type":"poll","id":"p","topic":"local/events","limit":20}),
    )
    .unwrap()
    .encode_to_vec();
    let expected = engine.handle_protobuf(reader, &query, NOW);
    let mut bytes = bytes::BytesMut::new();
    assert_eq!(
        engine.handle_protobuf_into(reader, &query, NOW, &mut bytes),
        "messages"
    );
    assert_eq!(pb::Frame::decode(bytes.freeze()).unwrap(), expected);
    let Some(pb::Body::Messages(page)) = expected.body else {
        panic!("messages expected")
    };
    assert_eq!(page.items[0].message.producer_id, f.ids[0]);
    // A custom Protobuf sender may append unknown fields, but cannot select the producer.
    let request = pb::Publish {
        id: "binary-spoof".into(),
        topic: "local/events".into(),
        payload: Payload::bytes(bytes::Bytes::from_static(b"binary"), None),
        expires: None,
    };
    let mut body = bytes::BytesMut::from(request.encode_to_vec().as_slice());
    crate::direct::data(5, f.ids[1].as_bytes(), &mut body);
    let mut frame = bytes::BytesMut::new();
    crate::direct::number(1, 1, &mut frame);
    crate::direct::data(12, &body, &mut frame);
    let result = engine.handle_protobuf(writer, &frame, NOW);
    let Some(pb::Body::Result(result)) = result.body else {
        panic!("publish result expected")
    };
    assert_eq!(
        get(
            &mut engine,
            writer,
            result.message_id.as_deref().unwrap(),
            NOW
        )["message"]["producer_id"],
        f.ids[0]
    );
    assert_eq!(std::mem::size_of::<Stored>(), 80);
}

#[test]
fn native_application_commands_match_json_responses_and_errors() {
    use prost::Message;
    let f = fixture();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let s = auth(&mut e, &f.keys[0], NOW);
    let first = publish(&mut e, s, json!({"text":"one"}), NOW);
    publish(&mut e, s, json!({"text":"two"}), NOW);
    let _page = poll(&mut e, s, None, 20, NOW);
    let requests = vec![
        json!({"version":1,"type":"get_message","id":"g","message_id":first["message_id"]}),
        json!({"version":1,"type":"get_message","id":"g","message_id":"missing"}),
        json!({"version":1,"type":"list_messages","id":"l","topic":"local/events","limit":1}),
        json!({"version":1,"type":"list_messages","id":"l","topic":"","limit":1}),
        json!({"version":1,"type":"get_permissions","id":"p","topic":"local/events"}),
        json!({"version":1,"type":"get_permissions","id":"p","topic":"missing"}),
        json!({"version":1,"type":"get_catalog","id":"c","name":"mynofy","catalog_version":1}),
        json!({"version":1,"type":"get_catalog","id":"c","name":"other","catalog_version":1}),
    ];
    for request in requests {
        let expected = send(&mut e, s, request.clone(), NOW);
        let raw = pb::Frame::from_json(request.clone())
            .unwrap()
            .encode_to_vec();
        let actual = e.handle_protobuf(s, &raw, NOW).to_json().unwrap();
        assert_eq!(actual, expected, "request={request}");
    }
    let stranger = auth(&mut e, &f.keys[1], NOW);
    let query = json!({"version":1,"type":"get_message","id":"g","message_id":first["message_id"]});
    let expected = send(&mut e, stranger, query.clone(), NOW);
    let raw = pb::Frame::from_json(query).unwrap().encode_to_vec();
    assert_eq!(
        e.handle_protobuf(stranger, &raw, NOW).to_json().unwrap(),
        expected
    );
    assert_eq!(expected["code"], "not_found");
}

#[test]
fn request_context_never_survives_acl_change_or_key_revocation() {
    use prost::Message;
    let f = fixture();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    let raw = pb::Frame::from_json(
        json!({"version":1,"type":"publish","id":"p","topic":"local/events","payload":"x"}),
    )
    .unwrap()
    .encode_to_vec();
    assert_eq!(e.handle_protobuf(session, &raw, NOW).kind(), "result");
    e.set_acl(&f.ids[0], "local/events", vec![Action::Subscribe])
        .unwrap();
    assert_eq!(
        e.handle_protobuf(session, &raw, NOW).to_json().unwrap()["code"],
        "unauthorized"
    );
    let again = auth(&mut e, &f.keys[0], NOW);
    assert_eq!(
        e.handle_protobuf(again, &raw, NOW).to_json().unwrap()["code"],
        "forbidden"
    );
    e.revoke_key(&crypto::token_id(&crypto::public_key(&f.keys[0]).unwrap()))
        .unwrap();
    assert_eq!(
        e.handle_protobuf(again, &raw, NOW).to_json().unwrap()["code"],
        "unauthorized"
    );
}
#[test]
fn poll_scratch_is_bounded_and_restored_after_an_error() {
    let f = fixture();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    publish(&mut e, session, json!("one"), NOW);
    publish(&mut e, session, json!("two"), NOW);
    assert_eq!(poll(&mut e, session, None, 1, NOW)["type"], "messages");
    let output_limit = e.config.limits.max_response_bytes;
    e.config.limits.max_response_bytes = 1;
    assert_eq!(poll(&mut e, session, None, 2, NOW)["type"], "error");
    e.config.limits.max_response_bytes = output_limit;
    assert!(e.poll_scratch.is_empty());
    assert!(e.poll_scratch.capacity() <= 100);
    let buffer = e.poll_scratch.as_ptr();
    assert_eq!(poll(&mut e, session, None, 1, NOW)["type"], "messages");
    assert_eq!(buffer, e.poll_scratch.as_ptr());
    assert!(e.poll_scratch.is_empty());
}

#[test]
fn early_rate_limit_rejects_mutation_without_parsing_or_creating_state() {
    use protocol::ProtobufMessage;
    for binary in [false, true] {
        let mut f = fixture();
        f.cfg.limits.client_burst = 1;
        f.cfg.limits.client_requests_per_second = 1;
        let mut e = Engine::new(f.cfg, NOW).unwrap();
        let s = auth(&mut e, &f.keys[0], NOW);
        assert_eq!(poll(&mut e, s, None, 1, NOW)["type"], "messages");
        let value = json!({"version":1,"type":"publish","id":"rejected","topic":"local/events","payload":1});
        let response = if binary {
            let frame = pb::Frame::from_json(value).unwrap();
            e.handle_protobuf(s, &frame.encode_to_vec(), NOW)
                .to_json()
                .unwrap()
        } else {
            e.handle(s, &value.to_string(), NOW)
        };
        assert_eq!(response["code"], "rate_limited");
        assert!(response.get("request_id").is_none());
        assert!(response["retry_after_ms"].as_u64().is_some_and(|ms| ms > 0));
        assert_eq!(e.records.len(), 0);
        assert_eq!(poll(&mut e, s, None, 1, NOW + 1000)["type"], "messages");
    }
}

#[cfg(unix)]
#[test]
fn content_working_budget_rejects_before_capture_and_releases_save_slot() {
    let mut f = fixture();
    let _directory = content_fixture(&mut f, ContentEncryption::None);
    f.cfg.limits.max_content_working_bytes = 1;
    let mut engine = Engine::new(f.cfg.clone(), NOW).unwrap();
    let session = auth(&mut engine, &f.keys[0], NOW);
    publish(&mut engine, session, json!("keep in RAM"), NOW);
    assert_eq!(
        engine.save_content().unwrap_err().code,
        "content_memory_budget_exceeded"
    );
    assert!(!engine.content_status().save_in_progress);
    assert_eq!(engine.content_status().working_memory_reserved_bytes, 0);
    assert_eq!(
        poll(&mut engine, session, None, 20, NOW)["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    engine.config.limits.max_content_working_bytes = 256 * 1024 * 1024;
    let revision = engine.save_content().unwrap();
    wait_content(&engine, |status| status.saved_revision == revision);
    assert_eq!(engine.content_status().working_memory_reserved_bytes, 0);
    drop(engine);
    let restored = Engine::new(f.cfg, NOW).unwrap();
    assert_eq!(restored.records.len(), 1);
}

#[cfg(unix)]
#[test]
fn prepared_password_topics_survive_multiple_saves_restart_and_offline_rekey() {
    use crate::content::{ContentSecret, offline::OfflineStorage};
    let mut f = fixture();
    let _directory = content_fixture(&mut f, ContentEncryption::PasswordStdin);
    let secret = || {
        Some(ContentSecret::Password(Zeroizing::new(
            b"original test password".to_vec(),
        )))
    };
    let replacement =
        || ContentSecret::Password(Zeroizing::new(b"replacement test password".to_vec()));
    let mut e = Engine::new_with_content_secret(f.cfg.clone(), NOW, secret()).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    publish(&mut e, session, json!("topic one"), NOW);
    send(
        &mut e,
        session,
        json!({"version":1,"type":"publish","id":"other","topic":"other","payload":"topic two"}),
        NOW,
    );
    for _ in 0..2 {
        let revision = e.save_content().unwrap();
        wait_content(&e, |status| {
            status.saved_revision == revision && !status.save_in_progress
        });
    }
    drop(e);
    let e = Engine::new_with_content_secret(f.cfg.clone(), NOW, secret()).unwrap();
    assert_eq!(e.records.len(), 2);
    drop(e);
    let mut storage = OfflineStorage::open(f.cfg.clone(), secret()).unwrap();
    storage.rekey(replacement()).unwrap();
    drop(storage);
    assert!(Engine::new_with_content_secret(f.cfg.clone(), NOW, secret()).is_err());
    let e = Engine::new_with_content_secret(f.cfg, NOW, Some(replacement())).unwrap();
    assert_eq!(e.records.len(), 2);
}

#[test]
fn stateless_poll_has_no_reader_retention_quota_or_delivery_fields() {
    let mut f = fixture();
    f.cfg.limits.client_burst = 100_000;
    f.cfg.limits.client_requests_per_second = 100_000;
    let _directory = content_fixture(&mut f, ContentEncryption::None);
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let producer = auth(&mut e, &f.keys[0], NOW);
    let reader = auth(&mut e, &f.keys[1], NOW);
    for n in 0..1100 {
        assert_eq!(publish(&mut e, producer, json!(n), NOW)["type"], "result");
    }
    let before = e.storage_stats();
    let revision = e.content_status().revision;
    let mut after = None;
    let mut total = 0;
    loop {
        let page = poll(&mut e, reader, after.as_deref(), 100, NOW);
        assert_eq!(page["type"], "messages");
        for item in page["items"].as_array().unwrap() {
            assert!(item.get("delivery_id").is_none());
            assert_eq!(item["message"]["producer_id"], f.ids[0]);
        }
        total += page["items"].as_array().unwrap().len();
        after = Some(page["next_cursor"].as_str().unwrap().to_owned());
        if page["has_more"] == false {
            break;
        }
    }
    assert_eq!(total, 1100); // More than the former per-reader 1,000-pair limit.
    assert_eq!(e.storage_stats().resident_records, before.resident_records);
    assert_eq!(e.content_status().revision, revision);
    let replay = poll(&mut e, reader, None, 100, NOW);
    e.disconnect(reader);
    let reader = auth(&mut e, &f.keys[1], NOW);
    assert_eq!(
        poll(&mut e, reader, None, 100, NOW)["items"],
        replay["items"]
    );
    assert!(
        poll(&mut e, reader, after.as_deref(), 100, NOW)["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let mid = replay["items"][0]["message"]["id"].as_str().unwrap();
    assert!(get(&mut e, producer, mid, NOW).get("deliveries").is_none());
}

#[test]
fn removed_confirmation_commands_are_rejected_without_side_effects() {
    let f = fixture();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    for kind in ["ack", "nack"] {
        assert_eq!(
            send(
                &mut e,
                session,
                json!({"version":1,"type":kind,"id":"old","delivery_id":1}),
                NOW
            )["code"],
            "unknown_type"
        );
    }
    // Retired Frame oneof tags 14 and 15 remain unassigned.
    for tag in [14u8, 15] {
        let raw = [8, 1, (tag << 3) | 2, 5, 10, 3, b'o', b'l', b'd'];
        assert!(matches!(
            e.handle_protobuf(session, &raw, NOW).body,
            Some(pb::Body::Error(_))
        ));
    }
    assert_eq!(e.storage_stats().resident_records, 0);
}

#[test]
fn legacy_checkpoint_confirmations_are_validated_discarded_and_not_rewritten() {
    use crate::content::{Checkpoint, checkpoint::TopicState};
    let f = fixture();
    let mut e = Engine::new(f.cfg.clone(), NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    let result = publish(&mut e, session, json!("legacy-content"), NOW);
    let mid = Id::parse(result["message_id"].as_str().unwrap()).unwrap();
    let checkpoint = Checkpoint {
        image: e.capture_message_image(Default::default()).unwrap(),
        revision: 1,
        next_message: e.next_message,
        reserved_ids: e.message_ids.reserved_position(),
        cutoff: NOW,
        produced: e
            .client_names
            .iter()
            .map(|name| e.clients[name].produced)
            .collect(),
        topics: e
            .topic_names
            .iter()
            .map(|name| TopicState {
                head: e.topics[name].head,
                policy: e.topics[name].policy.clone(),
            })
            .collect(),
    };
    let mut bytes = checkpoint.write(1 << 24).unwrap();
    let at = 128 + checkpoint.produced.len() * 8 + checkpoint.topics.len() * 32;
    let mut legacy = [0u8; 24];
    legacy[..8].copy_from_slice(&7u64.to_le_bytes());
    legacy[8..16].copy_from_slice(&mid.0);
    legacy[16..18].copy_from_slice(&e.clients[&f.ids[0]].slot.to_le_bytes());
    legacy[18] = 1; // Legacy ACKed state.
    bytes.splice(at..at, legacy);
    bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
    bytes[40..48].copy_from_slice(&7u64.to_le_bytes());
    fn rehash(bytes: &mut [u8]) {
        let hash = crypto::sha256(&bytes[128..]);
        bytes[64..96].copy_from_slice(&hash);
        let hash = crypto::sha256(&bytes[..96]);
        bytes[96..128].copy_from_slice(&hash);
    }
    rehash(&mut bytes);
    let loaded = Checkpoint::read(&bytes, &f.cfg, 1 << 24).unwrap();
    assert_eq!(loaded.image.message_count(), 1);
    let saved = loaded.write(1 << 24).unwrap();
    assert_eq!(&saved[20..24], &[0; 4]);
    assert_eq!(&saved[40..48], &[0; 8]);
    assert_eq!(saved.len() + 24, bytes.len());
    bytes[at + 18] = 3; // A corrupt obsolete state must still fail validation.
    rehash(&mut bytes);
    assert!(Checkpoint::read(&bytes, &f.cfg, 1 << 24).is_err());
}

#[test]
fn polling_does_not_add_expiry_cleanup_work() {
    let mut f = fixture();
    f.cfg.topics[0].policy.as_mut().unwrap().default_ttl_seconds = 1;
    f.cfg.limits.client_burst = 10_000;
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let a = auth(&mut e, &f.keys[0], NOW);
    let b = auth(&mut e, &f.keys[1], NOW);
    let published = publish(&mut e, a, json!("expires"), NOW);
    for _ in 0..100 {
        for session in [a, b] {
            assert_eq!(
                poll(&mut e, session, None, 1, NOW)["items"]
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
        }
    }
    let progress = e.cleanup_step(NOW + 1000, 1);
    assert_eq!(progress.work, 1);
    assert_eq!(progress.records, 1);
    assert_eq!(e.storage_stats().resident_records, 0);
    assert_eq!(
        get(
            &mut e,
            a,
            published["message_id"].as_str().unwrap(),
            NOW + 1000
        )["code"],
        "not_found"
    );
}

#[test]
fn diagnostics_keep_expired_payloads_and_exact_selected_topic_counts() {
    let f = fixture();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    let producer = e.clients[&f.ids[0]].slot;
    for (topic, count, bytes, ttl) in [
        ("local/events", 90, 7, 1000),
        ("local/events", 10, 13, 2000),
        ("other", 3, 19, 1000),
        ("other", 4, 23, 2000),
    ] {
        for _ in 0..count {
            e.publish_typed(
                producer,
                "p",
                topic,
                Payload::bytes(vec![42; bytes], None),
                Some(protocol::Expires {
                    time: protocol::timestamp(NOW + ttl).unwrap(),
                }),
                NOW,
            )
            .unwrap();
        }
    }
    e.cleanup_step(NOW + 1000, 0);
    let stats = e.storage_stats();
    assert_eq!(stats.resident_records, 107);
    assert_eq!(stats.active_records, 14);
    assert_eq!(stats.expired_resident_records, 93);
    assert_eq!(stats.payload_data_bytes, 90 * 7 + 10 * 13 + 3 * 19 + 4 * 23);
    // Read helpers use the indexes without doing implicit cleanup. Expired
    // residents must not count as active, even at the exact expiry instant.
    let single = e.admin_topic("other", NOW + 1000).unwrap();
    assert_eq!(single["active_messages"], 4);
    assert_eq!(single["resident_messages"], 7);
    let page = e
        .admin_topics_list(Some("local/events".into()), Some(1), NOW + 1000)
        .unwrap();
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0], single);
    let broad = e.admin_topics_list(None, Some(100), NOW + 1000).unwrap();
    assert_eq!(broad["items"][0]["active_messages"], 10);
    assert_eq!(broad["items"][1]["active_messages"], 4);
    assert_eq!(
        e.admin_topic("local/events", NOW + 1000).unwrap()["active_messages"],
        10
    );
    assert_eq!(
        e.admin_topics_list(Some("zzz".into()), Some(1), NOW + 1000)
            .unwrap()["items"],
        json!([])
    );
    assert_eq!(e.records.len(), 107);
    let before = e
        .poll_typed(
            session,
            "p",
            "other",
            None,
            Some(100),
            NOW + 1000,
            true,
            None,
        )
        .unwrap();
    assert_eq!(before.items.len(), 4);
}

#[cfg(unix)]
#[test]
fn lowered_topic_policy_restores_existing_lifetimes_and_discards_expired_records() {
    use crate::admin::Command as A;
    let mut f = fixture();
    let _directory = content_fixture(&mut f, ContentEncryption::None);
    let mut e = Engine::new(f.cfg.clone(), NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    for topic in ["local/events", "other"] {
        let result = topic_publish(&mut e, session, topic, "old lifetime", NOW);
        assert_eq!(result["type"], "result", "{result}");
        e.admin(
            A::TopicPolicy {
                topic: topic.into(),
                policy: Policy {
                    default_ttl_seconds: 1,
                    max_ttl_seconds: 1,
                },
            },
            NOW,
        )
        .unwrap();
    }
    let revision = e.save_content().unwrap();
    wait_content(&e, |s| !s.save_in_progress && s.saved_revision == revision);
    drop(e);
    let mut e = Engine::new(f.cfg.clone(), NOW + 2_000).unwrap();
    assert_eq!(e.records.len(), 2);
    for record in e.records.iter() {
        assert_eq!(record.until(), NOW + 300_000);
    }
    let session = auth(&mut e, &f.keys[0], NOW + 2_000);
    for topic in ["local/events", "other"] {
        assert_eq!(e.topics[topic].policy.max_ttl_seconds, 1);
        let result = topic_publish(&mut e, session, topic, "new lifetime", NOW + 2_000);
        assert_eq!(
            result["expires"]["time"],
            protocol::timestamp(NOW + 3_000).unwrap()
        );
    }
    drop(e);
    // The same saved records must also validate before startup expiry filtering.
    let e = Engine::new(f.cfg.clone(), NOW + 400_000).unwrap();
    assert_eq!(e.records.len(), 0);
    assert_eq!(e.topics["local/events"].head, 1);
}

#[cfg(unix)]
#[test]
fn content_save_fences_pending_security_and_recovers_without_losing_the_previous_checkpoint() {
    use crate::admin::Command as A;
    for recover in [false, true] {
        let mut f = fixture();
        let directory = content_fixture(&mut f, ContentEncryption::None);
        f.cfg
            .security_storage
            .as_mut()
            .unwrap()
            .compact_after_records = 1;
        let mut e = Engine::new(f.cfg.clone(), NOW).unwrap();
        let session = auth(&mut e, &f.keys[0], NOW);
        publish(&mut e, session, json!("committed baseline"), NOW);
        let revision = e.save_content().unwrap();
        wait_content(&e, |s| !s.save_in_progress && s.saved_revision == revision);
        let manifest = directory.0.join("content/state.bin");
        let original = std::fs::read(&manifest).unwrap();
        let blocked = directory.0.join("security/snapshot.tmp");
        std::fs::create_dir(&blocked).unwrap();
        e.set_acl(
            &f.ids[0],
            "local/events",
            vec![Action::Publish, Action::Subscribe],
        )
        .unwrap();
        wait_security(&e, |s| s.last_error.is_some());
        let key = crypto::generate_key().unwrap();
        let public = crypto::public_key(&key).unwrap();
        let client = crypto::initial_client_id(&public);
        e.admin(
            A::ClientAdd {
                public_key: crypto::encode(&public),
                valid_until: None,
                display_name: None,
            },
            NOW,
        )
        .unwrap();
        e.set_acl(
            &client,
            "local/events",
            vec![Action::Publish, Action::Subscribe],
        )
        .unwrap();
        // Registration, ACL and Publish still take effect immediately in RAM.
        let new_session = auth(&mut e, &key, NOW);
        assert_eq!(
            publish(&mut e, new_session, json!("pending identity"), NOW)["type"],
            "result"
        );
        assert!(e.security_status().submitted_sequence > e.security_status().durable_sequence);
        e.save_content().unwrap();
        wait_content(&e, |s| !s.save_in_progress && s.last_error.is_some());
        assert!(e.content_status().dirty);
        assert!(
            e.content_status()
                .last_error
                .unwrap()
                .contains("Security durability fence")
        );
        assert_eq!(e.content_status().working_memory_reserved_bytes, 0);
        assert_eq!(std::fs::read(&manifest).unwrap(), original);
        if recover {
            std::fs::remove_dir(&blocked).unwrap();
            wait_security(&e, |s| {
                s.durable_sequence == s.submitted_sequence && s.last_error.is_none()
            });
            let revision = e.save_content().unwrap();
            wait_content(&e, |s| !s.save_in_progress && s.saved_revision == revision);
            assert!(!e.content_status().dirty);
        }
        drop(e);
        if !recover {
            std::fs::remove_dir(&blocked).unwrap();
        }
        let mut e = Engine::new(f.cfg, NOW + 1).unwrap();
        assert_eq!(e.records.len(), if recover { 2 } else { 1 });
        assert_eq!(e.clients.contains_key(&client), recover);
        if recover {
            let session = auth(&mut e, &key, NOW + 1);
            assert_eq!(
                poll(&mut e, session, None, 10, NOW + 1)["items"]
                    .as_array()
                    .unwrap()
                    .len(),
                2
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn missing_committed_manifest_preserves_topics_but_first_save_orphans_can_recover() {
    let mut f = fixture();
    let directory = content_fixture(&mut f, ContentEncryption::None);
    let mut e = Engine::new(f.cfg.clone(), NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    publish(&mut e, session, json!("first interrupted save"), NOW);
    let staging = directory.0.join("content/state.tmp");
    std::fs::create_dir(&staging).unwrap();
    e.save_content().unwrap();
    wait_content(&e, |s| !s.save_in_progress && s.last_error.is_some());
    drop(e);
    std::fs::remove_dir(&staging).unwrap();
    let orphan = directory
        .0
        .join("content")
        .join(format!("topic-{}-0.bin", "0".repeat(64)));
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&orphan)
            .unwrap();
        file.write_all(b"partial uncommitted first generation")
            .unwrap();
    }
    assert!(orphan.exists());
    let mut e = Engine::new(f.cfg.clone(), NOW).unwrap();
    assert_eq!(e.records.len(), 0);
    assert!(!orphan.exists());
    let session = auth(&mut e, &f.keys[0], NOW);
    publish(&mut e, session, json!("committed"), NOW);
    let revision = e.save_content().unwrap();
    wait_content(&e, |s| !s.save_in_progress && s.saved_revision == revision);
    drop(e);
    let content = directory.0.join("content");
    // A legacy empty lock alongside a valid manifest is upgraded safely on open.
    std::fs::OpenOptions::new()
        .write(true)
        .open(content.join("lock"))
        .unwrap()
        .set_len(0)
        .unwrap();
    drop(Engine::new(f.cfg.clone(), NOW).unwrap());
    let files: Vec<_> = std::fs::read_dir(&content)
        .unwrap()
        .map(|v| v.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("topic-")
        })
        .collect();
    assert!(!files.is_empty());
    let originals: Vec<_> = files.iter().map(|p| std::fs::read(p).unwrap()).collect();
    std::fs::rename(content.join("state.bin"), content.join("recovery-manifest")).unwrap();
    for _ in 0..2 {
        assert!(Engine::new(f.cfg.clone(), NOW).is_err());
        for (path, bytes) in files.iter().zip(&originals) {
            assert_eq!(&std::fs::read(path).unwrap(), bytes);
        }
    }
    std::fs::rename(content.join("recovery-manifest"), content.join("state.bin")).unwrap();
    assert_eq!(Engine::new(f.cfg, NOW).unwrap().records.len(), 1);
}

#[cfg(unix)]
#[test]
fn offline_restore_preserves_observed_time_and_still_allows_fresh_store_recovery() {
    use crate::content::offline::OfflineStorage;
    let mut f = fixture();
    let directory = content_fixture(&mut f, ContentEncryption::None);
    let mut e = Engine::new(f.cfg.clone(), NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    let message = send(
        &mut e,
        session,
        json!({"version":1,"type":"publish","id":"p","topic":"local/events","payload":"old","expires":{"time":protocol::timestamp(NOW+1000).unwrap()}}),
        NOW,
    );
    assert_eq!(message["type"], "result");
    let revision = e.save_content().unwrap();
    wait_content(&e, |s| !s.save_in_progress && s.saved_revision == revision);
    drop(e);
    let backup = directory.0.join("backup");
    OfflineStorage::open(f.cfg.clone(), None)
        .unwrap()
        .backup(&backup)
        .unwrap();
    let mut e = Engine::new(f.cfg.clone(), NOW + 2000).unwrap();
    assert_eq!(e.records.len(), 0);
    let revision = e.save_content().unwrap();
    wait_content(&e, |s| !s.save_in_progress && s.saved_revision == revision);
    drop(e);
    {
        let mut storage = OfflineStorage::open(f.cfg.clone(), None).unwrap();
        storage.restore(&backup).unwrap();
        assert_eq!(storage.check()["active_messages"], 0);
    }
    assert_eq!(Engine::new(f.cfg.clone(), NOW).unwrap().records.len(), 0);
    // Losing the initialized target manifest must not erase its observed cutoff.
    let manifest = directory.0.join("content/state.bin");
    let preserved = directory.0.join("preserved-manifest");
    std::fs::rename(&manifest, &preserved).unwrap();
    let marker = std::fs::read(directory.0.join("content/lock")).unwrap();
    assert_eq!(marker, b"MYNCOM01");
    assert!(Engine::new(f.cfg.clone(), NOW).is_err());
    let error = OfflineStorage::open_for_restore(f.cfg.clone(), None)
        .err()
        .expect("initialized missing manifest must fail");
    assert!(error.to_string().contains("manifest missing"), "{error}");
    assert!(!manifest.exists());
    assert_eq!(
        std::fs::read(directory.0.join("content/lock")).unwrap(),
        marker
    );
    std::fs::rename(&preserved, &manifest).unwrap();
    f.cfg.content_storage.as_mut().unwrap().directory = directory.0.join("fresh-restore");
    {
        let mut storage = OfflineStorage::open_for_restore(f.cfg.clone(), None).unwrap();
        storage.restore(&backup).unwrap();
        assert_eq!(storage.check()["active_messages"], 1);
    }
    assert_eq!(Engine::new(f.cfg, NOW).unwrap().records.len(), 1);
}

#[test]
fn expired_rotation_is_replaced_despite_cleanup_backlog_without_resetting_rate_limits() {
    let mut f = fixture();
    for _ in 0..8 {
        let key = crypto::generate_key().unwrap();
        let public = crypto::public_key(&key).unwrap();
        let mut client = f.cfg.clients[0].clone();
        client.client_id = crypto::initial_client_id(&public);
        client.keys[0].public_key = crypto::encode(&public);
        f.ids.push(client.client_id.clone());
        f.keys.push(key);
        f.cfg.clients.push(client);
    }
    f.cfg.rotation.timeout_seconds = 1;
    f.cfg.rotation.attempts_per_client = 2;
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let mut sessions = std::collections::BTreeMap::new();
    for (key, id) in f.keys.iter().zip(&f.ids) {
        let session = auth(&mut e, key, NOW);
        assert_eq!(
            send(
                &mut e,
                session,
                json!({"version":1,"type":"rotation_start","id":"r"}),
                NOW
            )["type"],
            "rotation_offer"
        );
        sessions.insert(id.clone(), session);
    }
    let (_, session) = sessions.last_key_value().unwrap();
    assert_eq!(
        send(
            &mut e,
            *session,
            json!({"version":1,"type":"rotation_start","id":"r2"}),
            NOW + 1001
        )["type"],
        "rotation_offer"
    );
    assert_eq!(
        send(
            &mut e,
            *session,
            json!({"version":1,"type":"rotation_start","id":"r3"}),
            NOW + 2002
        )["code"],
        "rate_limited"
    );
}

#[cfg(unix)]
#[test]
fn captured_content_is_an_owned_epoch_across_live_expiry_reuse_and_registry_changes() {
    use crate::admin::Command as A;
    let mut f = fixture();
    let _directory = content_fixture(&mut f, ContentEncryption::None);
    let mut ram = f.cfg.topics[0].clone();
    ram.name = "ram-only".into();
    ram.storage = TopicStorage::Ram {};
    f.cfg.topics.push(ram);
    let mut e = Engine::new(f.cfg.clone(), NOW).unwrap();
    let a = auth(&mut e, &f.keys[0], NOW);
    let b = auth(&mut e, &f.keys[1], NOW);
    let original = publish(&mut e, a, json!({"snapshot":"original"}), NOW);
    let other = send(
        &mut e,
        b,
        json!({"version":1,"type":"publish","id":"other","topic":"other","payload":{"kind":"bytes","data":"b3RoZXI=","content_type":"application/other"}}),
        NOW,
    );
    assert_eq!(other["type"], "result");
    let ram = send(
        &mut e,
        a,
        json!({"version":1,"type":"publish","id":"ram","topic":"ram-only","payload":"RAM-ONLY-SECRET-MARKER"}),
        NOW,
    );
    assert_eq!(ram["type"], "result");
    e.content.as_ref().unwrap().reserve().unwrap();
    let selection = vec![
        (
            "local/events".into(),
            TopicStorage::Manual { encrypted: false },
        ),
        ("other".into(), TopicStorage::Manual { encrypted: false }),
    ];
    // Keep only one RAM record, then create enough persistent records for broad capture.
    for _ in 0..8 {
        publish(&mut e, a, json!({"snapshot":"extra"}), NOW);
    }
    let capture = e.capture_content(&selection).unwrap();
    let old_head = e.topics["local/events"].head;
    let old_next = e.next_message;
    let old_produced: Vec<_> = e
        .client_names
        .iter()
        .map(|name| e.clients[name].produced)
        .collect();
    // Expire and reclaim all live records/pages, then reuse storage and add an identity.
    let later = NOW + 400_000;
    for _ in 0..64 {
        e.maintenance(later);
    }
    assert_eq!(e.records.len(), 0);
    let fresh = publish(&mut e, a, json!("after capture"), later);
    assert_eq!(fresh["type"], "result");
    let key = crypto::generate_key().unwrap();
    e.admin(
        A::ClientAdd {
            public_key: crypto::encode(&crypto::public_key(&key).unwrap()),
            valid_until: None,
            display_name: Some("after-capture".into()),
        },
        later,
    )
    .unwrap();
    let image = capture.finish(&selection, 128 * 1024 * 1024).unwrap();
    assert_eq!(image.image.message_count(), 10);
    assert_eq!(image.next_message, old_next);
    assert_eq!(image.produced, old_produced);
    assert_eq!(image.cutoff, NOW);
    assert_eq!(image.topics[0].head, old_head);
    assert_eq!(image.image.clients.len(), 2);
    let captured_other = image
        .image
        .message(other["message_id"].as_str().unwrap())
        .unwrap();
    assert_eq!(
        captured_other.payload.content_type.as_deref(),
        Some("application/other")
    );
    assert_eq!(
        captured_other.payload.to_json().unwrap()["content_type"],
        "application/other"
    );

    assert_eq!(image.image.topics, vec!["local/events", "other"]);
    assert_eq!(
        image
            .image
            .message(original["message_id"].as_str().unwrap())
            .unwrap()
            .payload
            .to_json()
            .unwrap(),
        json!({"snapshot":"original"})
    );
    assert!(
        image
            .image
            .message(fresh["message_id"].as_str().unwrap())
            .is_none()
    );
    let bytes = image.write(128 * 1024 * 1024).unwrap();
    assert!(
        !bytes
            .windows(b"RAM-ONLY-SECRET-MARKER".len())
            .any(|w| w == b"RAM-ONLY-SECRET-MARKER")
    );
    e.content.as_ref().unwrap().cancel();
    drop(e);
    let restored = crate::content::Checkpoint::read(&bytes, &f.cfg, 128 * 1024 * 1024).unwrap();
    assert_eq!(restored.image.message_count(), 10);
    assert_eq!(restored.produced, old_produced);
}

#[cfg(unix)]
#[test]
fn selected_save_ignores_large_ram_topics_and_never_writes_their_content() {
    let mut f = fixture();
    let directory = content_fixture(&mut f, ContentEncryption::None);
    f.cfg.topics[0].storage = TopicStorage::Interval {
        seconds: 60,
        encrypted: false,
    };
    f.cfg.topics[1].storage = TopicStorage::Ram {};
    f.cfg.content_storage.as_mut().unwrap().max_snapshot_bytes = 65536;
    f.cfg.limits.client_burst = 1000;
    f.cfg.limits.max_content_working_bytes = 2 * 1024 * 1024;
    let mut e = Engine::new(f.cfg.clone(), NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    let original = publish(&mut e, session, json!("small persistent message"), NOW);
    let status = e
        .admin(
            crate::admin::Command::ContentTopicStatus {
                topic: "local/events".into(),
            },
            NOW,
        )
        .unwrap();
    let crate::admin::ResultBody::Data(status) = status else {
        panic!("expected topic details")
    };
    assert_eq!(
        status,
        serde_json::to_value(&e.content_status().topics["local/events"]).unwrap()
    );
    assert!(status["dirty"].as_bool().unwrap());
    assert!(
        e.admin(
            crate::admin::Command::ContentTopicStatus {
                topic: "other".into()
            },
            NOW,
        )
        .is_err()
    );
    let marker = b"PRIVATE-RAM-TOPIC-MARKER";
    let mut bytes = vec![b'x'; 4096];
    bytes[..marker.len()].copy_from_slice(marker);
    for i in 0..100 {
        let reply = send(
            &mut e,
            session,
            json!({"version":1,"type":"publish","id":format!("ram-{i}"),"topic":"other","payload":{"kind":"bytes","data":crypto::encode(&bytes)}}),
            NOW,
        );
        assert_eq!(reply["type"], "result", "{reply}");
    }
    assert!(e.payloads.allocated_bytes() > 65536);
    e.content.as_ref().unwrap().reserve().unwrap();
    let capture = e
        .capture_content(&[(
            "local/events".into(),
            TopicStorage::Manual { encrypted: false },
        )])
        .unwrap();
    assert_eq!(capture.checkpoint.image.message_count(), 1);
    assert!(capture.checkpoint.image.blobs.allocated_bytes() < 65536);
    drop(capture);
    e.content.as_ref().unwrap().cancel();

    let revision = e.save_content().unwrap();
    wait_content(&e, |status| !status.save_in_progress);
    assert_eq!(e.content_status().saved_revision, revision);
    assert!(e.content_status().last_error.is_none());
    assert_eq!(
        publish(&mut e, session, json!("small autosaved message"), NOW)["type"],
        "result"
    );
    e.content.as_mut().unwrap().force_due();
    e.maintenance(NOW);
    wait_content(&e, |status| {
        status.saved_revision > revision && !status.save_in_progress
    });
    assert!(e.content_status().last_error.is_none());
    for entry in std::fs::read_dir(directory.0.join("content")).unwrap() {
        let p = entry.unwrap().path();
        if p.is_file() {
            let data = std::fs::read(p).unwrap();
            assert!(!data.windows(marker.len()).any(|w| w == marker));
        }
    }
    drop(e);
    let mut restored = Engine::new(f.cfg, NOW).unwrap();
    assert_eq!(restored.records.len(), 2);
    let session = auth(&mut restored, &f.keys[0], NOW);
    assert_eq!(
        get(
            &mut restored,
            session,
            original["message_id"].as_str().unwrap(),
            NOW
        )["type"],
        "message_status"
    );
}

#[cfg(unix)]
#[test]
fn worker_projection_failure_preserves_manifest_dirty_state_and_save_budget() {
    let mut f = fixture();
    let directory = content_fixture(&mut f, ContentEncryption::None);
    f.cfg.content_storage.as_mut().unwrap().max_snapshot_bytes = 65536;
    f.cfg.limits.client_burst = 1000;
    let mut e = Engine::new(f.cfg.clone(), NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    publish(
        &mut e,
        session,
        json!("saved before oversized projection"),
        NOW,
    );
    let revision = e.save_content().unwrap();
    wait_content(&e, |s| s.saved_revision == revision && !s.save_in_progress);
    let manifest = directory.0.join("content/state.bin");
    let original = std::fs::read(&manifest).unwrap();
    let payload = json!({"kind":"bytes","data":crypto::encode(&[7;4096])});
    for _ in 0..32 {
        assert_eq!(
            publish(&mut e, session, payload.clone(), NOW)["type"],
            "result"
        );
    }
    e.save_content().unwrap();
    wait_content(&e, |s| !s.save_in_progress && s.last_error.is_some());
    assert!(e.content_status().dirty);
    assert_eq!(e.content_status().working_memory_reserved_bytes, 0);
    assert_eq!(std::fs::read(&manifest).unwrap(), original);
    assert_eq!(e.records.len(), 33);
    e.content.as_ref().unwrap().reserve().unwrap();
    e.content.as_ref().unwrap().cancel();
    drop(e);
    assert_eq!(Engine::new(f.cfg, NOW).unwrap().records.len(), 1);
}

#[test]
fn basic_status_never_runs_storage_diagnostics_and_explicit_stats_keep_expired_residents() {
    use crate::admin::{Command as A, ResultBody};
    let f = fixture();
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let producer = e.clients[&f.ids[0]].slot;
    for i in 0..257 {
        e.publish_typed(
            producer,
            "p",
            "local/events",
            Payload::bytes(vec![42; if i == 256 { 7 } else { 32 }], None),
            Some(protocol::Expires {
                time: protocol::timestamp(NOW + if i == 256 { 5000 } else { 1000 }).unwrap(),
            }),
            NOW,
        )
        .unwrap();
    }
    let before = STORAGE_STATS_SCANS.with(|scans| scans.get());
    let ResultBody::Data(status) = e.admin(A::Status {}, NOW + 1000).unwrap() else {
        panic!("status data")
    };
    assert_eq!(STORAGE_STATS_SCANS.with(|scans| scans.get()), before);
    assert!(status.get("storage").is_none());
    assert!(status.get("active_records").is_none());
    assert_eq!(status["resident_messages"], e.records.len());
    assert!(e.records.len() > 1); // Bounded normal cleanup leaves expired residents.
    let ResultBody::Data(stats) = e.admin(A::StorageStats {}, NOW + 1000).unwrap() else {
        panic!("storage data")
    };
    assert_eq!(STORAGE_STATS_SCANS.with(|scans| scans.get()), before + 1);
    assert_eq!(stats["active_records"], 1);
    assert_eq!(stats["resident_records"], e.records.len());
    assert_eq!(stats["expired_resident_records"], e.records.len() - 1);
    assert_eq!(stats["payload_data_bytes"], (e.records.len() - 1) * 32 + 7);
    assert_eq!(stats["record_bytes"], 80);
    e.admin(A::Status {}, NOW + 1000).unwrap();
    assert_eq!(STORAGE_STATS_SCANS.with(|scans| scans.get()), before + 1);
}

#[cfg(unix)]
#[test]
fn offline_topic_ram_keeps_bindings_of_untouched_expired_producers_on_reopen() {
    use crate::content::offline::OfflineStorage;
    let mut f = fixture();
    let _directory = content_fixture(&mut f, ContentEncryption::None);
    let mut third = f.cfg.topics[0].clone();
    third.name = "converted".into();
    f.cfg.topics.push(third);
    let now = crate::transport::now_ms() - 5000;
    let mut e = Engine::new(f.cfg.clone(), now).unwrap();
    let old = auth(&mut e, &f.keys[0], now);
    let live = auth(&mut e, &f.keys[1], now);
    assert_eq!(
        send(
            &mut e,
            old,
            json!({"version":1,"type":"publish","id":"old","topic":"local/events","payload":"expired producer","expires":{"time":protocol::timestamp(now+1000).unwrap()}}),
            now
        )["type"],
        "result"
    );
    let message = send(
        &mut e,
        live,
        json!({"version":1,"type":"publish","id":"live","topic":"other","payload":"survives offline conversion"}),
        now,
    );
    assert_eq!(message["type"], "result");
    let revision = e.save_content().unwrap();
    wait_content(&e, |s| !s.save_in_progress && s.saved_revision == revision);
    drop(e);
    f.cfg.topics[2].storage = TopicStorage::Ram {};
    {
        let mut store = OfflineStorage::open(f.cfg.clone(), None).unwrap();
        assert_eq!(store.check()["active_messages"], 1);
        store.topic_to_ram("converted").unwrap();
        assert_eq!(store.check()["active_messages"], 1);
    }
    assert_eq!(
        OfflineStorage::open(f.cfg.clone(), None).unwrap().check()["active_messages"],
        1
    );
    let now = crate::transport::now_ms();
    let mut reopened = Engine::new(f.cfg, now).unwrap();
    let session = auth(&mut reopened, &f.keys[1], now);
    let page = send(
        &mut reopened,
        session,
        json!({"version":1,"type":"poll","id":"check","topic":"other"}),
        now,
    );
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0]["message"]["id"], message["message_id"]);
    assert_eq!(page["items"][0]["message"]["producer_id"], f.ids[1]);
    assert_eq!(
        page["items"][0]["message"]["payload"],
        "survives offline conversion"
    );
}

#[cfg(unix)]
#[test]
fn combined_topic_replacement_budgets_the_final_generation_in_both_orders() {
    for reverse in [false, true] {
        let mut f = fixture();
        let directory = content_fixture(&mut f, ContentEncryption::None);
        f.cfg.content_storage.as_mut().unwrap().max_snapshot_bytes = 90000;
        if reverse {
            f.cfg.topics.reverse();
        }
        let mut e = Engine::new(f.cfg.clone(), NOW).unwrap();
        let producer = e.clients[&f.ids[0]].slot;
        for _ in 0..7 {
            e.publish_typed(
                producer,
                "old",
                "other",
                Payload::bytes(vec![7; 3700], None),
                Some(protocol::Expires {
                    time: protocol::timestamp(NOW + 1000).unwrap(),
                }),
                NOW,
            )
            .unwrap();
        }
        let revision = e.save_content().unwrap();
        wait_content(&e, |s| !s.save_in_progress && s.saved_revision == revision);
        drop(e);
        let now = NOW + 2000;
        let mut e = Engine::new(f.cfg.clone(), now).unwrap();
        assert_eq!(e.records.len(), 0);
        let producer = e.clients[&f.ids[0]].slot;
        for _ in 0..7 {
            e.publish_typed(
                producer,
                "new",
                "local/events",
                Payload::bytes(vec![9; 3700], None),
                None,
                now,
            )
            .unwrap();
        }
        let manifest = directory.0.join("content/state.bin");
        let previous = std::fs::read(&manifest).unwrap();
        // An untouched old B still consumes disk space: replacing only A must fail.
        e.save_content_topics(Some(&["local/events".to_owned()]))
            .unwrap();
        wait_content(&e, |s| !s.save_in_progress);
        assert!(
            e.content_status()
                .last_error
                .as_ref()
                .is_some_and(|e| e.contains("aggregate size"))
        );
        assert_eq!(std::fs::read(&manifest).unwrap(), previous);
        let revision = e.save_content().unwrap();
        wait_content(&e, |s| !s.save_in_progress);
        assert!(
            e.content_status().last_error.is_none(),
            "reverse={reverse}: {:?}",
            e.content_status().last_error
        );
        assert_eq!(e.content_status().saved_revision, revision);
        drop(e);
        let mut reopened = Engine::new(f.cfg, now).unwrap();
        assert_eq!(reopened.records.len(), 7);
        let session = auth(&mut reopened, &f.keys[0], now);
        let page = poll(&mut reopened, session, None, 20, now);
        assert_eq!(page["items"].as_array().unwrap().len(), 7);
        for item in page["items"].as_array().unwrap() {
            assert_eq!(
                item["message"]["payload"]["data"],
                crypto::encode(&[9; 3700])
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn empty_ram_topic_names_do_not_consume_disk_budget_in_both_capture_paths() {
    let mut f = fixture();
    let directory = content_fixture(&mut f, ContentEncryption::None);
    f.cfg.content_storage.as_mut().unwrap().max_snapshot_bytes = 65536;
    f.cfg.topics[1].storage = TopicStorage::Ram {};
    let base = f.cfg.topics[1].clone();
    for i in 0..2199 {
        let mut topic = base.clone();
        topic.name = format!("ram/{i}");
        f.cfg.topics.push(topic);
    }
    let mut e = Engine::new(f.cfg.clone(), NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    let message = publish(&mut e, session, json!("persistent"), NOW);
    assert_eq!(message["type"], "result");
    // First broad, then indexed: one persistent and one RAM record.
    for sparse in [false, true] {
        if sparse {
            assert_eq!(
                send(
                    &mut e,
                    session,
                    json!({"version":1,"type":"publish","id":"ram","topic":"other","payload":"RAM only"}),
                    NOW
                )["type"],
                "result"
            );
        }
        let revision = e.save_content().unwrap();
        wait_content(&e, |s| !s.save_in_progress);
        assert!(
            e.content_status().last_error.is_none(),
            "sparse={sparse}: {:?}",
            e.content_status().last_error
        );
        assert_eq!(e.content_status().saved_revision, revision);
    }
    drop(e);
    let mut reopened = Engine::new(f.cfg, NOW).unwrap();
    assert_eq!(reopened.records.len(), 1);
    let revision = reopened.save_content().unwrap();
    wait_content(&reopened, |s| !s.save_in_progress);
    assert!(reopened.content_status().last_error.is_none());
    assert_eq!(reopened.content_status().saved_revision, revision);
    let total: u64 = std::fs::read_dir(directory.0.join("content"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|s| s == "bin"))
        .map(|p| std::fs::metadata(p).unwrap().len())
        .sum();
    assert!(total <= 65536, "{total}");
}

#[cfg(unix)]
#[test]
fn topic_encryption_conflicts_require_an_offline_rewrite() {
    use crate::content::{ContentSecret, offline::OfflineStorage};
    let now = crate::transport::now_ms();
    let mut f = fixture();
    for client in &mut f.cfg.clients {
        for key in &mut client.keys {
            key.valid_until = protocol::timestamp(now + 86_400_000).unwrap();
        }
    }
    let directory = content_fixture(&mut f, ContentEncryption::KeyStdin);
    let secret = || Some(ContentSecret::Key(Zeroizing::new([7; 32])));
    let mut e = Engine::new_with_content_secret(f.cfg.clone(), now, secret()).unwrap();
    let session = auth(&mut e, &f.keys[0], now);
    assert_eq!(
        publish(&mut e, session, json!("migrate"), now)["type"],
        "result"
    );
    let revision = e.save_content().unwrap();
    wait_content(&e, |status| status.saved_revision == revision);
    drop(e);

    let backup = directory.0.join("backup");
    OfflineStorage::open(f.cfg.clone(), secret())
        .unwrap()
        .backup(&backup)
        .unwrap();
    let manifest = directory.0.join("content/state.bin");
    for encrypted in [false, true] {
        f.cfg
            .topics
            .iter_mut()
            .find(|topic| topic.name == "local/events")
            .unwrap()
            .storage = TopicStorage::Manual { encrypted };
        let before = std::fs::read(&manifest).unwrap();
        assert!(Engine::new_with_content_secret(f.cfg.clone(), now, secret()).is_err());
        assert!(OfflineStorage::open_for_restore(f.cfg.clone(), secret()).is_err());
        assert_eq!(std::fs::read(&manifest).unwrap(), before);
        let mut store = OfflineStorage::open(f.cfg.clone(), secret()).unwrap();
        store.migrate_topic_encryption("local/events").unwrap();
        drop(store);
        let e = Engine::new_with_content_secret(f.cfg.clone(), now, secret()).unwrap();
        assert_eq!(e.records.len(), 1);
        assert_eq!(
            e.config
                .topics
                .iter()
                .find(|topic| topic.name == "local/events")
                .unwrap()
                .storage
                .encrypted(),
            encrypted
        );
        drop(e);
        if !encrypted {
            let before = std::fs::read(&manifest).unwrap();
            let mut store = OfflineStorage::open_for_restore(f.cfg.clone(), secret()).unwrap();
            let error = store.restore(&backup).unwrap_err();
            assert!(error.to_string().contains("encryption differs"));
            assert_eq!(std::fs::read(&manifest).unwrap(), before);
        }
    }
}

#[cfg(unix)]
#[test]
fn committed_save_reports_and_retries_old_slot_cleanup() {
    let mut f = fixture();
    let directory = content_fixture(&mut f, ContentEncryption::None);
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    publish(&mut e, session, json!("first"), NOW);
    let revision = e.save_content().unwrap();
    wait_content(&e, |s| s.saved_revision == revision && !s.save_in_progress);

    let old_file = std::fs::read_dir(directory.0.join("content"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("topic-")
        })
        .unwrap();
    // A directory at the old slot path makes unlink fail without special privileges.
    std::fs::remove_file(&old_file).unwrap();
    std::fs::create_dir(&old_file).unwrap();
    publish(&mut e, session, json!("second"), NOW);
    let revision = e.save_content().unwrap();
    wait_content(&e, |s| s.saved_revision == revision && !s.save_in_progress);
    let status = e.content_status();
    assert!(status.last_error.is_none());
    assert!(status.cleanup_error.is_some());

    std::fs::remove_dir(&old_file).unwrap();
    std::fs::write(&old_file, b"unreferenced slot awaiting cleanup").unwrap();
    wait_content(&e, |s| s.cleanup_error.is_none());
    assert!(!old_file.exists());
    assert_eq!(e.content_status().saved_revision, revision);
}

#[test]
fn publisher_and_topic_budgets_isolate_other_topics_and_reclaim_on_expiry() {
    let mut f = fixture();
    f.cfg.limits.max_active_messages = 4;
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let publisher = e.clients[&f.ids[0]].slot;
    let independent = e.clients[&f.ids[1]].slot;
    for _ in 0..2 {
        e.publish_typed(
            publisher,
            "p",
            "local/events",
            Payload::bytes(vec![1], None),
            Some(protocol::Expires {
                time: protocol::timestamp(NOW + 1000).unwrap(),
            }),
            NOW,
        )
        .unwrap();
    }
    let payload = || Payload::bytes(vec![2], None);
    let denied = e
        .publish_typed(publisher, "p", "local/events", payload(), None, NOW)
        .unwrap_err();
    assert_eq!(denied.code, "resource_exhausted");
    let other = e.publish_typed(independent, "p", "other", payload(), None, NOW);
    assert!(other.is_ok(), "{other:?}");
    e.maintenance(NOW + 1000);
    e.publish_typed(publisher, "p", "local/events", payload(), None, NOW + 1000)
        .unwrap();
}

#[cfg(unix)]
#[test]
fn sparse_worker_projection_keeps_only_selected_payloads_from_capture_epoch() {
    let mut f = fixture();
    let _store = content_fixture(&mut f, ContentEncryption::None);
    f.cfg.topics[1].storage = TopicStorage::Ram {};
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let producer = e.clients[&f.ids[0]].slot;
    for _ in 0..300 {
        for (topic, byte) in [("local/events", 7), ("other", 9)] {
            e.publish_typed(
                producer,
                "p",
                topic,
                Payload::bytes(vec![byte; 256], Some(format!("application/t{byte}"))),
                Some(protocol::Expires {
                    time: protocol::timestamp(NOW + 1000).unwrap(),
                }),
                NOW,
            )
            .unwrap();
        }
    }
    let selection = [(
        "local/events".into(),
        TopicStorage::Manual { encrypted: false },
    )];
    e.content.as_ref().unwrap().reserve().unwrap();
    let snapshot = e.capture_content(&selection).unwrap();
    assert_eq!(snapshot.indices.as_ref().unwrap().len(), 300);
    let first = e.records.iter().next().unwrap().id().to_string();

    for _ in 0..16 {
        e.maintenance(NOW + 1000);
    }
    assert_eq!(e.records.len(), 0);
    e.publish_typed(
        producer,
        "p",
        "local/events",
        Payload::bytes(vec![3; 256], None),
        None,
        NOW + 1000,
    )
    .unwrap();

    let projected = snapshot.finish(&selection, 128 * 1024 * 1024).unwrap();
    assert_eq!(projected.image.message_count(), 300);
    let message = projected.image.message(&first).unwrap();
    assert_eq!(
        message.payload.content_type.as_deref(),
        Some("application/t7")
    );
    assert_eq!(
        message.payload,
        Payload::bytes(vec![7; 256], Some("application/t7".into()))
    );
}

#[cfg(unix)]
#[test]
fn failed_autosave_uses_configured_retry_before_regular_interval() {
    let mut f = fixture();
    let _dir = content_fixture(&mut f, ContentEncryption::None);
    f.cfg.topics[0].storage = TopicStorage::Interval {
        seconds: 86_400,
        encrypted: false,
    };
    f.cfg.topics[1].storage = TopicStorage::Ram {};
    f.cfg.limits.max_content_working_bytes = 1;
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    assert_eq!(
        publish(&mut e, session, json!("retry"), NOW)["type"],
        "result"
    );
    e.content.as_mut().unwrap().force_due();
    e.maintenance(NOW);
    assert!(e.content_status().last_error.is_some());
    assert!(e.content_status().dirty);

    e.config.limits.max_content_working_bytes = 256 * 1024 * 1024;
    e.maintenance(NOW);
    assert!(!e.content_status().save_in_progress);
    std::thread::sleep(std::time::Duration::from_millis(1050));
    e.maintenance(NOW);
    wait_content(&e, |status| !status.dirty && !status.save_in_progress);
    assert!(e.content_status().last_error.is_none());
}

#[cfg(unix)]
#[test]
fn autosave_capacity_error_isolates_small_topic_from_oversized_topic() {
    let mut f = fixture();
    let _directory = content_fixture(&mut f, ContentEncryption::None);
    f.cfg.content_storage.as_mut().unwrap().max_snapshot_bytes = 65_536;
    for topic in &mut f.cfg.topics {
        topic.storage = TopicStorage::Interval {
            seconds: 3600,
            encrypted: false,
        };
    }
    let mut e = Engine::new(f.cfg, NOW).unwrap();
    let session = auth(&mut e, &f.keys[0], NOW);
    assert_eq!(
        publish(&mut e, session, json!("small"), NOW)["type"],
        "result"
    );
    let producer = e.clients[&f.ids[0]].slot;
    for _ in 0..40 {
        e.publish_typed(
            producer,
            "large",
            "other",
            Payload::bytes(vec![7; 4096], None),
            None,
            NOW,
        )
        .unwrap();
    }
    e.content.as_mut().unwrap().force_due();
    e.maintenance(NOW);
    wait_content(&e, |s| s.last_error.is_some() && !s.save_in_progress);
    e.maintenance(NOW);
    assert!(e.content_status().topics["other"].autosave_isolated);
    std::thread::sleep(std::time::Duration::from_millis(1050));
    e.maintenance(NOW);
    wait_content(&e, |s| {
        !s.topics["local/events"].dirty && !s.save_in_progress
    });
    assert!(e.content_status().topics["other"].dirty);
}

#[test]
fn routing_index_tracks_registry_and_supports_large_configurations() {
    use crate::admin::Command;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    };
    use std::time::Instant;

    let mut f = fixture();
    let initial = f.cfg.topics[0].clone();
    let owner = f.cfg.service.id.clone();
    for index in 0..(9_999 - f.cfg.topics.len()) {
        let mut topic = initial.clone();
        topic.name = format!("route/{index}");
        topic.acl = Some(Vec::new());
        f.cfg.topics.push(topic);
    }
    let mut engine = Engine::new(f.cfg, NOW).unwrap();
    assert_eq!(engine.topic_owner("local/events"), Some(owner.as_str()));
    assert_eq!(engine.topic_owner("route/missing"), None);
    admin_data(
        &mut engine,
        Command::TopicCreate {
            topic: "route/dynamic".into(),
            policy: initial.policy.unwrap(),
        },
    );
    assert_eq!(engine.topic_owner("route/dynamic"), Some(owner.as_str()));
    admin_data(
        &mut engine,
        Command::TopicDelete {
            topic: "route/dynamic".into(),
        },
    );
    assert_eq!(engine.topic_owner("route/dynamic"), None);

    if std::env::var_os("MYNOFY_MEASURE_ROUTE_LOOKUP").is_none() {
        return;
    }
    let known = engine.config.topics.last().unwrap().name.clone();
    let session = auth(&mut engine, &f.keys[0], NOW);
    let producer = engine.clients[&f.ids[0]].slot;
    let engine = Arc::new(Mutex::new(engine));
    let done = AtomicBool::new(false);
    std::thread::scope(|scope| {
        for publish in [true, false] {
            let engine = &engine;
            let done = &done;
            scope.spawn(move || {
                let payload = Payload::bytes(vec![1; 64], None);
                let mut operations = 0;
                while !done.load(Ordering::Relaxed) {
                    let mut engine = engine.lock().unwrap();
                    if publish {
                        let _ = engine.publish_typed(
                            producer,
                            "pub",
                            "local/events",
                            payload.clone(),
                            None,
                            NOW,
                        );
                    } else {
                        let page = engine
                            .poll_typed(
                                session,
                                "poll",
                                "local/events",
                                None,
                                Some(1),
                                NOW,
                                false,
                                None,
                            )
                            .unwrap();
                        std::hint::black_box(page);
                    }
                    operations += 1;
                    drop(engine);
                    std::thread::sleep(std::time::Duration::from_micros(25));
                }
                println!("route background publish={publish} operations={operations}");
            });
        }
        for round in 0..3 {
            for indexed in [round % 2 == 0, round % 2 != 0] {
                for topic in [&known, &"route/missing".to_owned()] {
                    let mut ns = Vec::with_capacity(10_000);
                    for _ in 0..ns.capacity() {
                        let started = Instant::now();
                        let engine = engine.lock().unwrap();
                        let result = if indexed {
                            engine.topic_owner(std::hint::black_box(topic))
                        } else {
                            engine
                                .config
                                .topics
                                .iter()
                                .find(|entry| entry.name == *std::hint::black_box(topic))
                                .map(|entry| entry.owner_id.as_str())
                        };
                        std::hint::black_box(result);
                        drop(engine);
                        ns.push(started.elapsed().as_nanos());
                    }
                    ns.sort_unstable();
                    println!(
                        "route round={round} indexed={indexed} hit={} p50_ns={} p95_ns={} p99_ns={}",
                        *topic == known,
                        ns[5000],
                        ns[9500],
                        ns[9900]
                    );
                }
            }
        }
        done.store(true, Ordering::Relaxed);
    });
}

#[test]
fn producer_list_cursor_renews_after_every_page() {
    let f = fixture();
    let mut engine = Engine::new(f.cfg, NOW).unwrap();
    let session = auth(&mut engine, &f.keys[0], NOW);
    for value in 0..5 {
        publish(&mut engine, session, json!(value), NOW);
    }
    let mut cursor = None;
    for index in 0..5 {
        let now = NOW + index * 20_000;
        let page = engine
            .list_typed(
                &f.ids[0],
                "list",
                Some("local/events".into()),
                Some(1),
                cursor,
                now,
                false,
            )
            .unwrap();
        assert_eq!(page.items.len(), 1);
        cursor = page.next_cursor;
    }
    assert!(cursor.is_none());
}

#[cfg(unix)]
#[test]
fn recovered_security_sequence_must_reach_configured_floor() {
    let mut f = fixture();
    let _directory = SecurityDirectory::configure(&mut f.cfg, 128, 4096);
    drop(Engine::new(f.cfg.clone(), NOW).unwrap());
    f.cfg.security_storage.as_mut().unwrap().minimum_sequence = Some(1);
    assert!(Engine::new(f.cfg.clone(), NOW).is_err());
    f.cfg.security_storage.as_mut().unwrap().minimum_sequence = Some(0);
    assert!(Engine::new(f.cfg, NOW).is_ok());
}

#[cfg(unix)]
#[test]
fn failed_group_save_requires_success_for_each_selected_topic() {
    let mut f = fixture();
    let dir = content_fixture(&mut f, ContentEncryption::None);
    f.cfg.topics[1].storage = TopicStorage::Interval {
        seconds: 3600,
        encrypted: false,
    };
    let mut engine = Engine::new(f.cfg, NOW).unwrap();
    let session = auth(&mut engine, &f.keys[0], NOW);
    topic_publish(&mut engine, session, "local/events", "manual", NOW);
    topic_publish(&mut engine, session, "other", "automatic", NOW);
    let blocked = dir.0.join("content/state.tmp");
    std::fs::create_dir(&blocked).unwrap();
    let failed = engine.save_content().unwrap();
    wait_content(&engine, |s| s.last_error.is_some() && !s.save_in_progress);
    let status = serde_json::to_value(engine.content_status()).unwrap();
    assert_eq!(
        status["topics"]["local/events"]["save_failure"]["revision"],
        failed
    );
    assert_eq!(
        status["topics"]["other"]["save_failure"]["revision"],
        failed
    );

    std::fs::remove_dir(blocked).unwrap();
    engine.content.as_mut().unwrap().force_due();
    engine.maintenance(NOW);
    wait_content(&engine, |s| {
        s.topics["other"].save_failure.is_none() && !s.save_in_progress
    });
    assert!(
        engine.content_status().topics["local/events"]
            .save_failure
            .is_some()
    );

    let retry = engine
        .save_content_topics(Some(&["local/events".into()]))
        .unwrap();
    wait_content(&engine, |s| {
        s.topics["local/events"].saved_revision == retry && !s.save_in_progress
    });
    assert!(
        engine.content_status().topics["local/events"]
            .save_failure
            .is_none()
    );
    engine.content.as_mut().unwrap().drain().unwrap();
}

#[cfg(unix)]
#[test]
fn failed_security_recovery_preserves_unreferenced_content_slots() {
    let mut f = fixture();
    let dir = content_fixture(&mut f, ContentEncryption::None);
    let mut engine = Engine::new(f.cfg.clone(), NOW).unwrap();
    let session = auth(&mut engine, &f.keys[0], NOW);
    topic_publish(&mut engine, session, "local/events", "kept", NOW);
    let revision = engine.save_content().unwrap();
    wait_content(&engine, |s| {
        s.saved_revision == revision && !s.save_in_progress
    });
    drop(engine);
    let orphan = dir
        .0
        .join("content")
        .join(format!("topic-{}-0.bin", "1".repeat(64)));
    std::fs::write(&orphan, b"retained for diagnosis").unwrap();
    let snapshot = dir.0.join("security/snapshot");
    let baseline = std::fs::read(&snapshot).unwrap();
    f.cfg.security_storage.as_mut().unwrap().minimum_sequence = Some(1);
    assert!(Engine::new(f.cfg, NOW).is_err());
    assert_eq!(std::fs::read(snapshot).unwrap(), baseline);
    assert_eq!(std::fs::read(orphan).unwrap(), b"retained for diagnosis");
}

#[test]
fn precharged_binary_request_consumes_one_token_and_rechecks_identity() {
    let mut f = fixture();
    f.cfg.limits.client_burst = 2;
    f.cfg.limits.client_requests_per_second = 1;
    let mut engine = Engine::new(f.cfg, NOW).unwrap();
    let session = auth(&mut engine, &f.keys[0], NOW);
    let frame = || {
        Some(pb::Frame {
            version: 1,
            body: Some(pb::Body::Publish(pb::Publish {
                id: "precharged".into(),
                topic: "local/events".into(),
                payload: Payload::bytes(b"once".to_vec(), None),
                expires: None,
            })),
        })
    };
    let mut response = bytes::BytesMut::new();
    engine.charge(session, NOW).unwrap();
    assert_eq!(
        engine.write_protobuf_response(session, frame, NOW, &mut response, None, true),
        "result"
    );
    engine.charge(session, NOW).unwrap();
    engine.disconnect(session);
    assert_eq!(
        engine.write_protobuf_response(
            session,
            || panic!("invalid session decoded"),
            NOW,
            &mut response,
            None,
            true
        ),
        "error"
    );
    assert_eq!(engine.storage_stats().active_records, 1);
}

#[cfg(unix)]
#[test]
fn rejected_combined_start_leaves_incomplete_security_tail_intact() {
    let mut f = fixture();
    let dir = content_fixture(&mut f, ContentEncryption::None);
    let mut engine = Engine::new(f.cfg.clone(), NOW).unwrap();
    let session = auth(&mut engine, &f.keys[0], NOW);
    topic_publish(&mut engine, session, "local/events", "durable", NOW);
    let revision = engine.save_content().unwrap();
    wait_content(&engine, |s| {
        s.saved_revision == revision && !s.save_in_progress
    });
    drop(engine);

    let mut incompatible = f.cfg.clone();
    incompatible.clients.remove(0);
    for topic in &mut incompatible.topics {
        topic
            .acl
            .as_mut()
            .unwrap()
            .retain(|rule| rule.client_id != f.ids[0]);
    }
    let wrong = dir.0.join("wrong-security");
    incompatible.security_storage.as_mut().unwrap().directory = wrong.clone();
    incompatible
        .security_storage
        .as_mut()
        .unwrap()
        .minimum_sequence = Some(0);
    crate::initialize_security(&incompatible).unwrap();
    let journal = wrong.join("journal");
    std::fs::write(&journal, b"M").unwrap();
    let snapshot = std::fs::read(wrong.join("snapshot")).unwrap();
    let manifest = std::fs::read(dir.0.join("content/state.bin")).unwrap();
    assert!(Engine::new(incompatible, NOW).is_err());
    assert_eq!(std::fs::read(journal).unwrap(), b"M");
    assert_eq!(std::fs::read(wrong.join("snapshot")).unwrap(), snapshot);
    assert_eq!(
        std::fs::read(dir.0.join("content/state.bin")).unwrap(),
        manifest
    );
    let matching_journal = dir.0.join("security/journal");
    assert!(std::fs::read(&matching_journal).unwrap().is_empty());
    std::fs::write(&matching_journal, b"M").unwrap();
    let mut recovered = Engine::new(f.cfg, NOW).unwrap();
    assert!(std::fs::read(matching_journal).unwrap().is_empty());
    let session = auth(&mut recovered, &f.keys[0], NOW);
    assert_eq!(
        topic_poll(&mut recovered, session, "local/events", NOW)["items"][0]["message"]["payload"],
        "durable"
    );
}

#[test]
fn admin_client_indexes_track_key_and_session_lifecycles() {
    use crate::admin::Command as A;

    let fixture = fixture();
    let client = &fixture.ids[0];
    let mut engine = Engine::new(fixture.cfg, NOW).unwrap();
    let session = auth(&mut engine, &fixture.keys[0], NOW);
    let other = auth(&mut engine, &fixture.keys[1], NOW);
    let get = |engine: &mut Engine, now| {
        let crate::admin::ResultBody::Data(data) = engine
            .admin(
                A::ClientGet {
                    client: client.clone(),
                },
                now,
            )
            .unwrap()
        else {
            panic!("expected admin data")
        };
        data
    };
    let before = get(&mut engine, NOW);
    assert_eq!(before["active_sessions"], 1);
    assert_eq!(before["session_ids"], json!([session]));
    assert_eq!(before["keys"].as_array().unwrap().len(), 1);
    assert_eq!(get(&mut engine, NOW + 86_400_000)["active_sessions"], 0);
    let token = before["keys"][0]["token_id"].as_str().unwrap();
    engine.revoke_key(token).unwrap();
    let after = get(&mut engine, NOW + 86_400_000);
    assert_eq!(after["active_sessions"], 0);
    assert_eq!(after["keys"], json!([]));
    assert!(!engine.sessions.contains_key(&session));
    assert!(engine.sessions.contains_key(&other));
}
