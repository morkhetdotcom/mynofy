use crate::{Failure, Response, Result, abi, boundary, getter, payload, pb};
use abi::{Bytes, Str};

pub fn kind(r: &pb::Frame) -> u32 {
    match r.body {
        Some(pb::Body::Result(_)) => 1,
        Some(pb::Body::Messages(_)) => 2,
        Some(pb::Body::MessagesPage(_)) => 3,
        Some(pb::Body::MessageStatus(_)) => 4,
        Some(pb::Body::Permissions(_)) => 5,
        Some(pb::Body::Catalog(_)) => 6,
        Some(pb::Body::RotationComplete(_)) => 7,
        _ => 0,
    }
}
pub fn validate(
    r: &pb::Frame,
    id: &str,
    expected: u32,
    request: Option<&pb::Frame>,
) -> std::result::Result<(), String> {
    if r.version != 1 {
        return Err("invalid response version".into());
    }
    let reply = match &r.body {
        Some(pb::Body::Result(v)) => &v.request_id,
        Some(pb::Body::Messages(v)) => &v.request_id,
        Some(pb::Body::MessagesPage(v)) => &v.request_id,
        Some(pb::Body::MessageStatus(v)) => &v.request_id,
        Some(pb::Body::Permissions(v)) => &v.request_id,
        Some(pb::Body::Catalog(v)) => &v.request_id,
        Some(pb::Body::RotationComplete(v)) => &v.request_id,
        Some(pb::Body::Error(v)) => v.request_id.as_deref().unwrap_or(""),
        _ => return Err("unexpected response body".into()),
    };
    let early_rate_limit = matches!(&r.body, Some(pb::Body::Error(e))
        if e.code == "rate_limited" && e.request_id.is_none() && e.retry_after_ms.is_some());
    if (!early_rate_limit && reply != id)
        || (kind(r) != expected && !matches!(r.body, Some(pb::Body::Error(_))))
    {
        return Err("response type/request ID mismatch".into());
    }
    match (&r.body, request.and_then(|q| q.body.as_ref())) {
        (Some(pb::Body::Messages(v)), Some(pb::Body::Poll(q))) => {
            if !crate::cursor(&v.next_cursor) || v.items.len() > q.limit.unwrap_or(20) as usize {
                return Err("invalid poll page".into());
            }
            for item in &v.items {
                if item.message.topic != q.topic || item.message.producer_id.is_empty() {
                    return Err("poll topic mismatch".into());
                }
                item.message.payload.validate().map_err(|e| e.to_string())?;
            }
        }
        (Some(pb::Body::MessagesPage(v)), Some(pb::Body::ListMessages(q))) => {
            if v.items.len() > q.limit.unwrap_or(20) as usize
                || v.items
                    .iter()
                    .any(|v| q.topic.as_ref().is_some_and(|t| &v.topic != t))
            {
                return Err("invalid producer page".into());
            }
        }
        (Some(pb::Body::MessageStatus(v)), Some(pb::Body::GetMessage(q))) => {
            if v.message.id != q.message_id || v.message.producer_id.is_empty() {
                return Err("message ID mismatch".into());
            }
            v.message.payload.validate().map_err(|e| e.to_string())?;
        }
        (Some(pb::Body::Permissions(v)), Some(pb::Body::GetPermissions(q))) => {
            if v.topic != q.topic {
                return Err("permissions topic mismatch".into());
            }
        }
        (Some(pb::Body::Result(v)), Some(pb::Body::Publish(_)))
            if v.message_id.is_none() || v.expires.is_none() =>
        {
            return Err("publish result is incomplete".into());
        }
        _ => {}
    }
    Ok(())
}
fn count(r: &Response) -> usize {
    match &r.frame.body {
        Some(pb::Body::Messages(v)) => v.items.len(),
        Some(pb::Body::MessagesPage(v)) => v.items.len(),
        Some(pb::Body::MessageStatus(_)) => 1,
        _ => 0,
    }
}
fn stored(v: &pb::StoredMessage) -> abi::Message {
    let mut out = abi::Message {
        id: Str::new(&v.id),
        topic: Str::new(&v.topic),
        expires: Str::new(&v.expires.time),
        ..abi::Message::default()
    };
    out.content_type = v
        .payload
        .content_type
        .as_deref()
        .map(Str::new)
        .unwrap_or_default();
    match &v.payload.body {
        Some(payload::Body::Data(bytes)) => {
            out.payload_kind = 1;
            out.data = Bytes::new(bytes);
        }
        Some(payload::Body::Event(e)) => {
            out.payload_kind = 2;
            out.event = abi::event_view(e);
        }
        None => {}
    }
    out
}

