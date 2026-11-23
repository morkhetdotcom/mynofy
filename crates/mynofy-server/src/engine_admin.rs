//! Administrative snapshots and atomic changes. Transport authorizes the local operator.
use super::*;
use crate::{
    admin::{Command as AdminCommand, ResultBody},
    config::{Client, PublicKey, Rule},
};
impl Engine {
    pub fn admin(&mut self, command: AdminCommand, now: u64) -> Result<ResultBody> {
        self.sweep(now);
        let now = now.max(self.last_cleanup_now);
        let changed_client = match &command {
            AdminCommand::ClientAdd { public_key, .. } => Some(crypto::initial_client_id(
                &crypto::decode::<32>(public_key).map_err(|_| admin_invalid())?,
            )),
            AdminCommand::ClientDisable { client, .. } | AdminCommand::KeyAdd { client, .. } => {
                Some(client.clone())
            }
            _ => None,
        };
        let permit = if changed_client.is_some() {
            self.security_reserve()?
        } else {
            None
        };
        if self.security.is_some() {
            let candidate = match &command {
                AdminCommand::ClientAdd {
                    public_key,
                    valid_until,
                    display_name,
                } => {
                    let (public, until, _) =
                        self.admin_key(public_key, valid_until.clone(), now)?;
                    Some(Client {
                        client_id: crypto::initial_client_id(&public),
                        display_name: display_name.clone(),
                        disabled: false,
                        keys: vec![PublicKey {
                            algorithm: "ed25519".into(),
                            public_key: crypto::encode(&public),
                            valid_until: protocol::timestamp(until)?,
                        }],
                    })
                }
                AdminCommand::ClientDisable { client, disabled } => {
                    if !self.clients.contains_key(client) {
                        return Err(admin_not_found());
                    }
                    let mut value = self.security_client(client)?;
                    value.disabled = *disabled;
                    Some(value)
                }
                AdminCommand::KeyAdd {
                    client,
                    public_key,
                    valid_until,
                } => {
                    if !self.clients.contains_key(client) {
                        return Err(admin_not_found());
                    }
                    let (public, until, _) =
                        self.admin_key(public_key, valid_until.clone(), now)?;
                    let mut value = self.security_client(client)?;
                    value.keys.push(PublicKey {
                        algorithm: "ed25519".into(),
                        public_key: crypto::encode(&public),
                        valid_until: protocol::timestamp(until)?,
                    });
                    Some(value)
                }
                _ => None,
            };
            if let Some(value) = candidate {
                self.security_check(&crate::security::Change::Client { value })?;
            }
        }
        let value = match command {
            AdminCommand::PeersList { after, limit } => self.admin_peers_list(after, limit)?,
            AdminCommand::PeerSet { peer } => self.admin_peer_set(peer)?,
            AdminCommand::PeerDelete { service_id } => self.admin_peer_delete(service_id)?,
            AdminCommand::PeerDisable {
                service_id,
                disabled,
            } => self.admin_peer_disable(service_id, disabled)?,
            AdminCommand::PeerCertificateRevoke { fingerprint } => {
                self.admin_peer_certificate_revoke(fingerprint)?
            }
            AdminCommand::PeerCertificatesList { after, limit } => {
                self.admin_peer_certificates_list(after, limit)?
            }
            AdminCommand::ClientsList { after, limit } => {
                let limit = usize::from(limit.unwrap_or(20));
                if !(1..=100).contains(&limit) {
                    return Err(admin_invalid());
                }
                let start = after.map_or(0, |slot| usize::from(slot) + 1);
                if start > self.client_names.len() {
                    return Err(admin_invalid());
                }
                let selected =
                    &self.client_names[start..self.client_names.len().min(start + limit)];
                let mut key_counts: HashMap<&str, usize> =
                    selected.iter().map(|name| (name.as_str(), 0)).collect();
                let mut session_counts = key_counts.clone();
                for key in self.keys.values() {
                    if let Some(count) = key_counts.get_mut(key.client.as_str()) {
                        *count += 1;
                    }
                }
                for session in self.sessions.keys() {
                    if let Ok((client, _, _)) = self.identity_ref(*session, now)
                        && let Some(count) = session_counts.get_mut(client)
                    {
                        *count += 1;
                    }
                }
                let items:Vec<_>=selected.iter().map(|name|{let state=&self.clients[name];json!({"client_id":name,"display_name":state.display_name,"disabled":state.disabled,"acl_revision":state.revision,"registered_keys":key_counts[name.as_str()],"active_sessions":session_counts[name.as_str()]})}).collect();
                let next = if start + selected.len() < self.client_names.len() {
                    Some((start + selected.len() - 1) as u16)
                } else {
                    None
                };
                json!({"items":items,"next_after":next})
            }
            AdminCommand::ClientGet { client } => self.admin_client(&client, now)?,
            AdminCommand::ClientAdd {
                public_key,
                valid_until,
                display_name,
            } => {
                if self.clients.len() >= 10000 {
                    return Err(ApiError::exhausted());
                }
                if display_name.as_ref().is_some_and(|name| name.len() > 256) {
                    return Err(admin_invalid());
                }
                let (public, until, token) = self.admin_key(&public_key, valid_until, now)?;
                let client = crypto::initial_client_id(&public);
                if self.clients.contains_key(&client) {
                    return Err(ApiError::new("conflict", "Client identity already exists."));
                }
                let slot = self.client_names.len() as u16;
                self.clients.insert(
                    client.clone(),
                    ClientState {
                        slot,
                        disabled: false,
                        display_name,
                        request_slots: std::sync::Arc::new(tokio::sync::Semaphore::new(
                            self.config.limits.max_parallel_requests_per_client,
                        )),
                        produced: 0,
                        revision: 0,
                        bucket: Bucket::new(self.config.limits.client_burst, now),
                        rotation_attempts: Vec::new(),
                    },
                );
                self.client_names.push(client.clone());
                self.own.insert(client.clone(), OrderedIndex::new());
                self.keys.insert(
                    token,
                    Credential {
                        client: client.clone(),
                        public,
                        until,
                    },
                );
                self.admin_client(&client, now)?
            }
            AdminCommand::ClientDisable { client, disabled } => {
                let state = self.clients.get_mut(&client).ok_or_else(admin_not_found)?;
                if state.disabled != disabled {
                    let revision = state
                        .revision
                        .checked_add(1)
                        .ok_or_else(ApiError::exhausted)?;
                    state.revision = revision;
                    state.disabled = disabled;
                    let sessions: Vec<_> = self
                        .sessions
                        .iter()
                        .filter_map(|(id, s)| {
                            s.auth.as_ref().filter(|a| a.client == client).map(|_| *id)
                        })
                        .collect();
                    for session in sessions {
                        self.disconnect(session);
                    }
                    self.take_rotation(&client);
                }
                self.admin_client(&client, now)?
            }
            AdminCommand::KeyAdd {
                client,
                public_key,
                valid_until,
            } => {
                if !self.clients.contains_key(&client) {
                    return Err(admin_not_found());
                }
                if self
                    .keys
                    .values()
                    .filter(|key| key.client == client)
                    .count()
                    >= 8
                {
                    return Err(ApiError::exhausted());
                }
                let (public, until, token) = self.admin_key(&public_key, valid_until, now)?;
                self.keys.insert(
                    token,
                    Credential {
                        client: client.clone(),
                        public,
                        until,
                    },
                );
                self.admin_client(&client, now)?
            }
            AdminCommand::KeyRevoke { token } => {
                if !self.keys.contains_key(&token) {
                    return Err(admin_not_found());
                }
                self.revoke_key(&token)?;
                json!({"revoked":token})
            }
            AdminCommand::AclGet { client, topic } => {
                let client = self.clients.get(&client).ok_or_else(admin_not_found)?;
                let topic = self.topics.get(&topic).ok_or_else(admin_not_found)?;
                json!({"allow":self.acl.actions(client.slot,topic.slot)})
            }
            AdminCommand::AclSet {
                client,
                topic,
                allow,
            } => {
                if allow.len() > 3
                    || allow.iter().collect::<std::collections::HashSet<_>>().len() != allow.len()
                {
                    return Err(admin_invalid());
                }
                if !self.clients.contains_key(&client) || !self.topics.contains_key(&topic) {
                    return Err(admin_not_found());
                }
                self.set_acl(&client, &topic, allow)?;
                json!({"client_id":client,"topic":topic,"updated":true})
            }
            AdminCommand::TopicsList { after, limit } => {
                self.admin_topics_list(after, limit, now)?
            }
            AdminCommand::TopicGet { topic } => self.admin_topic(&topic, now)?,
            AdminCommand::TopicCreate { topic, policy } => {
                self.admin_topic_create(topic, policy, now)?
            }
            AdminCommand::TopicPolicy { topic, policy } => {
                self.admin_topic_policy(topic, policy, now)?
            }
            AdminCommand::TopicDelete { topic } => self.admin_topic_delete(topic)?,
            AdminCommand::TopicStorage { topic, storage } => {
                self.enable_topic_storage(&topic, storage)?;
                json!({"topic":topic,"updated":true,"durable":false})
            }
            AdminCommand::ContentSave { topics } => {
                let revision = self.save_content_topics(topics.as_deref())?;
                json!({"queued":true,"revision":revision})
            }
            AdminCommand::Status {} => {
                json!({"service_id":self.config.service.id,"restart_counter":self.restart_counter,"clients":self.clients.len(),"registered_keys":self.keys.len(),"sessions":self.sessions.keys().filter(|id|self.authenticated(**id,now)).count(),"topics":self.topics.len(),"storage":self.storage_stats(),"message_bytes_charged":self.active_bytes,"cleanup_pending":self.cleanup_record.is_some()||self.cleanup_batch.is_some()||self.expiry.has_due(now)||self.records.needs_reclaim()||self.rotation_expiry.first().is_some_and(|entry|entry.0<=now),"limits":self.config.limits,"security_storage":self.security_status(),"content_storage":self.content_status(),"service_links":self.link_status.lock().map_err(|_|ApiError::internal())?.clone()})
            }
            AdminCommand::Export {} => {
                return Ok(ResultBody::Configuration(Box::new(self.admin_export()?)));
            }
        };
        if let Some(client) = changed_client {
            let value = self.security_client(&client)?;
            self.security_submit(permit, crate::security::Change::Client { value })?;
        }
        Ok(ResultBody::Data(value))
    }
    fn admin_key(
        &self,
        public: &str,
        valid_until: Option<String>,
        now: u64,
    ) -> Result<([u8; 32], u64, String)> {
        let public = crypto::decode::<32>(public).map_err(|_| admin_invalid())?;
        let token = crypto::token_id(&public);
        if self.keys.contains_key(&token) {
            return Err(ApiError::new(
                "conflict",
                "Public key is already registered.",
            ));
        }
        let until = match valid_until {
            Some(time) => protocol::parse_time(&time).map_err(|_| admin_invalid())?,
            None => now
                .checked_add(self.config.rotation.key_validity_seconds * 1000)
                .ok_or_else(admin_invalid)?,
        };
        if until <= now {
            return Err(admin_invalid());
        }
        protocol::timestamp(until).map_err(|_| admin_invalid())?;
        Ok((public, until, token))
    }
    fn admin_client(&self, client: &str, now: u64) -> Result<Value> {
        let state = self.clients.get(client).ok_or_else(admin_not_found)?;
        let mut keys:Vec<_>=self.keys.iter().filter(|(_,key)|key.client==client).map(|(token,key)|Ok(json!({"token_id":token,"algorithm":"ed25519","public_key":crypto::encode(&key.public),"valid_until":protocol::timestamp(key.until)?,"expired":key.until<=now}))).collect::<Result<_>>()?;
        keys.sort_by(|a, b| a["token_id"].as_str().cmp(&b["token_id"].as_str()));
        let mut sessions = Vec::new();
        let mut count = 0;
        for id in self.sessions.keys() {
            if self
                .identity_ref(*id, now)
                .is_ok_and(|(name, _, _)| name == client)
            {
                count += 1;
                if sessions.len() < 100 {
                    sessions.push(*id);
                }
            }
        }
        sessions.sort_unstable();
        Ok(
            json!({"client_id":client,"display_name":state.display_name,"disabled":state.disabled,"acl_revision":state.revision,"keys":keys,"active_sessions":count,"session_ids":sessions,"sessions_truncated":count>100}),
        )
    }
    fn admin_export(&self) -> Result<Config> {
        let mut config = self.config.clone();
        config.clients = self
            .client_names
            .iter()
            .map(|name| {
                let state = &self.clients[name];
                Client {
                    client_id: name.clone(),
                    display_name: state.display_name.clone(),
                    disabled: state.disabled,
                    keys: Vec::new(),
                }
            })
            .collect();
        for key in self.keys.values() {
            let slot = usize::from(self.clients[&key.client].slot);
            config.clients[slot].keys.push(PublicKey {
                algorithm: "ed25519".into(),
                public_key: crypto::encode(&key.public),
                valid_until: protocol::timestamp(key.until)?,
            });
        }
        for client in &mut config.clients {
            client.keys.sort_by(|a, b| a.public_key.cmp(&b.public_key));
        }
        let mut indexes = vec![0; self.topic_names.len()];
        for (index, topic) in config.topics.iter_mut().enumerate() {
            if let Some(local) = self.topics.get(&topic.name) {
                indexes[usize::from(local.slot)] = index;
                topic.acl = Some(Vec::new());
            }
        }
        for (client, topic, allow) in self.acl.entries() {
            let topic = &mut config.topics[indexes[usize::from(topic)]];
            if let Some(rules) = &mut topic.acl {
                rules.push(Rule {
                    client_id: self.client_names[usize::from(client)].clone(),
                    allow,
                });
            }
        }
        for topic in &mut config.topics {
            if let Some(rules) = &mut topic.acl {
                rules.sort_by(|a, b| a.client_id.cmp(&b.client_id));
            }
        }
        Ok(config)
    }
}
fn admin_not_found() -> ApiError {
    ApiError::new("not_found", "Client, key or local topic not found.")
}

fn admin_invalid() -> ApiError {
    ApiError::new(
        "invalid_argument",
        "Invalid public key, future UTC expiry, name, actions or pagination bounds.",
    )
}
