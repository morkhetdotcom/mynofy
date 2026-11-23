//! Tools-only MCP over bounded stdio, backed by the existing authenticated client.
use mynofy_client as client;
use mynofy_protocol::{self as protocol, BoxError};
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Duration};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    sync::mpsc,
    task::JoinHandle,
    time::timeout,
};
mod backend;
mod tools;
const MODERN: &str = "2026-07-28";
const LEGACY: &str = "2025-11-25";
const VERSION_KEY: &str = "io.modelcontextprotocol/protocolVersion";
const CAPS_KEY: &str = "io.modelcontextprotocol/clientCapabilities";
const MAX_INPUT: usize = 64 * 1024;
const MAX_OUTPUT: usize = 16 * 1024 * 1024;
fn info() -> Value {
    json!({"name":"mynofy","version":env!("CARGO_PKG_VERSION")})
}
fn capabilities() -> Value {
    json!({"tools":{"listChanged":false}})
}
fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}
fn result(id: Value, mut value: Value, modern: bool) -> Value {
    if modern {
        value["resultType"] = "complete".into();
        value["_meta"] = json!({"io.modelcontextprotocol/serverInfo":info()});
    }
    json!({"jsonrpc":"2.0","id":id,"result":value})
}
fn tool_result(mut value: Value, error: bool) -> Value {
    tools::lossless_output(&mut value);
    json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":error})
}
fn valid_id(id: &Value) -> bool {
    id.is_string() || id.as_u64().is_some() || id.as_i64().is_some()
}
#[derive(Default, PartialEq)]
enum Phase {
    #[default]
    New,
    Initializing,
    Ready,
}
#[derive(Default)]
struct Session {
    phase: Phase,
}
enum Action {
    Reply(Value),
    Call {
        id: Value,
        request: Option<Value>,
        modern: bool,
    },
    Cancel(Value),
    None,
}
impl Session {
    fn prepare(&mut self, bytes: &[u8]) -> Action {
        let parsed = std::str::from_utf8(bytes)
            .ok()
            .and_then(|s| protocol::strict_json(s).ok());
        let Some(value) = parsed else {
            return Action::Reply(rpc_error(Value::Null, -32700, "Invalid JSON"));
        };
        let id = value.get("id").cloned().unwrap_or(Value::Null);
        let Some(fields) = value.as_object() else {
            return Action::Reply(rpc_error(
                Value::Null,
                -32600,
                "Expected one JSON-RPC object",
            ));
        };
        if value["jsonrpc"] != "2.0"
            || value["method"].as_str().is_none()
            || fields
                .keys()
                .any(|k| !["jsonrpc", "method", "id", "params"].contains(&k.as_str()))
            || value.get("params").is_some_and(|p| !p.is_object())
            || (fields.contains_key("id") && !valid_id(&id))
        {
            return Action::Reply(rpc_error(
                if valid_id(&id) { id } else { Value::Null },
                -32600,
                "Invalid JSON-RPC request",
            ));
        }
        let method = value["method"].as_str().expect("checked method");
        let params = value.get("params").cloned().unwrap_or_else(|| json!({}));
        if !fields.contains_key("id") {
            if method == "notifications/initialized" && self.phase == Phase::Initializing {
                self.phase = Phase::Ready;
            }
            if method == "notifications/cancelled" && valid_id(&params["requestId"]) {
                return Action::Cancel(params["requestId"].clone());
            }
            // In particular, notifications must never execute tools or initialize a session.
            return Action::None;
        }
        let meta = params.get("_meta");
        if meta.is_some_and(|m| !m.is_object()) {
            return Action::Reply(rpc_error(id, -32602, "Invalid request metadata"));
        }
        let modern = if let Some(version) = meta.and_then(|m| m.get(VERSION_KEY)) {
            let Some(version) = version.as_str() else {
                return Action::Reply(rpc_error(id, -32602, "Protocol version must be a string"));
            };
            if version != MODERN {
                let mut error = rpc_error(id, -32022, "Unsupported protocol version");
                error["error"]["data"] = json!({"supported":[MODERN,LEGACY],"requested":version});
                return Action::Reply(error);
            }
            if !meta
                .and_then(|m| m.get(CAPS_KEY))
                .is_some_and(Value::is_object)
            {
                return Action::Reply(rpc_error(id, -32602, "Missing client capabilities"));
            }
            true
        } else {
            false
        };
        if method == "initialize" {
            if modern
                || self.phase != Phase::New
                || params["protocolVersion"].as_str().is_none()
                || !params["capabilities"].is_object()
                || params["clientInfo"]["name"].as_str().is_none()
                || params["clientInfo"]["version"].as_str().is_none()
            {
                return Action::Reply(rpc_error(id, -32602, "Invalid initialization"));
            }
            self.phase = Phase::Initializing;
            return Action::Reply(result(
                id,
                json!({"protocolVersion":LEGACY,"capabilities":capabilities(),"serverInfo":info(),"instructions":"Use existing topics and configured credentials. Poll explicitly; publish business results as separate messages. No automatic mutation retries."}),
                false,
            ));
        }
        if !modern && method != "ping" && self.phase != Phase::Ready {
            return Action::Reply(rpc_error(
                id,
                -32602,
                "Initialize legacy MCP or provide per-request protocol metadata",
            ));
        }
        match method {
            "server/discover" if modern => Action::Reply(result(
                id,
                json!({"supportedVersions":[MODERN,LEGACY],"capabilities":capabilities()}),
                true,
            )),
            "ping" => Action::Reply(result(id, json!({}), modern)),
            "tools/list" => {
                if params
                    .as_object()
                    .expect("object")
                    .keys()
                    .any(|k| k != "_meta")
                {
                    return Action::Reply(rpc_error(
                        id,
                        -32602,
                        "Tool list has no cursor or filters",
                    ));
                }
                Action::Reply(result(id, tools::list(), modern))
            }
            "tools/call" => {
                if params
                    .as_object()
                    .expect("object")
                    .keys()
                    .any(|k| !["name", "arguments", "_meta"].contains(&k.as_str()))
                {
                    return Action::Reply(rpc_error(id, -32602, "Unknown tool-call parameter"));
                }
                let Some(name) = params["name"].as_str() else {
                    return Action::Reply(rpc_error(id, -32602, "Missing tool name"));
                };
                let args = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                match tools::command(name, &args) {
                    Ok(request) => Action::Call {
                        id,
                        request,
                        modern,
                    },
                    Err(message) => Action::Reply(rpc_error(id, -32602, message)),
                }
            }
            _ => Action::Reply(rpc_error(id, -32601, "Method not available")),
        }
    }
}
struct Active {
    id: Value,
    modern: bool,
    task: JoinHandle<(backend::Backend, Value)>,
}
impl Drop for Active {
    fn drop(&mut self) {
        self.task.abort();
    }
}
struct ReaderTask(JoinHandle<()>);
impl Drop for ReaderTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}
async fn line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<Result<Vec<u8>, ()>>> {
    let mut bytes = Vec::new();
    let mut too_large = false;
    loop {
        let input = reader.fill_buf().await?;
        if input.is_empty() {
            return if bytes.is_empty() && !too_large {
                Ok(None)
            } else {
                Ok(Some(if too_large { Err(()) } else { Ok(bytes) }))
            };
        }
        let newline = input.iter().position(|b| *b == b'\n');
        let n = newline.map_or(input.len(), |i| i + 1);
        if !too_large {
            if bytes.len() + n > MAX_INPUT {
                too_large = true;
                bytes.clear();
            } else {
                bytes.extend_from_slice(&input[..n]);
            }
        }
        reader.consume(n);
        if newline.is_some() {
            return Ok(Some(if too_large { Err(()) } else { Ok(bytes) }));
        }
    }
}
async fn write<W: AsyncWrite + Unpin>(output: &mut W, value: &Value) -> Result<(), BoxError> {
    let mut bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_OUTPUT {
        return Err("MCP response exceeds output limit".into());
    }
    bytes.push(b'\n');
    timeout(Duration::from_secs(10), async {
        output.write_all(&bytes).await?;
        output.flush().await
    })
    .await??;
    Ok(())
}
pub async fn run(flags: &BTreeMap<String, String>) -> Result<(), BoxError> {
    let config = backend::Config::parse(flags)?;
    tokio::select! {result=serve(config,tokio::io::stdin(),tokio::io::stdout())=>result,result=super::shutdown()=>{result?;Ok(())}}
}
async fn serve<R: AsyncRead + Unpin + Send + 'static, W: AsyncWrite + Unpin>(
    config: backend::Config,
    input: R,
    mut output: W,
) -> Result<(), BoxError> {
    let (tx, mut incoming) = mpsc::channel(8);
    let _reader = ReaderTask(tokio::spawn(async move {
        let mut reader = BufReader::new(input);
        loop {
            match line(&mut reader).await {
                Ok(Some(line)) => {
                    if tx.send(Ok(line)).await.is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    let _ = tx.send(Err(error)).await;
                    break;
                }
            }
        }
    }));
    let mut session = Session::default();
    let mut backend = Some(backend::Backend::new(config.clone()));
    let mut active: Option<Active> = None;
    loop {
        tokio::select! {
            input=incoming.recv()=>{
                let Some(input)=input else{break};
                let action=match input? {Ok(bytes)=>session.prepare(&bytes),Err(())=>Action::Reply(rpc_error(Value::Null,-32600,"MCP input line exceeds 64 KiB"))};
                match action {
                    Action::Reply(value)=>write(&mut output,&value).await?,
                    Action::None=>{},
                    Action::Cancel(id)=>{if active.as_ref().is_some_and(|a|a.id==id){active.take();backend=Some(backend::Backend::new(config.clone()));}},
                    Action::Call {id,request,modern}=>{
                        if let Some(current)=&active {
                            let response=if current.id==id {rpc_error(id,-32600,"Request ID already in flight")}else{result(id,tool_result(json!({"code":"busy","message":"One owner operation is already running. No operation was queued."}),true),modern)};
                            write(&mut output,&response).await?;continue;
                        }
                        let mut owner=backend.take().expect("idle backend");
                        let task=tokio::spawn(async move {let value=owner.call(request).await;(owner,value)});
                        active=Some(Active {id,modern,task});
                    },
                }
            },
            completed=async {match active.as_mut(){Some(a)=>(&mut a.task).await,None=>std::future::pending().await}},if active.is_some()=>{
                let current=active.take().expect("active request");
                let value=match completed {Ok((owner,value))=>{backend=Some(owner);value},Err(_)=>{backend=Some(backend::Backend::new(config.clone()));tool_result(json!({"code":"internal_error","message":"Owner operation interrupted; state must be checked before repeating a mutation.","outcome_unknown":true}),true)}};
                write(&mut output,&result(current.id.clone(),value,current.modern)).await?;
            },
        }
    }
    // EOF cancels the pending call and closes its socket; no detached publish survives shutdown.
    drop(active);
    Ok(())
}
#[cfg(test)]
mod tests;
