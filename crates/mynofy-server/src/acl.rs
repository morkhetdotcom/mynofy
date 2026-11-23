//! Sparse numeric ACL. Configuration/wire names are resolved at the engine boundary.
use crate::config::Action;
use std::collections::HashMap;

#[derive(Clone, Copy, Default)]
struct Permissions(u8);
impl Permissions {
    fn bit(action: &Action) -> u8 {
        match action {
            Action::Publish => 1,
            Action::Subscribe => 2,
            Action::InspectOwn => 4,
        }
    }
    fn from_actions(actions: &[Action]) -> Self {
        Self(
            actions
                .iter()
                .fold(0, |mask, action| mask | Self::bit(action)),
        )
    }
    fn allows(self, action: &Action) -> bool {
        self.0 & Self::bit(action) != 0
    }
}
#[derive(Default)]
pub(crate) struct Acl {
    grants: HashMap<u32, Permissions>,
}
impl Acl {
    fn key(client: u16, topic: u16) -> u32 {
        (u32::from(client) << 16) | u32::from(topic)
    }
    pub(crate) fn remove_topic(&mut self, slot: u16, last: u16) {
        self.grants.retain(|key, _| *key as u16 != slot);
        if slot != last {
            let moved: Vec<_> = self
                .grants
                .iter()
                .filter(|(key, _)| **key as u16 == last)
                .map(|(key, value)| (*key, *value))
                .collect();
            for (key, value) in moved {
                self.grants.remove(&key);
                self.grants
                    .insert((key & 0xffff0000) | u32::from(slot), value);
            }
        }
    }
    pub(crate) fn entries(&self) -> impl Iterator<Item = (u16, u16, Vec<Action>)> + '_ {
        self.grants.keys().map(|key| {
            let client = (key >> 16) as u16;
            let topic = *key as u16;
            (client, topic, self.actions(client, topic))
        })
    }
    pub(crate) fn set(&mut self, client: u16, topic: u16, actions: &[Action]) {
        let key = Self::key(client, topic);
        let mask = Permissions::from_actions(actions);
        if mask.0 == 0 {
            self.grants.remove(&key);
        } else {
            self.grants.insert(key, mask);
        }
    }
    pub(crate) fn allows(&self, client: u16, topic: u16, action: Action) -> bool {
        self.grants
            .get(&Self::key(client, topic))
            .copied()
            .unwrap_or_default()
            .allows(&action)
    }
    pub(crate) fn actions(&self, client: u16, topic: u16) -> Vec<Action> {
        let permissions = self
            .grants
            .get(&Self::key(client, topic))
            .copied()
            .unwrap_or_default();
        [Action::Publish, Action::Subscribe, Action::InspectOwn]
            .into_iter()
            .filter(|action| permissions.allows(action))
            .collect()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_masks_replace_and_revoke_without_granting_missing_bits() {
        let mut acl = Acl::default();
        for mask in 0..8 {
            let actions: Vec<_> = [Action::Publish, Action::Subscribe, Action::InspectOwn]
                .into_iter()
                .filter(|a| mask & Permissions::bit(a) != 0)
                .collect();
            acl.set(7, 9, &actions);
            assert_eq!(acl.actions(7, 9), actions);
            for action in [Action::Publish, Action::Subscribe, Action::InspectOwn] {
                assert_eq!(
                    acl.allows(7, 9, action.clone()),
                    mask & Permissions::bit(&action) != 0
                );
                assert!(!acl.allows(9, 7, action));
            }
        }
        acl.set(7, 9, &[]);
        assert!(acl.grants.is_empty());
        acl.set(7, 9, &[Action::Publish, Action::Publish]);
        assert_eq!(acl.actions(7, 9), vec![Action::Publish]);
    }
    #[test]
    fn numeric_keys_keep_client_topic_and_high_bits_separate() {
        let mut acl = Acl::default();
        for (client, topic) in [
            (0, 0),
            (0, u16::MAX),
            (u16::MAX, 0),
            (u16::MAX, u16::MAX),
            (1, 0),
            (0, 1),
        ] {
            acl.set(client, topic, &[Action::Subscribe]);
        }
        assert_eq!(acl.grants.len(), 6);
        acl.set(u16::MAX, 0, &[]);
        assert!(!acl.allows(u16::MAX, 0, Action::Subscribe));
        assert!(acl.allows(0, u16::MAX, Action::Subscribe));
        assert!(!acl.allows(1, 1, Action::Subscribe));
    }
}
