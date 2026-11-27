use super::*;
use crate::progress_file::{MAX_PROGRESS_BYTES, ProgressFile};
use client::progress::ClientProgress;
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::Arc};
use tokio::{sync::Mutex, time::Instant};

const MAX_TOPICS: usize = 1024;
const MAX_STATE: usize = 16 * 1024;

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    progress: ClientProgress,
    state: BTreeMap<String, Value>,
}

#[derive(Default)]
struct PollState {
    ready_at: Option<Instant>,
    pending: Option<Pending>,
}

struct Pending {
    receipt: String,
    cursor: String,
    has_more: bool,
}

pub(super) struct Checkpoint {
    file: ProgressFile,
    saved: Saved,
    current: ClientProgress,
    polls: BTreeMap<String, PollState>,
}

pub(super) type Shared = Arc<Mutex<Checkpoint>>;

impl Checkpoint {
    pub(super) fn open(path: &Path, owner: &str, client: &str) -> Result<Shared, BoxError> {
        let (file, saved): (_, Saved) = ProgressFile::open_state(path)?;
        if (saved.progress.service_id != owner || saved.progress.client_id != client)
            && (saved.progress != ClientProgress::default() || !saved.state.is_empty())
        {
            return Err("MCP checkpoint belongs to a different owner or client".into());
        }
        if saved.progress.cursors.len() > MAX_TOPICS
            || saved
                .progress
                .cursors
                .iter()
                .any(|(topic, cursor)| !valid_topic(topic) || !valid_cursor(cursor))
            || saved.state.len() > MAX_TOPICS
            || saved
                .state
                .iter()
                .any(|(topic, value)| !valid_topic(topic) || !value.is_object())
        {
            return Err("Invalid MCP checkpoint".into());
        }
        Ok(Arc::new(Mutex::new(Self {
            file,
            current: saved.progress.clone(),
            saved,
            polls: BTreeMap::new(),
        })))
    }

    pub(super) fn bind(&mut self, welcome: &protocol::pb::AuthWelcome) -> Result<(), BoxError> {
        if self.current.bind(welcome)? {
            for poll in self.polls.values_mut() {
                poll.pending = None;
            }
        }
        Ok(())
    }

    pub(super) fn status(&self, topic: &str, welcome: Option<&protocol::pb::AuthWelcome>) -> Value {
        let committed = &self.saved.progress;
        json!({
            "type":"checkpoint",
            "topic":topic,
            "cursor":self.current.cursors.get(topic),
            "state":self.saved.state.get(topic),
            "committed_cursor":committed.cursors.get(topic),
            "committed_owner":(!committed.service_id.is_empty()).then(|| json!({
                "owner_id":committed.service_id,"client_id":committed.client_id,
                "boot_id":committed.boot_id,"restart_counter":committed.restart_counter
            })),
            "owner_context": if welcome.is_some() {"cached"} else {"unchecked"},
            "observed_owner":welcome.map(|w| json!({
                "owner_id":w.service_id,"client_id":w.client_id,
                "boot_id":w.boot_id,"restart_counter":w.restart_counter
            })),
            "max_checkpoint_bytes":MAX_PROGRESS_BYTES
        })
    }

    pub(super) fn has_receipt(&self, topic: &str, receipt: &str) -> bool {
        self.polls
            .get(topic)
            .and_then(|poll| poll.pending.as_ref())
            .is_some_and(|page| page.receipt == receipt)
    }

    pub(super) fn begin(
        &mut self,
        topic: &str,
        interval: Duration,
    ) -> Result<Option<String>, Value> {
        if !self.polls.contains_key(topic) && self.polls.len() >= MAX_TOPICS {
            let now = Instant::now();
            self.polls
                .retain(|_, poll| poll.ready_at.is_some_and(|ready| ready > now));
            if self.polls.len() >= MAX_TOPICS {
                return Err(tool_result(json!({"code":"checkpoint_topic_limit"}), true));
            }
        }
        if !self.current.cursors.contains_key(topic) && self.current.cursors.len() >= MAX_TOPICS {
            return Err(tool_result(json!({"code":"checkpoint_topic_limit"}), true));
        }
        let poll = self.polls.entry(topic.into()).or_default();
        let now = Instant::now();
        if let Some(ready) = poll.ready_at
            && ready > now
        {
            let retry_after_ms = (ready - now).as_millis() as u64 + 1;
            return Err(tool_result(
                json!({"type":"poll_deferred","topic":topic,"retry_after_ms":retry_after_ms}),
                false,
            ));
        }
        poll.ready_at = Some(now + interval);
        Ok(self.current.cursors.get(topic).cloned())
    }

