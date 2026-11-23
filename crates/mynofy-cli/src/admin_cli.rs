use super::BoxError;
use mynofy_server::admin::{self, Command};
use std::{collections::BTreeMap, io::Read, path::Path};

pub async fn run(args: Vec<String>) -> Result<(), BoxError> {
    if args.is_empty() || args == ["help"] || args == ["--help"] {
        println!("{}", HELP);
        return Ok(());
    }
    let mut flags = BTreeMap::new();
    let mut positions = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg.starts_with("--") {
            let value = args.next().ok_or("Missing admin flag value")?;
            if flags.insert(arg, value).is_some() {
                return Err("Duplicate admin flag".into());
            }
        } else {
            positions.push(arg);
        }
    }
    let words: Vec<_> = positions.iter().map(String::as_str).collect();
    let required = |name: &str| {
        flags
            .get(name)
            .map(String::as_str)
            .ok_or_else(|| BoxError::from(format!("Missing {name}")))
    };
    let public = || read_public(Path::new(required("--public-key")?));
    let command = match words.as_slice() {
        ["peers", "list"] => Command::PeersList {
            after: flags.get("--after").cloned(),
            limit: flags.get("--limit").map(|v| v.parse()).transpose()?,
        },
        ["peers", "set", id] => Command::PeerSet {
            peer: mynofy_server::config::Peer {
                service_id: (*id).into(),
                certificate_identity: flags.get("--identity").cloned(),
                topics: required("--topics")?.split(',').map(String::from).collect(),
            },
        },
        ["peers", "delete", id] => Command::PeerDelete {
            service_id: (*id).into(),
        },
        ["peers", "disable", id] | ["peers", "enable", id] => Command::PeerDisable {
            service_id: (*id).into(),
            disabled: words[1] == "disable",
        },
        ["peers", "revoked-certificates"] => Command::PeerCertificatesList {
            after: flags.get("--after").cloned(),
            limit: flags.get("--limit").map(|v| v.parse()).transpose()?,
        },
        ["peers", "revoke-certificate"] => {
            let fingerprint = match (flags.get("--fingerprint"), flags.get("--certificate")) {
                (Some(value), None) => value.clone(),
                (None, Some(path)) => {
                    use tokio_rustls::rustls::pki_types::{CertificateDer, pem::PemObject};
                    let cert = CertificateDer::pem_file_iter(path)?
                        .next()
                        .ok_or("Missing leaf certificate")??;
                    admin::certificate_fingerprint(cert.as_ref())
                }
                _ => return Err("Supply exactly one of --certificate or --fingerprint".into()),
            };
            Command::PeerCertificateRevoke { fingerprint }
        }
        ["clients", "list"] => Command::ClientsList {
            after: flags.get("--after").map(|v| v.parse()).transpose()?,
            limit: flags.get("--limit").map(|v| v.parse()).transpose()?,
        },
        ["clients", "get", client] => Command::ClientGet {
            client: (*client).into(),
        },
        ["clients", "add"] => Command::ClientAdd {
            public_key: public()?,
            valid_until: flags.get("--valid-until").cloned(),
            display_name: flags.get("--name").cloned(),
        },
        ["clients", "disable", client] => Command::ClientDisable {
            client: (*client).into(),
            disabled: true,
        },
        ["clients", "enable", client] => Command::ClientDisable {
            client: (*client).into(),
            disabled: false,
        },
        ["keys", "add", client] => Command::KeyAdd {
            client: (*client).into(),
            public_key: public()?,
            valid_until: flags.get("--valid-until").cloned(),
        },
        ["keys", "revoke", token] => Command::KeyRevoke {
            token: (*token).into(),
        },
        ["acl", "get", client, topic] => Command::AclGet {
            client: (*client).into(),
            topic: (*topic).into(),
        },
        ["acl", "set", client, topic] => {
            let raw = required("--allow")?;
            let allow = if raw.is_empty() || raw == "none" {
                Vec::new()
            } else {
                raw.split(',')
                    .map(|action| match action {
                        "publish" => Ok(mynofy_server::config::Action::Publish),
                        "subscribe" => Ok(mynofy_server::config::Action::Subscribe),
                        "inspect_own" => Ok(mynofy_server::config::Action::InspectOwn),
                        _ => Err("Unknown ACL action"),
                    })
                    .collect::<Result<_, _>>()?
            };
            Command::AclSet {
                client: (*client).into(),
                topic: (*topic).into(),
                allow,
            }
        }
        ["topics", "list"] => Command::TopicsList {
            after: flags.get("--after").cloned(),
            limit: flags.get("--limit").map(|v| v.parse()).transpose()?,
        },
        ["topics", "get", topic] => Command::TopicGet {
            topic: (*topic).into(),
        },
        ["topics", "create", topic] | ["topics", "policy", topic] => {
            let policy = mynofy_server::config::Policy {
                default_ttl_seconds: required("--default-ttl")?.parse()?,
                max_ttl_seconds: required("--max-ttl")?.parse()?,
            };
            if words[1] == "create" {
                Command::TopicCreate {
                    topic: (*topic).into(),
                    policy,
                }
            } else {
                Command::TopicPolicy {
                    topic: (*topic).into(),
                    policy,
                }
            }
        }
        ["topics", "delete", topic] => Command::TopicDelete {
            topic: (*topic).into(),
        },
        ["topics", "storage", topic] => {
            let encrypted = match flags
                .get("--encrypted")
                .map(String::as_str)
                .unwrap_or("false")
            {
                "true" => true,
                "false" => false,
                _ => return Err("--encrypted must be true or false".into()),
            };
            let storage = match required("--mode")? {
                "manual" => {
                    if flags.contains_key("--seconds") {
                        return Err("Manual storage has no interval".into());
                    }
                    mynofy_server::config::TopicStorage::Manual { encrypted }
                }
                "interval" => mynofy_server::config::TopicStorage::Interval {
                    seconds: required("--seconds")?.parse()?,
                    encrypted,
                },
                _ => {
                    return Err(
                        "Online transition only supports RAM to manual/interval storage".into(),
                    );
                }
            };
            Command::TopicStorage {
                topic: (*topic).into(),
                storage,
            }
        }
        ["content", "save"] => Command::ContentSave {
            topics: flags
                .get("--topics")
                .map(|names| names.split(',').map(str::to_owned).collect()),
        },
        ["status"] => Command::Status {},
        ["export"] => Command::Export {},
        _ => return Err("Unknown admin command; use mynofy admin help".into()),
    };
    let allowed: &[&str] = match &command {
        Command::PeersList { .. } | Command::PeerCertificatesList { .. } => {
            &["--socket", "--after", "--limit"]
        }
        Command::PeerSet { .. } => &["--socket", "--topics", "--identity"],
        Command::PeerCertificateRevoke { .. } => &["--socket", "--certificate", "--fingerprint"],
        Command::TopicsList { .. } => &["--socket", "--after", "--limit"],
        Command::TopicCreate { .. } | Command::TopicPolicy { .. } => {
            &["--socket", "--default-ttl", "--max-ttl"]
        }
        Command::ClientsList { .. } => &["--socket", "--after", "--limit"],
        Command::ClientAdd { .. } => &["--socket", "--public-key", "--valid-until", "--name"],
        Command::KeyAdd { .. } => &["--socket", "--public-key", "--valid-until"],
        Command::AclSet { .. } => &["--socket", "--allow"],
        Command::TopicStorage { .. } => &["--socket", "--mode", "--seconds", "--encrypted"],
        Command::ContentSave { .. } => &["--socket", "--topics"],
        Command::Export {} => &["--socket", "--out"],
        _ => &["--socket"],
    };
    if flags.keys().any(|flag| !allowed.contains(&flag.as_str())) {
        return Err("Unknown admin command flag".into());
    }
    let export = matches!(&command, Command::Export {});
    let reply = admin::request(Path::new(required("--socket")?), command).await?;
    if reply["ok"] != true {
        return Err(format!(
            "{}: {}",
            reply["error"]["code"].as_str().unwrap_or("admin_error"),
            reply["error"]["detail"]
                .as_str()
                .unwrap_or("Admin request rejected")
        )
        .into());
    }
    let value = reply.get("result").ok_or("Missing admin result")?;
    let data = if export
        && flags
            .get("--out")
            .is_some_and(|path| mynofy_server::config::is_yaml(Path::new(path)))
    {
        let config: mynofy_server::config::Config = serde_json::from_value(value.clone())?;
        config.to_yaml()?.into_bytes()
    } else {
        let mut data = serde_json::to_vec_pretty(value)?;
        data.push(b'\n');
        data
    };
    if let Some(out) = flags.get("--out") {
        let out = out.clone();
        tokio::task::spawn_blocking(move || super::write_private_file(Path::new(&out), &data))
            .await??;
    } else {
        tokio::io::AsyncWriteExt::write_all(&mut tokio::io::stdout(), &data).await?;
    }
    Ok(())
}
fn read_public(path: &Path) -> Result<String, BoxError> {
    let mut raw = Vec::new();
    std::fs::File::open(path)?
        .take(4097)
        .read_to_end(&mut raw)?;
    if raw.len() > 4096 {
        return Err("Public-key file exceeds 4096 bytes".into());
    }
    let text = std::str::from_utf8(&raw)?.trim();
    if text.starts_with('{') {
        let value = mynofy_protocol::strict_json(text)?;
        if value
            .get("algorithm")
            .is_some_and(|algorithm| algorithm != "ed25519")
        {
            return Err("Only ed25519 public keys are supported".into());
        }
        Ok(value["public_key"]
            .as_str()
            .ok_or("Expected public_key in keygen JSON")?
            .into())
    } else {
        Ok(text.into())
    }
}
const HELP: &str = "mynofy admin peers list --socket PATH [--after ID] [--limit 1..100]\nmynofy admin peers set ID --topics TOPIC,TOPIC [--identity SAN] --socket PATH\nmynofy admin peers delete|disable|enable ID --socket PATH\nmynofy admin peers revoke-certificate --certificate LEAF.pem --socket PATH\nmynofy admin peers revoke-certificate --fingerprint sha256:BASE64 --socket PATH\nmynofy admin peers revoked-certificates --socket PATH [--after FINGERPRINT] [--limit 1..100]\nmynofy admin clients list --socket PATH [--after SLOT] [--limit 1..100]\nmynofy admin clients get CLIENT --socket PATH\nmynofy admin clients add --public-key FILE --socket PATH [--valid-until UTC] [--name NAME]\nmynofy admin clients disable|enable CLIENT --socket PATH\nmynofy admin keys add CLIENT --public-key FILE --socket PATH [--valid-until UTC]\nmynofy admin keys revoke TOKEN --socket PATH\nmynofy admin acl get CLIENT TOPIC --socket PATH\nmynofy admin acl set CLIENT TOPIC --allow publish,subscribe,inspect_own|none --socket PATH\nmynofy admin topics list --socket PATH [--after TOPIC] [--limit 1..100]\nmynofy admin topics get TOPIC --socket PATH\nmynofy admin topics create|policy TOPIC --default-ttl SECONDS --max-ttl SECONDS --socket PATH\nmynofy admin topics delete TOPIC --socket PATH\nmynofy admin topics storage TOPIC --mode manual|interval [--seconds N] [--encrypted true|false] --socket PATH\nmynofy admin content save [--topics TOPIC,TOPIC] --socket PATH\nmynofy admin status --socket PATH\nmynofy admin export --socket PATH [--out NEW_FILE]\nPublic-key files contain keygen JSON or a raw Base64 public key. Security mutations take effect in RAM and are journaled asynchronously. Export uses YAML for .yaml/.yml output files, otherwise JSON.";
