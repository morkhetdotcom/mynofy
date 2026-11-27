//! Runtime peer policy is security state; TLS material and routes remain startup configuration.
use super::*;
use crate::{
    config::{DUPLICATE_PEER_CERTIFICATE_IDENTITY, Peer},
    security::Change,
};
fn invalid() -> ApiError {
    ApiError::new("invalid_request", "Invalid peer policy.")
}
fn not_found() -> ApiError {
    ApiError::new("not_found", "Unknown peer.")
}
fn page<T>(
    values: impl IntoIterator<Item = T>,
    after: Option<T>,
    limit: Option<u16>,
) -> Result<(Vec<T>, Option<T>)>
where
    T: Clone + Ord,
{
    let limit = usize::from(limit.unwrap_or(20));
    if !(1..=100).contains(&limit) {
        return Err(invalid());
    }
    let mut values: Vec<_> = values
        .into_iter()
        .filter(|v| after.as_ref().is_none_or(|a| v > a))
        .collect();
    values.sort();
    let more = values.len() > limit;
    values.truncate(limit);
    let next = more.then(|| values.last().cloned()).flatten();
    Ok((values, next))
}
impl Engine {
    pub(super) fn admin_peers_list(
        &self,
        after: Option<String>,
        limit: Option<u16>,
    ) -> Result<Value> {
        let (ids, next) = page(
            self.config
                .accepted_peers
                .iter()
                .map(|p| p.service_id.clone()),
            after,
            limit,
        )?;
        let items: Vec<_> = ids
            .iter()
            .map(|id| {
                let p = self
                    .config
                    .accepted_peers
                    .iter()
                    .find(|p| &p.service_id == id)
                    .expect("selected peer");
                json!({"peer":p,"disabled":self.config.disabled_peers.contains(id)})
            })
            .collect();
        Ok(
            json!({"items":items,"next_after":next,"upstream":self.config.upstream.as_ref().map(|u|json!({"service_id":u.service_id,"disabled":self.config.disabled_peers.contains(&u.service_id)}))}),
        )
    }
    pub(super) fn admin_peer_certificates_list(
        &self,
        after: Option<String>,
        limit: Option<u16>,
    ) -> Result<Value> {
        let (items, next) = page(self.config.revoked_peer_certificates.clone(), after, limit)?;
        Ok(json!({"items":items,"next_after":next}))
    }
    fn commit_peer_policy(
        &mut self,
        next: Config,
        change: Change,
        peer: Option<&str>,
    ) -> Result<()> {
        next.validate().map_err(|detail| {
            if detail == DUPLICATE_PEER_CERTIFICATE_IDENTITY {
                ApiError::new("invalid_request", DUPLICATE_PEER_CERTIFICATE_IDENTITY)
            } else {
                invalid()
            }
        })?;
        let revision = self
            .link_changes
            .borrow()
            .checked_add(1)
            .ok_or_else(ApiError::exhausted)?;
        let permit = self.security_reserve()?;
        self.security_submit(permit, change)?;
        self.config.accepted_peers = next.accepted_peers;
        self.config.disabled_peers = next.disabled_peers;
        self.config.revoked_peer_certificates = next.revoked_peer_certificates;
        if let Some(peer) = peer {
            if self
                .config
                .accepted_peers
                .iter()
                .any(|p| p.service_id == peer)
                || self
                    .config
                    .upstream
                    .as_ref()
                    .is_some_and(|u| u.service_id == peer)
            {
                self.peer_revisions.insert(peer.into(), revision);
            } else {
                self.peer_revisions.remove(peer);
            }
        }
        // Every manager sees the change even when no receiver is currently subscribed.
        self.link_changes.send_replace(revision);
        Ok(())
    }
    pub(super) fn admin_peer_set(&mut self, peer: Peer) -> Result<Value> {
        let id = peer.service_id.clone();
        if self.config.accepted_peers.iter().any(|old| old == &peer) {
            return Ok(json!({"service_id":id,"updated":false}));
        }
        let mut next = self.config.clone();
        next.accepted_peers.retain(|p| p.service_id != id);
        next.accepted_peers.push(peer.clone());
        self.commit_peer_policy(
            next,
            Change::Peer {
                service_id: id.clone(),
                value: Some(peer),
            },
            Some(&id),
        )?;
        Ok(json!({"service_id":id,"updated":true}))
    }
    pub(super) fn admin_peer_delete(&mut self, id: String) -> Result<Value> {
        if !self
            .config
            .accepted_peers
            .iter()
            .any(|p| p.service_id == id)
        {
            return Err(not_found());
        }
        let mut next = self.config.clone();
        next.accepted_peers.retain(|p| p.service_id != id);
        self.commit_peer_policy(
            next,
            Change::Peer {
                service_id: id.clone(),
                value: None,
            },
            Some(&id),
        )?;
        Ok(json!({"deleted":id}))
    }
    pub(super) fn admin_peer_disable(&mut self, id: String, disabled: bool) -> Result<Value> {
        if !self
            .config
            .accepted_peers
            .iter()
            .any(|p| p.service_id == id)
            && !self
                .config
                .upstream
                .as_ref()
                .is_some_and(|u| u.service_id == id)
            && !self.config.disabled_peers.contains(&id)
        {
            return Err(not_found());
        }
        if self.config.disabled_peers.contains(&id) == disabled {
            return Ok(json!({"service_id":id,"disabled":disabled,"updated":false}));
        }
        let mut next = self.config.clone();
        next.disabled_peers.retain(|p| p != &id);
        if disabled {
            next.disabled_peers.push(id.clone());
        }
        self.commit_peer_policy(
            next,
            Change::PeerDisabled {
                service_id: id.clone(),
                disabled,
            },
            Some(&id),
        )?;
        Ok(json!({"service_id":id,"disabled":disabled,"updated":true}))
    }
    pub(super) fn admin_peer_certificate_revoke(&mut self, fingerprint: String) -> Result<Value> {
        if !crate::admin::valid_certificate_fingerprint(&fingerprint) {
            return Err(invalid());
        }
        if self.config.revoked_peer_certificates.contains(&fingerprint) {
            return Ok(json!({"fingerprint":fingerprint,"revoked":true,"updated":false}));
        }
        let mut next = self.config.clone();
        next.revoked_peer_certificates.push(fingerprint.clone());
        self.commit_peer_policy(
            next,
            Change::PeerCertificateRevoked {
                fingerprint: fingerprint.clone(),
            },
            None,
        )?;
        Ok(json!({"fingerprint":fingerprint,"revoked":true,"updated":true}))
    }
}