fn message(r: &Response, index: usize) -> Result<abi::Message> {
    if index >= count(r) {
        return Err(Failure::invalid("message index out of bounds"));
    }
    Ok(match &r.frame.body {
        Some(pb::Body::Messages(v)) => {
            let v = &v.items[index];

            stored(&v.message)
        }
        Some(pb::Body::MessagesPage(v)) => {
            let v = &v.items[index];

            abi::Message {
                id: Str::new(&v.id),
                topic: Str::new(&v.topic),
                expires: Str::new(&v.expires.time),
                created_at: Str::new(&v.created_at),
                ..abi::Message::default()
            }
        }
        Some(pb::Body::MessageStatus(v)) => {
            let mut out = stored(&v.message);
            out.created_at = Str::new(&v.created_at);
            out
        }
        _ => unreachable!(),
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mynofy_response_free(p: *mut Response) {
    getter(|| {
        if !p.is_null() {
            // SAFETY: one free per owned response, no concurrent reads.
            drop(unsafe { Box::from_raw(p) });
        }
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mynofy_response_type(p: *const Response) -> u32 {
    getter(|| {
        if p.is_null() {
            0
        } else {
            // SAFETY: live immutable response.
            kind(&unsafe { &*p }.frame)
        }
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mynofy_response_message_count(p: *const Response) -> usize {
    getter(|| {
        if p.is_null() {
            0
        } else {
            // SAFETY: live immutable response.
            count(unsafe { &*p })
        }
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mynofy_response_message_at(
    p: *const Response,
    index: usize,
    out: *mut abi::Message,
    error: *mut abi::Error,
) -> i32 {
    boundary(error, || {
        // SAFETY: writable output and live immutable response.
        unsafe { abi::output(out, abi::Message::default()) }?;
        let r = unsafe { abi::reference(p) }?;
        unsafe { abi::output(out, message(r, index)?) }
    })
}
/// Owner-authenticated producer. No struct layout change; view borrows response.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mynofy_response_message_producer_id(
    p: *const Response,
    index: usize,
) -> Str {
    getter(|| {
        if p.is_null() {
            return Str::default();
        }
        // SAFETY: same live immutable response contract as the message accessors.
        let response = unsafe { &*p };
        let producer = match &response.frame.body {
            Some(pb::Body::Messages(v)) => v
                .items
                .get(index)
                .map(|item| item.message.producer_id.as_str()),
            Some(pb::Body::MessageStatus(v)) if index == 0 => Some(v.message.producer_id.as_str()),
            _ => None,
        };
        producer.map(Str::new).unwrap_or_default()
    })
}
// All generated accessor bodies have the same pointer/lifetime contract.
macro_rules! access {
    ($name:ident,$ty:ty,$r:ident,$value:expr) => {
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(p: *const Response) -> $ty {
            getter(|| {
                if p.is_null() {
                    <$ty>::default()
                } else {
                    // SAFETY: live immutable response; returned views borrow its allocation.
                    let $r = unsafe { &*p };
                    $value
                }
            })
        }
    };
}
access!(
    mynofy_response_message_id,
    Str,
    r,
    match &r.frame.body {
        Some(pb::Body::Result(v)) => v.message_id.as_deref().map(Str::new).unwrap_or_default(),
        _ => Str::default(),
    }
);
access!(
    mynofy_response_expires,
    Str,
    r,
    match &r.frame.body {
        Some(pb::Body::Result(v)) => v
            .expires
            .as_ref()
            .map(|v| Str::new(&v.time))
            .unwrap_or_default(),
        _ => Str::default(),
    }
);
access!(
    mynofy_response_next_cursor,
    Str,
    r,
    match &r.frame.body {
        Some(pb::Body::Messages(v)) => Str::new(&v.next_cursor),
        Some(pb::Body::MessagesPage(v)) =>
            v.next_cursor.as_deref().map(Str::new).unwrap_or_default(),
        _ => Str::default(),
    }
);
access!(
    mynofy_response_has_more,
    u32,
    r,
    match &r.frame.body {
        Some(pb::Body::Messages(v)) => u32::from(v.has_more),
        _ => 0,
    }
);
access!(
    mynofy_response_catalog,
    Bytes,
    r,
    match &r.frame.body {
        Some(pb::Body::Catalog(v)) => Bytes::new(&v.definition),
        _ => Bytes::default(),
    }
);
access!(
    mynofy_response_permission_count,
    usize,
    r,
    match &r.frame.body {
        Some(pb::Body::Permissions(v)) => v.actions.len(),
        _ => 0,
    }
);
access!(
    mynofy_response_owner,
    Str,
    r,
    match &r.frame.body {
        Some(pb::Body::Permissions(v)) => Str::new(&v.owner_id),
        _ => Str::default(),
    }
);
access!(
    mynofy_response_acl_revision,
    Str,
    r,
    match &r.frame.body {
        Some(pb::Body::Permissions(v)) => Str::new(&v.acl_revision),
        _ => Str::default(),
    }
);
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mynofy_response_permission_at(p: *const Response, index: usize) -> Str {
    getter(|| {
        if p.is_null() {
            Str::default()
        } else {
            // SAFETY: live immutable response.
            match &unsafe { &*p }.frame.body {
                Some(pb::Body::Permissions(v)) => v
                    .actions
                    .get(index)
                    .map(|s| Str::new(s))
                    .unwrap_or_default(),
                _ => Str::default(),
            }
        }
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mynofy_response_rotation(
    p: *const Response,
    out: *mut abi::Rotation,
    error: *mut abi::Error,
) -> i32 {
    boundary(error, || {
        // SAFETY: common C input/output contract.
        unsafe { abi::output(out, abi::Rotation::default()) }?;
        let r = unsafe { abi::reference(p) }?;
        let Some(pb::Body::RotationComplete(v)) = &r.frame.body else {
            return Err(Failure::invalid("not a rotation response"));
        };
        unsafe {
            abi::output(
                out,
                abi::Rotation {
                    rotation_id: Str::new(&v.rotation_id),
                    client_id: Str::new(&v.client_id),
                    token_id: Str::new(&v.token_id),
                    valid_until: Str::new(&v.valid_until),
                },
            )
        }
    })
}