    pub(super) fn finish(&mut self, topic: &str, interval: Duration) {
        if let Some(poll) = self.polls.get_mut(topic) {
            poll.ready_at = Some(Instant::now() + interval);
        }
    }

    pub(super) fn page(&mut self, topic: &str, response: &Value) -> Result<String, BoxError> {
        let cursor = response["next_cursor"]
            .as_str()
            .filter(|cursor| valid_cursor(cursor))
            .ok_or("Invalid poll cursor")?;
        let token = protocol::crypto::random::<16>()?;
        let receipt = token
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let poll = self.polls.get_mut(topic).ok_or("Missing poll state")?;
        poll.pending = Some(Pending {
            receipt: receipt.clone(),
            cursor: cursor.into(),
            has_more: response["has_more"] == true,
        });
        Ok(receipt)
    }

    pub(super) fn commit(
        &mut self,
        topic: &str,
        receipt: &str,
        state: Option<&Value>,
        welcome: Option<&protocol::pb::AuthWelcome>,
    ) -> Value {
        let Some(poll) = self.polls.get_mut(topic) else {
            return stale_receipt();
        };
        let Some(page) = poll.pending.as_ref().filter(|page| page.receipt == receipt) else {
            return stale_receipt();
        };
        if state.is_some_and(|value| {
            serde_json::to_vec(value).map_or(true, |bytes| bytes.len() > MAX_STATE)
        }) {
            return tool_result(json!({"code":"checkpoint_state_too_large"}), true);
        }
        if !self.current.cursors.contains_key(topic) && self.current.cursors.len() >= MAX_TOPICS {
            return tool_result(json!({"code":"checkpoint_topic_limit"}), true);
        }
        if state.is_some_and(|state| !state.is_null())
            && !self.saved.state.contains_key(topic)
            && self.saved.state.len() >= MAX_TOPICS
        {
            return tool_result(json!({"code":"checkpoint_topic_limit"}), true);
        }
        let mut next = self.saved.clone();
        next.progress = self.current.clone();
        next.progress
            .cursors
            .insert(topic.into(), page.cursor.clone());
        if let Some(state) = state {
            if state.is_null() {
                next.state.remove(topic);
            } else {
                next.state.insert(topic.into(), state.clone());
            }
        }
        let Ok(bytes) = serde_json::to_vec(&next) else {
            return tool_result(json!({"code":"checkpoint_encoding_failed"}), true);
        };
        if bytes.len() > MAX_PROGRESS_BYTES {
            return tool_result(
                json!({
                    "code":"checkpoint_too_large","max_checkpoint_bytes":MAX_PROGRESS_BYTES,
                    "attempted_bytes":bytes.len(),"message":"Reduce the state or pass null to remove this topic's saved state, then retry the receipt."
                }),
                true,
            );
        }
        if self.file.save_encoded(&bytes).is_err() {
            return tool_result(
                json!({"code":"checkpoint_write_failed","message":"Checkpoint durability could not be confirmed; retry the same receipt or inspect the file after restarting."}),
                true,
            );
        }
        if page.has_more {
            poll.ready_at = None;
        }
        poll.pending = None;
        self.current = next.progress.clone();
        self.saved = next;
        tool_result(self.status(topic, welcome), false)
    }
}

fn valid_topic(topic: &str) -> bool {
    !topic.is_empty() && topic.len() <= 256
}

