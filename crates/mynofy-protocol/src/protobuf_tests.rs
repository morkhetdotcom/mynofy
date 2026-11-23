use crate::{ProtobufMessage as _, payload::*, pb::*};
use serde_json::json;

#[test]
fn byte_payload_is_exact_and_has_a_hard_limit() {
    for n in [0, 1, 4096] {
        let p = Payload::bytes(vec![0xff; n], Some("application/octet-stream".into()));
        p.validate().unwrap();
        assert_eq!(Payload::decode(p.encode_to_vec().as_slice()).unwrap(), p);
        assert_eq!(Payload::from_json(p.to_json().unwrap()).unwrap(), p);
    }
    assert!(Payload::bytes(vec![0; 4097], None).validate().is_err());
    assert!(Payload::from_json(json!({"kind":"bytes","data":"!!!!"})).is_err());
    assert!(Payload::from_json(json!({"kind":"bytes","data":"AA==","extra":1})).is_err());
}
#[test]
fn typed_parameters_preserve_bits_and_validate_catalog() {
    let parameters = vec![
        Parameter {
            value: Some(ParameterValue::U64(u64::MAX)),
        },
        Parameter {
            value: Some(ParameterValue::I64(i64::MIN)),
        },
        Parameter {
            value: Some(ParameterValue::Bool(false)),
        },
    ];
    let mut e = Event {
        catalog: 1,
        catalog_version: 1,
        category: 12,
        action: 10,
        status: 7,
        parameters,
    };
    let p = Payload::event(e.clone());
    p.validate().unwrap();
    assert_eq!(p.value_len(), 17);
    assert_eq!(Payload::decode(p.encode_to_vec().as_slice()).unwrap(), p);
    assert_eq!(Payload::from_json(p.to_json().unwrap()).unwrap(), p);
    e.category = 200;
    assert!(Payload::event(e.clone()).validate().is_err());
    e.category = 12;
    e.catalog_version = 2;
    assert!(Payload::event(e.clone()).validate().is_err());
    e.catalog_version = 1;
    e.parameters.push(Parameter { value: None });
    assert!(Payload::event(e.clone()).validate().is_err());
    e.parameters = vec![Parameter {
        value: Some(ParameterValue::F64(f64::NAN)),
    }];
    assert!(Payload::event(e).validate().is_err());
}
#[test]
fn protocol_examples_roundtrip_with_all_control_frames() {
    let files = [
        include_str!("../../../protocol/v1/examples.json"),
        include_str!("../../../protocol/v1/poll-examples.json"),
        include_str!("../../../protocol/v1/status-examples.json"),
        include_str!("../../../protocol/v1/permission-examples.json"),
        include_str!("../../../protocol/v1/client-auth-examples.json"),
        include_str!("../../../protocol/v1/catalog-examples.json"),
        include_str!("../../../protocol/v1/security-storage-examples.json"),
    ];
    fn visit(value: &serde_json::Value, count: &mut usize) {
        if value.get("version") == Some(&json!(1)) && value.get("type").is_some() {
            let frame = Frame::from_json(value.clone()).unwrap();
            let decoded = Frame::decode(frame.encode_to_vec().as_slice()).unwrap();
            assert_eq!(decoded.to_json().unwrap(), *value);
            *count += 1;
        } else {
            match value {
                serde_json::Value::Array(v) => {
                    for v in v {
                        visit(v, count)
                    }
                }
                serde_json::Value::Object(m) => {
                    for v in m.values() {
                        visit(v, count)
                    }
                }
                _ => {}
            }
        }
    }
    let mut count = 0;
    for file in files {
        visit(&serde_json::from_str(file).unwrap(), &mut count);
    }
    assert!(count >= 30, "Only {count} frames tested");
}

#[test]
fn generated_catalog_membership_matches_published_catalog() {
    let c = crate::catalog::builtin_catalog().unwrap();
    assert_eq!(c["catalog_id"], 1);
    for id in 0..4096 {
        for (group, valid) in [
            ("categories", crate::catalog_values::valid_category(id)),
            ("actions", crate::catalog_values::valid_action(id)),
            ("statuses", crate::catalog_values::valid_status(id)),
        ] {
            assert_eq!(
                valid,
                c[group].as_array().unwrap().iter().any(|e| e["id"] == id)
            );
        }
    }
}
