//! One locally configured trigger, one bounded worker. No runtime command installation.
use crate::{BoxError, exchange_frame, protocol};
use protocol::{
    payload::{Body, Event, Payload},
    pb,
};
use serde::{Deserialize, Serialize};
use std::{
    future::{Future, pending},
    pin::Pin,
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};
use tokio_tungstenite::WebSocketStream;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterConfig {
    pub name: String,
    pub revision: u32,
    pub profile: String,
    pub effect: String,
    pub communication_topic: String,
    pub status_topic: String,
    pub interpretation_topic: String,
    pub trigger_topic: String,
    /// Exact event match including parameters; use a separate run-specific configuration.
    pub trigger: Event,
    pub started: Event,
    pub success: Event,
    pub failure: Event,
    pub mode: Mode,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub poll_interval_ms: u64,
    pub renew_seconds: u64,
    pub announcement_ttl_seconds: u64,
    pub execution_timeout_seconds: u64,
    pub max_output_bytes: usize,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Once,
    StdioJsonl,
}
impl AdapterConfig {
    pub fn validate(&self) -> Result<(), BoxError> {
        if self.name.is_empty()
            || self.name.len() > 128
            || self.profile.is_empty()
            || self.profile.len() > 128
            || self.effect.is_empty()
            || self.effect.len() > 512
            || self.revision == 0
            || self.program.is_empty()
            || self.args.len() > 64
            || self.args.iter().any(|s| s.len() > 4096)
            || !(100..=60000).contains(&self.poll_interval_ms)
            || !(1..=3600).contains(&self.renew_seconds)
            || self.renew_seconds >= self.announcement_ttl_seconds
            || self.announcement_ttl_seconds > 86400
            || !(1..=3600).contains(&self.execution_timeout_seconds)
            || !(1..=4096).contains(&self.max_output_bytes)
        {
            return Err("Invalid adapter identity, process or timing limits".into());
        }
        let topics = [
            &self.communication_topic,
            &self.status_topic,
            &self.interpretation_topic,
        ];
        if topics.iter().any(|s| s.is_empty() || s.len() > 256)
            || topics[0] == topics[1]
            || topics[0] == topics[2]
            || topics[1] == topics[2]
            || ![&self.communication_topic, &self.status_topic].contains(&&self.trigger_topic)
        {
            return Err(
                "Use three distinct topics; trigger must use communication or status".into(),
            );
        }
        for event in [&self.trigger, &self.started, &self.success, &self.failure] {
            Payload::event(event.clone()).validate()?;
        }
        if [&self.started, &self.success, &self.failure].contains(&&self.trigger) {
            return Err("Adapter status must not match its own trigger".into());
        }
        if self.trigger.status != 7
            || self.started.status != 2
            || self.success.status != 2
            || self.failure.status != 3
        {
            return Err("Expected PENDING trigger, SUCCESS start/success, FAILED failure".into());
        }
        self.announcement("validation", "validation")?.validate()?;
        Ok(())
    }
    fn announcement(&self, participant: &str, instance: &str) -> Result<Payload, BoxError> {
        let value = serde_json::json!({"kind":"interpretation","participant":participant,"instance":instance,"name":self.name,"revision":self.revision,"profile":self.profile,"effect":self.effect,"communication_topic":self.communication_topic,"status_topic":self.status_topic,"trigger_topic":self.trigger_topic,"trigger":self.trigger,"started":self.started,"success":self.success,"failure":self.failure,"renew_seconds":self.renew_seconds,"ttl_seconds":self.announcement_ttl_seconds});
        let payload = Payload::bytes(serde_json::to_vec(&value)?, Some("application/json".into()));
        payload.validate()?;
        Ok(payload)
    }
}
struct Pipes {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}
impl Pipes {
    fn start(config: &AdapterConfig) -> Result<Self, BoxError> {
        let mut command = Command::new(&config.program);
        command
            .args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn()?;
        let stdin = child.stdin.take().ok_or("No stdin pipe")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("No stdout pipe")?);
        Ok(Self {
            child,
            stdin: Some(stdin),
            stdout,
        })
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn exited(child: &Child) -> std::io::Result<bool> {
        use rustix::process::{Pid, WaitId, WaitIdOptions, waitid};
        let Some(pid) = child
            .id()
            .and_then(|id| i32::try_from(id).ok())
            .and_then(Pid::from_raw)
        else {
            return Ok(true);
        };
        // WNOWAIT leaves the exit status pending, pinning the group leader's PID.
        match waitid(
            WaitId::Pid(pid),
            WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
        ) {
            Ok(status) => Ok(status.is_some()),
            Err(rustix::io::Errno::INTR) => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    fn running(&mut self) -> std::io::Result<bool> {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            if !Self::exited(&self.child)? {
                return Ok(true);
            }
            self.stop_group();
        }
        Ok(self.child.try_wait()?.is_none())
    }

    fn stop_group(&mut self) {
        #[cfg(unix)]
        signal_group(self.child.id().and_then(|id| i32::try_from(id).ok()));
    }
}

#[cfg(unix)]
fn signal_group(id: Option<i32>) {
    if let Some(id) = id {
        // The caller still owns the unreaped child, so its group ID cannot be reused.
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(id),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
}

impl Drop for Pipes {
    fn drop(&mut self) {
        self.stop_group();
    }
}
async fn line_bounded(
    reader: &mut BufReader<ChildStdout>,
    max: usize,
) -> Result<Vec<u8>, BoxError> {
    let mut output = Vec::new();
    loop {
        let chunk = reader.fill_buf().await?;
        if chunk.is_empty() {
            return Err("Process closed stdout before complete JSONL response".into());
        }
        let count = chunk
            .iter()
            .position(|&b| b == b'\n')
            .map_or(chunk.len(), |i| i + 1);
        let ended = chunk[count - 1] == b'\n';
        if output.len() + count > max + 1 {
            return Err("Process output exceeds configured limit".into());
        }
        output.extend_from_slice(&chunk[..count]);
        reader.consume(count);
        if ended {
            output.pop();
            return Ok(output);
        }
    }
}
type WorkResult = (Option<Pipes>, Result<Vec<u8>, BoxError>);
struct Work {
    task: tokio::task::JoinHandle<WorkResult>,
}
impl Drop for Work {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Future for Work {
    type Output = WorkResult;
    fn poll(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        match Pin::new(&mut self.get_mut().task).poll(cx) {
            std::task::Poll::Ready(Ok(result)) => std::task::Poll::Ready(result),
            std::task::Poll::Ready(Err(error)) => std::task::Poll::Ready((None, Err(error.into()))),
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}
fn work(config: AdapterConfig, pipes: Option<Pipes>) -> Work {
    Work {
        task: tokio::spawn(async move {
            let mut pipes = match pipes {
                Some(p) => p,
                None => match Pipes::start(&config) {
                    Ok(p) => p,
                    Err(e) => return (None, Err(e)),
                },
            };
            let operation = async {
                // Fixed JSONL representation of the event; never construct a shell command.
                let mut input =
                    serde_json::to_vec(&Payload::event(config.trigger.clone()).to_json()?)?;
                input.push(b'\n');
                if config.mode == Mode::StdioJsonl {
                    let stdin = pipes.stdin.as_mut().ok_or("Process stdin closed")?;
                    stdin.write_all(&input).await?;
                    stdin.flush().await?;
                    let output = line_bounded(&mut pipes.stdout, config.max_output_bytes).await?;
                    protocol::strict_json(std::str::from_utf8(&output)?)?;
                    Ok::<_, BoxError>(output)
                } else {
                    // Drain stdout while writing stdin to avoid a pipe-to-pipe deadlock.
                    let input_future = async {
                        let mut stdin = pipes.stdin.take().ok_or("Process stdin closed")?;
                        stdin.write_all(&input).await?;
                        drop(stdin);
                        Ok::<(), BoxError>(())
                    };
                    let output_future = async {
                        let mut out = Vec::new();
                        (&mut pipes.stdout)
                            .take((config.max_output_bytes + 1) as u64)
                            .read_to_end(&mut out)
                            .await?;
                        if out.len() > config.max_output_bytes {
                            return Err("Process output exceeds configured limit".into());
                        }
                        Ok::<_, BoxError>(out)
                    };
                    #[cfg(any(target_os = "linux", target_os = "macos"))]
                    let (_, output) = {
                        let io = async { tokio::try_join!(input_future, output_future) };
                        tokio::pin!(io);
                        let child = &pipes.child;
                        let result = tokio::select! {
                            result = &mut io => Some(result),
                            exited = async {
                                while !Pipes::exited(child)? {
                                    tokio::time::sleep(Duration::from_millis(5)).await;
                                }
                                Ok::<_, std::io::Error>(())
                            } => { exited?; None }
                        };
                        // The I/O future only borrows stdin and stdout.
                        let id = child.id().and_then(|id| i32::try_from(id).ok());
                        if let Some(result) = result {
                            let output = result?;
                            while !Pipes::exited(child)? {
                                tokio::time::sleep(Duration::from_millis(5)).await;
                            }
                            signal_group(id);
                            output
                        } else {
                            signal_group(id);
                            io.await?
                        }
                    };
                    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
                    let (_, output) = tokio::try_join!(input_future, output_future)?;
                    if !pipes.child.wait().await?.success() {
                        return Err("Process returned a nonzero exit code".into());
                    }
                    Ok(output)
                }
            };
            let result = match tokio::time::timeout(
                Duration::from_secs(config.execution_timeout_seconds),
                operation,
            )
            .await
            {
                Ok(r) => r,
                Err(_) => Err("Process execution timed out".into()),
            };
            if result.is_err() || config.mode == Mode::Once {
                pipes.stop_group();
                let _ = pipes.child.kill().await;
                let _ = pipes.child.wait().await;
                (None, result)
            } else {
                (Some(pipes), result)
            }
        }),
    }
}
async fn publish<S: AsyncRead + AsyncWrite + Unpin>(
    ws: &mut WebSocketStream<S>,
    topic: &str,
    payload: Payload,
    ttl: Option<u64>,
) -> Result<(), BoxError> {
    let expires = if let Some(ttl) = ttl {
        let ms = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
        Some(pb::Expires {
            time: protocol::timestamp(ms + ttl * 1000)?,
        })
    } else {
        None
    };
    let response = exchange_frame(
        ws,
        &pb::Frame {
            version: 1,
            body: Some(pb::Body::Publish(pb::Publish {
                id: "adapter-publish".into(),
                topic: topic.into(),
                payload,
                expires,
            })),
        },
    )
    .await?;
    if !matches!(response.body, Some(pb::Body::Result(_))) {
        return Err(format!("Adapter publish rejected: {}", response.to_json()?).into());
    }
    Ok(())
}
/// Runs until cancelled or connection failure. Dropping cancels its direct child process.
/// State/checkpoints are RAM-only; reconnect/restart is not exactly-once execution.
pub async fn run<S: AsyncRead + AsyncWrite + Unpin>(
    mut ws: WebSocketStream<S>,
    config: AdapterConfig,
    participant: &str,
) -> Result<(), BoxError> {
    config.validate()?;
    let instance = crate::crypto::encode(&crate::crypto::random::<16>()?);
    let announcement = config.announcement(participant, &instance)?;
    let mut renewal = tokio::time::interval(Duration::from_secs(config.renew_seconds));
    let mut polling = tokio::time::interval(Duration::from_millis(config.poll_interval_ms));
    renewal.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    polling.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut cursor = None;
    let mut active: Option<Work> = None;
    let mut idle_pipes = None;
    // Publish before any trigger processing; subsequent ticks renew even during execution.
    publish(
        &mut ws,
        &config.interpretation_topic,
        announcement.clone(),
        Some(config.announcement_ttl_seconds),
    )
    .await?;
    renewal.tick().await;
    loop {
        tokio::select! {
            _=renewal.tick()=>publish(&mut ws,&config.interpretation_topic,announcement.clone(),Some(config.announcement_ttl_seconds)).await?,
            result=async{match &mut active{Some(work)=>work.await,None=>pending().await}}=>{
                active=None;let (pipes,result)=result;idle_pipes=pipes;
                let success=result.is_ok();
                if let Ok(output)=result && !output.is_empty(){publish(&mut ws,&config.status_topic,Payload::bytes(output,Some(if config.mode==Mode::StdioJsonl{"application/json"}else{"application/octet-stream"}.into())),None).await?;}
                let event=if success {config.success.clone()}else{config.failure.clone()};
                publish(&mut ws,&config.status_topic,Payload::event(event),None).await?;
            },
            _=polling.tick(),if active.is_none()=>{
                let response=exchange_frame(&mut ws,&pb::Frame{version:1,body:Some(pb::Body::Poll(pb::Poll{id:"adapter-poll".into(),topic:config.trigger_topic.clone(),after:cursor.clone(),limit:Some(1)}))}).await?;
                let Some(pb::Body::Messages(page))=response.body else{return Err("Adapter poll rejected".into());};
                for item in page.items {
                    let matches=matches!(&item.message.payload.body,Some(Body::Event(e)) if *e==config.trigger);
                    if matches {
                        let pipes=if let Some(mut pipes)=idle_pipes.take(){
                            if pipes.running()?{Ok(pipes)}else{Pipes::start(&config)}
                        }else{Pipes::start(&config)};
                        match pipes {
                            Ok(pipes)=>{
                                publish(&mut ws,&config.status_topic,Payload::event(config.started.clone()),None).await?;
                                active=Some(work(config.clone(),Some(pipes)));
                            },
                            Err(_)=>publish(&mut ws,&config.status_topic,Payload::event(config.failure.clone()),None).await?,
                        }
                    }
                }
                cursor=Some(page.next_cursor);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> AdapterConfig {
        let event = |action, status| Event {
            catalog: 1,
            catalog_version: 1,
            category: 12,
            action,
            status,
            parameters: vec![],
        };
        AdapterConfig {
            name: "test".into(),
            revision: 1,
            profile: "job-test-v1".into(),
            effect: "Run configured test process".into(),
            communication_topic: "job/1/communication".into(),
            status_topic: "job/1/status".into(),
            interpretation_topic: "job/1/interpretations".into(),
            trigger_topic: "job/1/communication".into(),
            trigger: event(40, 7),
            started: event(10, 2),
            success: event(40, 2),
            failure: event(40, 3),
            mode: Mode::Once,
            program: "/bin/cat".into(),
            args: vec![],
            poll_interval_ms: 100,
            renew_seconds: 1,
            announcement_ttl_seconds: 3,
            execution_timeout_seconds: 1,
            max_output_bytes: 4096,
        }
    }
    fn valid_config() -> AdapterConfig {
        config()
    }
    #[test]
    fn config_and_announcements_are_bounded_and_do_not_expose_program() {
        let c = valid_config();
        c.validate().unwrap();
        let p = c.announcement("client:test", "instance").unwrap();
        let Some(Body::Data(data)) = p.body else {
            panic!("Expected metadata")
        };
        let value = protocol::strict_json(std::str::from_utf8(&data).unwrap()).unwrap();
        assert!(value.get("program").is_none());
        assert!(data.len() <= 4096);
        let mut bad = c.clone();
        bad.renew_seconds = bad.announcement_ttl_seconds;
        assert!(bad.validate().is_err());
        let mut bad = c.clone();
        bad.trigger_topic = bad.interpretation_topic.clone();
        assert!(bad.validate().is_err());
        let mut bad = c.clone();
        bad.trigger = bad.success.clone();
        assert!(bad.validate().is_err());
        let mut bad = c;
        bad.max_output_bytes = 4097;
        assert!(bad.validate().is_err());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_and_timeout_kill_the_adapter_process_group() {
        let dir = std::env::temp_dir().join(format!(
            "mynofy-adapter-{}",
            crate::crypto::encode(&crate::crypto::random::<8>().unwrap()).replace('/', "_")
        ));
        std::fs::create_dir(&dir).unwrap();
        let marker = dir.join("survived");
        let mut c = valid_config();
        c.program = "/bin/sh".into();
        c.args = vec![
            "-c".into(),
            r#"(sleep 0.8; printf done > "$1") & printf 'started\n'; wait"#.into(),
            "mynofy-test".into(),
            marker.to_string_lossy().into_owned(),
        ];
        let mut pipes = Pipes::start(&c).unwrap();
        let line =
            tokio::time::timeout(Duration::from_secs(1), line_bounded(&mut pipes.stdout, 100))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(line, b"started");
        drop(pipes);
        tokio::time::sleep(Duration::from_millis(1000)).await;
        assert!(!marker.exists());
        c.args[1] = r#"(sleep 1.6; printf done > "$1") & wait"#.into();
        let (pipes, result) = work(c, None).await;
        assert!(pipes.is_none());
        assert!(result.unwrap_err().to_string().contains("timed out"));
        tokio::time::sleep(Duration::from_millis(800)).await;
        assert!(!marker.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn normal_adapter_exit_cleans_group_before_reaping() {
        let dir = std::env::temp_dir().join(format!(
            "mynofy-adapter-exit-{:x}",
            u64::from_le_bytes(crate::crypto::random().unwrap())
        ));
        std::fs::create_dir(&dir).unwrap();
        let marker = dir.join("survived");
        let mut config = valid_config();
        config.program = "/bin/sh".into();
        config.args = vec![
            "-c".into(),
            r#"cat >/dev/null; (sleep 0.5; printf leaked > "$1") & printf 'done'; exit 0"#.into(),
            "mynofy-test".into(),
            marker.to_string_lossy().into_owned(),
        ];
        let (pipes, output) = work(config.clone(), None).await;
        assert!(pipes.is_none());
        assert_eq!(output.unwrap(), b"done");

        config.mode = Mode::StdioJsonl;
        config.args[1] = r#"read -r entry; (sleep 0.5; printf leaked > "$1") & printf '{}
'; exit 0"#
            .into();
        let (pipes, output) = work(config, None).await;
        assert!(output.is_ok());
        let mut pipes = pipes.unwrap();
        tokio::time::timeout(Duration::from_millis(300), async {
            while pipes.running().unwrap() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(!marker.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn once_and_persistent_jsonl_processes_follow_framing() {
        let c = valid_config();
        let (pipes, result) = work(c.clone(), None).await;
        assert!(pipes.is_none());
        assert_eq!(
            protocol::strict_json(std::str::from_utf8(&result.unwrap()).unwrap().trim()).unwrap()["kind"],
            "event"
        );
        let mut c = c;
        c.mode = Mode::StdioJsonl;
        let (pipes, one) = work(c.clone(), None).await;
        assert!(one.is_ok());
        let pid = pipes.as_ref().unwrap().child.id();
        let (pipes, two) = work(c, pipes).await;
        assert!(two.is_ok());
        assert_eq!(pipes.as_ref().unwrap().child.id(), pid);
        drop(pipes);
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn process_failures_output_cap_and_timeout_do_not_leave_reusable_worker() {
        for script in [
            "cat >/dev/null; exit 7",
            "cat >/dev/null; printf '%5000s' x",
            "cat >/dev/null; exec sleep 5",
        ] {
            let mut c = valid_config();
            c.program = "/bin/sh".into();
            c.args = vec!["-c".into(), script.into()];
            let (pipes, result) = work(c, None).await;
            assert!(result.is_err());
            assert!(pipes.is_none());
        }
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn interpretation_renews_while_process_is_running() {
        use crate::protocol::ProtobufMessage as _;
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::{Message, protocol::Role};
        let (a, b) = tokio::io::duplex(16384);
        let client = WebSocketStream::from_raw_socket(a, Role::Client, None).await;
        let mut peer = WebSocketStream::from_raw_socket(b, Role::Server, None).await;
        let mut c = valid_config();
        c.program = "/bin/sh".into();
        c.args = vec!["-c".into(), "cat >/dev/null; exec sleep 3".into()];
        c.execution_timeout_seconds = 5;
        let reference = c.clone();
        let responder = tokio::spawn(async move {
            let mut sent_trigger = false;
            let mut renewals = 0;
            let mut started = false;
            while let Some(Ok(Message::Binary(raw))) = peer.next().await {
                let request = pb::Frame::decode(raw).unwrap();
                let id = request.request_id().unwrap().to_owned();
                let body = match request.body.unwrap() {
                    pb::Body::Publish(p) => {
                        if p.topic == reference.interpretation_topic {
                            renewals += 1;
                            assert!(p.expires.is_some());
                        }
                        if p.payload == Payload::event(reference.started.clone()) {
                            started = true;
                        }
                        pb::Body::Result(pb::Result {
                            request_id: id,
                            message_id: Some("00000001".into()),
                            expires: Some(pb::Expires {
                                time: "2030-01-01T00:00:00Z".into(),
                            }),
                        })
                    }
                    pb::Body::Poll(_) => {
                        let items = if sent_trigger {
                            vec![]
                        } else {
                            sent_trigger = true;
                            vec![pb::PolledMessage {
                                message: pb::StoredMessage {
                                    producer_id: "test-producer".into(),
                                    id: "00000001".into(),
                                    topic: reference.trigger_topic.clone(),
                                    payload: Payload::event(reference.trigger.clone()),
                                    expires: pb::Expires {
                                        time: "2030-01-01T00:00:00Z".into(),
                                    },
                                },
                            }]
                        };
                        pb::Body::Messages(pb::Messages {
                            request_id: id,
                            items,
                            next_cursor: "00000001".into(),
                            has_more: false,
                        })
                    }
                    _ => panic!("Unexpected adapter request"),
                };
                peer.send(Message::Binary(
                    pb::Frame {
                        version: 1,
                        body: Some(body),
                    }
                    .encode_to_vec()
                    .into(),
                ))
                .await
                .unwrap();
                if renewals >= 2 && started {
                    return;
                }
            }
            panic!("Missing renewal during execution");
        });
        let adapter = run(client, c, "client:test");
        tokio::pin!(adapter);
        tokio::time::timeout(Duration::from_secs(5),async{tokio::select!{r=responder=>r.unwrap(),r=&mut adapter=>panic!("Adapter ended early: {r:?}"),}}).await.unwrap();
        // Dropping the active adapter future kills its direct child (sleep).
    }
}
