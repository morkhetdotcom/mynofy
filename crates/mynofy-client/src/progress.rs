//! Caller-owned restart/cursor state; no global registry and no automatic command retries.
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientProgress {
    pub service_id: String,
    pub client_id: String,
    pub restart_counter: Option<u32>,
    pub cursors: BTreeMap<String, String>,
    #[serde(default)]
    pub reset_missing_cursors: bool,
}
pub struct RestartClient<S> {
    socket: WebSocketStream<S>,
    progress: ClientProgress,
    changed: bool,
}
impl<S: AsyncRead + AsyncWrite + Unpin> RestartClient<S> {
    /// Bind a previously saved state to a successfully authenticated connection.
    pub fn new(
        socket: WebSocketStream<S>,
        welcome: protocol::pb::AuthWelcome,
        mut progress: ClientProgress,
    ) -> Result<Self, BoxError> {
        let counter = welcome
            .restart_counter
            .ok_or("Owner did not provide a restart counter")?;
        if (!progress.service_id.is_empty() && progress.service_id != welcome.service_id)
            || (!progress.client_id.is_empty() && progress.client_id != welcome.client_id)
            || progress.cursors.len() > 10_000
            || progress.cursors.iter().any(|(topic, cursor)| {
                topic.is_empty()
                    || topic.len() > 256
                    || cursor.len() != 8
                    || !cursor.bytes().all(|c| c.is_ascii_alphanumeric())
            })
        {
            return Err(
                "Client progress does not match the authenticated owner/client or is invalid"
                    .into(),
            );
        }
        // Zero denotes an owner without a durable restart epoch. Replay conservatively.
        let changed = counter == 0
            || progress.restart_counter.is_some_and(|old| old != counter)
            || (progress.restart_counter.is_none() && !progress.cursors.is_empty());
        if changed {
            progress.cursors.clear();
            progress.reset_missing_cursors = true;
        }
        progress.service_id = welcome.service_id;
        progress.client_id = welcome.client_id;
        progress.restart_counter = Some(counter);
        Ok(Self {
            socket,
            progress,
            changed,
        })
    }
    /// Read position, not a processing checkpoint. Persist only after completing work,
    /// or after durably recording pending work that recovery will reconcile.
    pub fn progress(&self) -> &ClientProgress {
        &self.progress
    }
    /// Whether continuation was reset. Zero epochs are conservatively treated as
    /// unknown, so true need not mean a restart actually happened.
    pub fn restart_detected(&self) -> bool {
        self.changed
    }
    pub fn into_parts(self) -> (WebSocketStream<S>, ClientProgress) {
        (self.socket, self.progress)
    }
    pub async fn poll(
        &mut self,
        topic: &str,
        limit: Option<u32>,
    ) -> Result<protocol::pb::Frame, BoxError> {
        self.exchange_frame(&protocol::pb::Frame {
            version: 1,
            body: Some(protocol::pb::Body::Poll(protocol::pb::Poll {
                id: "poll".into(),
                topic: topic.into(),
                after: self.progress.cursors.get(topic).cloned(),
                limit,
            })),
        })
        .await
    }
    pub async fn exchange(&mut self, request: &Value) -> Result<Value, BoxError> {
        self.exchange_frame(&protocol::pb::Frame::from_json(request.clone())?)
            .await?
            .to_json()
            .map_err(Into::into)
    }
    pub async fn exchange_frame(
        &mut self,
        request: &protocol::pb::Frame,
    ) -> Result<protocol::pb::Frame, BoxError> {
        let mut request = request.clone();
        let topic = if let Some(protocol::pb::Body::Poll(poll)) = &mut request.body {
            if poll.topic.is_empty() || poll.topic.len() > 256 {
                return Err("Invalid topic".into());
            }
            if !self.progress.cursors.contains_key(&poll.topic)
                && self.progress.cursors.len() >= 10_000
            {
                return Err("Client progress topic limit reached".into());
            }
            if self.progress.reset_missing_cursors
                && !self.progress.cursors.contains_key(&poll.topic)
            {
                poll.after = None;
            }
            Some((poll.topic.clone(), poll.id.clone()))
        } else {
            None
        };
        let response = super::exchange_frame(&mut self.socket, &request).await?;
        if let (Some((topic, id)), Some(protocol::pb::Body::Messages(page))) =
            (topic, &response.body)
        {
            if page.request_id != id
                || page.next_cursor.len() != 8
                || !page.next_cursor.bytes().all(|c| c.is_ascii_alphanumeric())
            {
                return Err(
                    "Poll response does not match request or contains an invalid cursor".into(),
                );
            }
            self.progress
                .cursors
                .insert(topic.clone(), page.next_cursor.clone());
        }
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{ProtobufMessage, pb};
    use tokio_tungstenite::tungstenite::protocol::Role;
    async fn sockets() -> (
        WebSocketStream<tokio::io::DuplexStream>,
        WebSocketStream<tokio::io::DuplexStream>,
    ) {
        let (a, b) = tokio::io::duplex(8192);
        (
            WebSocketStream::from_raw_socket(a, Role::Client, None).await,
            WebSocketStream::from_raw_socket(b, Role::Server, None).await,
        )
    }
    fn welcome(counter: u32) -> pb::AuthWelcome {
        pb::AuthWelcome {
            request_id: "auth".into(),
            service_id: "owner".into(),
            client_id: "client".into(),
            restart_counter: Some(counter),
        }
    }
    #[tokio::test]
    async fn zero_epoch_always_replays_but_equal_nonzero_epoch_retains_progress() {
        for counter in [0, 7] {
            let (socket, _) = sockets().await;
            let progress = ClientProgress {
                service_id: "owner".into(),
                client_id: "client".into(),
                restart_counter: Some(counter),
                cursors: [("a".into(), "00000001".into())].into_iter().collect(),
                reset_missing_cursors: false,
            };
            let client = RestartClient::new(socket, welcome(counter), progress).unwrap();
            assert_eq!(client.restart_detected(), counter == 0);
            assert_eq!(client.progress().cursors.is_empty(), counter == 0);
        }
    }
    #[tokio::test]
    async fn counter_change_and_wrap_replay_every_topic_once_then_retain_new_progress() {
        let (socket, mut peer) = sockets().await;
        let progress = ClientProgress {
            service_id: "owner".into(),
            client_id: "client".into(),
            restart_counter: Some(u32::MAX),
            reset_missing_cursors: false,
            cursors: [
                ("a".into(), "00000009".into()),
                ("b".into(), "00000009".into()),
            ]
            .into_iter()
            .collect(),
        };
        let mut client = RestartClient::new(socket, welcome(0), progress).unwrap();
        assert!(client.restart_detected());
        assert!(client.progress().cursors.is_empty());
        let task = tokio::spawn(async move {
            for expected in [None, None, Some("00000001")] {
                let Message::Binary(bytes) = peer.next().await.unwrap().unwrap() else {
                    panic!("binary expected")
                };
                let frame = pb::Frame::decode(bytes).unwrap();
                let Some(pb::Body::Poll(poll)) = frame.body else {
                    panic!("poll expected")
                };
                assert_eq!(poll.after.as_deref(), expected);
                peer.send(Message::Binary(
                    pb::Frame {
                        version: 1,
                        body: Some(pb::Body::Messages(pb::Messages {
                            request_id: poll.id,
                            items: vec![],
                            next_cursor: "00000001".into(),
                            has_more: false,
                        })),
                    }
                    .encode_to_vec()
                    .into(),
                ))
                .await
                .unwrap();
            }
        });
        for topic in ["a", "b"] {
            client.exchange(&json!({"version":1,"type":"poll","id":"poll","topic":topic,"after":"00000009"})).await.unwrap();
        }
        client.poll("a", Some(1)).await.unwrap();
        task.await.unwrap();
        assert_eq!(client.progress().restart_counter, Some(0));
        assert_eq!(client.progress().cursors.len(), 2);
        let state = serde_json::to_vec(client.progress()).unwrap();
        let state: ClientProgress = serde_json::from_slice(&state).unwrap();
        let (socket, _) = sockets().await;
        let client = RestartClient::new(socket, welcome(0), state).unwrap();
        assert!(client.restart_detected());
        assert!(client.progress().cursors.is_empty());
    }
    #[tokio::test]
    async fn failed_poll_does_not_consume_restart_replay_and_owner_binding_is_checked() {
        let (socket, mut peer) = sockets().await;
        let progress = ClientProgress {
            restart_counter: Some(8),
            ..Default::default()
        };
        let mut client = RestartClient::new(socket, welcome(9), progress).unwrap();
        let task = tokio::spawn(async move {
            for index in 0..2 {
                let Message::Binary(bytes) = peer.next().await.unwrap().unwrap() else {
                    panic!()
                };
                let frame = pb::Frame::decode(bytes).unwrap();
                let Some(pb::Body::Poll(poll)) = frame.body else {
                    panic!()
                };
                assert!(poll.after.is_none());
                let response = if index == 0 {
                    pb::Frame::error(protocol::ApiError::exhausted(), Some(poll.id))
                } else {
                    pb::Frame {
                        version: 1,
                        body: Some(pb::Body::Messages(pb::Messages {
                            request_id: poll.id,
                            items: vec![],
                            next_cursor: "00000001".into(),
                            has_more: false,
                        })),
                    }
                };
                peer.send(Message::Binary(response.encode_to_vec().into()))
                    .await
                    .unwrap();
            }
        });
        for _ in 0..2 {
            client
                .exchange(
                    &json!({"version":1,"type":"poll","id":"p","topic":"a","after":"00000009"}),
                )
                .await
                .unwrap();
        }
        task.await.unwrap();
        let (socket, _) = sockets().await;
        assert!(
            RestartClient::new(
                socket,
                welcome(0),
                ClientProgress {
                    service_id: "other-owner".into(),
                    ..Default::default()
                }
            )
            .is_err()
        );
    }
}

#[cfg(test)]
mod interruption_tests {
    use super::*;
    use protocol::{ProtobufMessage, pb};
    use tokio_tungstenite::tungstenite::protocol::Role;
    #[tokio::test]
    async fn persisted_restart_reset_survives_client_interruption_before_poll() {
        let (a, b) = tokio::io::duplex(8192);
        let socket = WebSocketStream::from_raw_socket(a, Role::Client, None).await;
        let mut peer = WebSocketStream::from_raw_socket(b, Role::Server, None).await;
        // A prior connection already observed counter 2, persisted the reset, then stopped before polling.
        let progress = ClientProgress {
            service_id: "owner".into(),
            client_id: "client".into(),
            restart_counter: Some(2),
            reset_missing_cursors: true,
            ..Default::default()
        };
        let progress: ClientProgress =
            serde_json::from_slice(&serde_json::to_vec(&progress).unwrap()).unwrap();
        let mut client = RestartClient::new(
            socket,
            pb::AuthWelcome {
                request_id: "auth".into(),
                client_id: "client".into(),
                service_id: "owner".into(),
                restart_counter: Some(2),
            },
            progress,
        )
        .unwrap();
        assert!(!client.restart_detected());
        let server = tokio::spawn(async move {
            let Message::Binary(bytes) = peer.next().await.unwrap().unwrap() else {
                panic!()
            };
            let frame = pb::Frame::decode(bytes).unwrap();
            let Some(pb::Body::Poll(poll)) = frame.body else {
                panic!()
            };
            assert!(poll.after.is_none());
            peer.send(Message::Binary(
                pb::Frame {
                    version: 1,
                    body: Some(pb::Body::Messages(pb::Messages {
                        request_id: poll.id,
                        items: vec![],
                        next_cursor: "00000001".into(),
                        has_more: false,
                    })),
                }
                .encode_to_vec()
                .into(),
            ))
            .await
            .unwrap();
        });
        client
            .exchange(
                &json!({"version":1,"type":"poll","id":"poll","topic":"topic","after":"00000009"}),
            )
            .await
            .unwrap();
        server.await.unwrap();
        assert_eq!(client.progress().cursors["topic"], "00000001");
    }
}
