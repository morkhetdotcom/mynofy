//! Prepared additional-operation and varied-deadline workloads, excluding request generation.
#![forbid(unsafe_code)]
mod support;
use mynofy_server::{
    BoxError,
    config::{Action, Client, Config, PublicKey, Rule},
    crypto,
    engine::Engine,
    output::ResponseBuffer,
    protocol::{self, ProtobufMessage, payload::Payload, pb},
};
use support::*;
fn main() -> Result<(), BoxError> {
    let iterations = std::env::args()
        .skip(1)
        .find_map(|s| s.strip_prefix("--iterations=").map(str::to_owned))
        .unwrap_or_else(|| "10000".into())
        .parse::<usize>()?;
    assert!((1..=100000).contains(&iterations));
    tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
  let key=crypto::generate_key()?;let public=crypto::public_key(&key)?;let client=crypto::initial_client_id(&public);
  let mut cfg:Config=serde_json::from_str(include_str!("../../../config/scenarios/local.json"))?;
  cfg.clients=vec![Client{client_id:client.clone(),display_name:None,disabled:false,keys:vec![PublicKey{algorithm:"ed25519".into(),public_key:crypto::encode(&public),valid_until:protocol::timestamp(NOW+86400000)?}]}];
  cfg.topics[0].acl=Some(vec![Rule{client_id:client,allow:vec![Action::Publish,Action::Subscribe,Action::InspectOwn]}]);
  cfg.limits.client_burst=1000000;cfg.limits.client_requests_per_second=1000000;
  let mut engine=Engine::new(cfg,NOW)?;engine.start_id_allocator();let session=authenticate(&mut engine,&key);
  // Setup a varied-expiry page, so formatting is not hidden by the last-expiry cache.
  for i in 0..100 {let frame=frame(pb::Body::Publish(pb::Publish{id:"seed".into(),topic:"local/events".into(),payload:Payload::bytes(vec![1;32],None),expires:Some(pb::Expires{time:protocol::timestamp(NOW+10000+i)?})}));let r=engine.handle_protobuf(session,&frame.encode_to_vec(),NOW);assert!(matches!(r.body,Some(pb::Body::Result(_))));}
  let poll=frame(pb::Body::Poll(pb::Poll{id:"seed-poll".into(),topic:"local/events".into(),after:None,limit:Some(100)}));let p=engine.handle_protobuf(session,&poll.encode_to_vec(),NOW);
  let Some(pb::Body::Messages(page))=p.body else {panic!("messages expected")};let mid=page.items[0].message.id.clone();
  let cases=[
   ("get_message",frame(pb::Body::GetMessage(pb::GetMessage{id:"op".into(),message_id:mid}))),
   ("list_messages",frame(pb::Body::ListMessages(pb::ListMessages{id:"op".into(),topic:Some("local/events".into()),limit:Some(20),cursor:None}))),
   ("get_permissions",frame(pb::Body::GetPermissions(pb::GetPermissions{id:"op".into(),topic:"local/events".into()}))),
   ("poll_varied_expiry",poll)];
  let mut output=ResponseBuffer::new(131072);
  for (name,frame) in cases {let raw=frame.encode_to_vec();let expected=match name {"get_message"=>"message_status","list_messages"=>"messages_page","get_permissions"=>"permissions",_=>"messages"};
   let run=if name=="poll_varied_expiry"{iterations/10+1}else{iterations};let mut samples=Vec::with_capacity(run);
   for _ in 0..200 {let response=engine.handle_protobuf(session,&raw,NOW);assert_eq!(response.kind(),expected);}
   for _ in 0..run {let response=request(&mut engine,session,&raw,&mut samples,"protobuf",Some(&mut output));let Response::Binary(response)=response else {panic!("protobuf")};assert_eq!(response.kind(),expected);}
   report(name,&mut samples,run);
  }
  println!("{}",serde_json::json!({"phase":"resources","rss_bytes":rss_bytes(),"record_bytes":engine.storage_stats().record_bytes,"request_generation_timed":false}));
  Ok::<_,BoxError>(())
 })
}
