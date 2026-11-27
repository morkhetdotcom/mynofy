use super::*;
use crate::{
    config::{Client, PublicKey},
    security::{Change, Reservation, Status},
};
impl Engine {
    #[cfg(all(test, unix))]
    pub(crate) fn set_security_write_gate(&self, gate: std::sync::Arc<crate::security::WriteGate>) {
        self.security.as_ref().unwrap().set_write_gate(gate);
    }

    /// Service transports confirm this fence after releasing the Engine lock.
    /// Embedders invoking synchronous Core APIs own their acknowledgement policy.
    pub(crate) fn security_confirmation(&self) -> Option<crate::security::DurabilityFence> {
        self.security
            .as_ref()
            .filter(|_| {
                self.config
                    .security_storage
                    .as_ref()
                    .is_some_and(|storage| storage.sync)
            })
            .map(|journal| journal.durability_fence())
    }
    pub(crate) fn security_check(&self, change: &Change) -> Result<()> {
        if let Some(journal) = &self.security {
            journal.validate_change(change)?;
        }
        Ok(())
    }
    pub(crate) fn security_reserve(&self) -> Result<Option<Reservation>> {
        if self.shutting_down {
            return Err(ApiError::new("service_stopping", "Owner is shutting down."));
        }
        self.security
            .as_ref()
            .map(|journal| journal.reserve())
            .transpose()
    }
    pub(crate) fn security_submit(
        &mut self,
        permit: Option<Reservation>,
        change: Change,
    ) -> Result<()> {
        if let (Some(journal), Some(permit)) = (&mut self.security, permit) {
            journal.submit(permit, change)?;
        }
        Ok(())
    }
    pub(crate) fn security_status(&self) -> Status {
        self.security
            .as_ref()
            .map_or_else(Status::default, |journal| journal.status())
    }
    pub(crate) fn security_client(&self, name: &str) -> Result<Client> {
        let state = self.clients.get(name).ok_or_else(ApiError::internal)?;
        let mut keys = state
            .key_tokens
            .iter()
            .map(|token| &self.keys[token])
            .map(|key| {
                Ok(PublicKey {
                    algorithm: "ed25519".into(),
                    public_key: crypto::encode(&key.public),
                    valid_until: protocol::timestamp(key.until)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        keys.sort_by(|a, b| a.public_key.cmp(&b.public_key));
        Ok(Client {
            client_id: name.into(),
            display_name: state.display_name.clone(),
            disabled: state.disabled,
            keys,
        })
    }
}