fn valid_cursor(cursor: &str) -> bool {
    cursor.len() == 8 && cursor.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

pub(super) fn stale_receipt() -> Value {
    tool_result(json!({"code":"stale_checkpoint_receipt"}), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Directory(std::path::PathBuf);

    impl Directory {
        fn new() -> Self {
            use std::os::unix::fs::DirBuilderExt;
            let id = u64::from_ne_bytes(protocol::crypto::random().unwrap());
            let path = std::env::temp_dir().join(format!("mcp-checkpoint-{id:016x}"));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&path)
                .unwrap();
            Self(path)
        }
    }

    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn expired_poll_slots_can_be_reused_without_reclaiming_active_cooldowns() {
        let dir = Directory::new();
        let shared = Checkpoint::open(&dir.0.join("state"), "owner", "client").unwrap();
        let mut checkpoint = shared.try_lock().unwrap();
        let now = Instant::now();
        let active = now + Duration::from_secs(60);
        for index in 0..MAX_TOPICS {
            checkpoint.polls.insert(
                index.to_string(),
                PollState {
                    ready_at: Some(active),
                    pending: None,
                },
            );
        }
        let interval = Duration::from_secs(60);
        let reject = checkpoint.begin("new", interval).unwrap_err();
        assert_eq!(
            reject["structuredContent"]["code"],
            "checkpoint_topic_limit"
        );
        checkpoint.polls.get_mut("0").unwrap().ready_at = Some(now);
        let receipt = checkpoint
            .page("0", &json!({"next_cursor":"00000000"}))
            .unwrap();
        assert!(checkpoint.has_receipt("0", &receipt));
        assert_eq!(checkpoint.begin("new", interval).unwrap(), None);
        assert_eq!(checkpoint.polls.len(), MAX_TOPICS);
        assert!(!checkpoint.has_receipt("0", &receipt));
        assert_eq!(
            checkpoint.begin("1", interval).unwrap_err()["structuredContent"]["type"],
            "poll_deferred"
        );
    }

    #[test]
    fn file_budget_rejects_a_valid_state_and_allows_a_smaller_retry() {
        let dir = Directory::new();
        let path = dir.0.join("state");
        let shared = Checkpoint::open(&path, "owner", "client").unwrap();
        let mut checkpoint = shared.try_lock().unwrap();
        let welcome = protocol::pb::AuthWelcome {
            service_id: "owner".into(),
            client_id: "client".into(),
            restart_counter: Some(1),
            ..Default::default()
        };
        checkpoint.bind(&welcome).unwrap();
        checkpoint.saved.progress = checkpoint.current.clone();
        let state = json!({"data":"x".repeat(MAX_STATE - 20)});
        for index in 0..255 {
            checkpoint
                .saved
                .state
                .insert(index.to_string(), state.clone());
        }
        checkpoint
            .saved
            .state
            .insert("other".into(), json!({"data":"y".repeat(1024)}));
        checkpoint.file.save(&checkpoint.saved).unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(serde_json::to_vec(&state).unwrap().len() < MAX_STATE);

        checkpoint.begin("new", Duration::from_secs(60)).unwrap();
        let receipt = checkpoint
            .page("new", &json!({"next_cursor":"00000001"}))
            .unwrap();
        let failed = checkpoint.commit("new", &receipt, Some(&state), Some(&welcome));
        let details = &failed["structuredContent"];
        assert_eq!(details["code"], "checkpoint_too_large");
        assert!(details["attempted_bytes"].as_u64().unwrap() > MAX_PROGRESS_BYTES as u64);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(checkpoint.has_receipt("new", &receipt));

        let small = json!({"ok":true});
        let committed = checkpoint.commit("new", &receipt, Some(&small), Some(&welcome));
        assert_eq!(
            committed["structuredContent"]["committed_cursor"],
            "00000001"
        );
        assert_eq!(committed["structuredContent"]["state"], small);
        assert!(!checkpoint.has_receipt("new", &receipt));

        checkpoint.begin("other", Duration::from_secs(60)).unwrap();
        let receipt = checkpoint
            .page("other", &json!({"next_cursor":"00000002"}))
            .unwrap();
        let removed = checkpoint.commit("other", &receipt, Some(&Value::Null), Some(&welcome));
        assert_eq!(removed["structuredContent"]["committed_cursor"], "00000002");
        assert!(removed["structuredContent"]["state"].is_null());
        assert!(!checkpoint.saved.state.contains_key("other"));
        drop(checkpoint);
        drop(shared);
        assert!(Checkpoint::open(&path, "another-owner", "client").is_err());
        assert!(Checkpoint::open(&path, "owner", "another-client").is_err());
        assert!(Checkpoint::open(&path, "owner", "client").is_ok());
    }
}
