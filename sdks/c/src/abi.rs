//! Only representation and pointer validation. Network/business logic stays safe Rust.
use crate::{Failure, Result};
use std::{
    ffi::{CStr, c_char},
    slice,
};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Str {
    pub data: *const c_char,
    pub len: usize,
}
impl Default for Str {
    fn default() -> Self {
        Self {
            data: std::ptr::null(),
            len: 0,
        }
    }
}
impl Str {
    pub fn new(s: &str) -> Self {
        Self {
            data: s.as_ptr().cast(),
            len: s.len(),
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Bytes {
    pub data: *const u8,
    pub len: usize,
}
impl Default for Bytes {
    fn default() -> Self {
        Self {
            data: std::ptr::null(),
            len: 0,
        }
    }
}
impl Bytes {
    pub fn new(s: &[u8]) -> Self {
        Self {
            data: s.as_ptr(),
            len: s.len(),
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Config {
    pub struct_size: usize,
    pub abi_version: u32,
    pub timeout_ms: u32,
    pub url: *const c_char,
    pub socket_path: *const c_char,
    pub ca_file: *const c_char,
    pub owner: *const c_char,
    pub client_id: *const c_char,
    pub key: *const crate::Key,
    pub via_topic: *const c_char,
    pub owner_ca_file: *const c_char,
    pub owner_name: *const c_char,
}
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct Info {
    pub owner: Str,
    pub client_id: Str,
    pub restart_counter: u32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub union ParameterValue {
    pub u32_value: u32,
    pub u64_value: u64,
    pub i32_value: i32,
    pub i64_value: i64,
    pub f32_value: f32,
    pub f64_value: f64,
    pub bool_value: u32,
}
impl Default for ParameterValue {
    fn default() -> Self {
        Self { u64_value: 0 }
    }
}
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct Parameter {
    pub kind: u32,
    pub value: ParameterValue,
}
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct Event {
    pub catalog: u32,
    pub catalog_version: u32,
    pub category: u32,
    pub action: u32,
    pub status: u32,
    pub parameter_count: u32,
    pub parameters: [Parameter; 3],
}
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct Message {
    pub id: Str,
    pub topic: Str,
    pub expires: Str,
    pub created_at: Str,
    pub payload_kind: u32,
    pub data: Bytes,
    pub content_type: Str,
    pub event: Event,
}
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct Rotation {
    pub rotation_id: Str,
    pub client_id: Str,
    pub token_id: Str,
    pub valid_until: Str,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Error {
    pub code: i32,
    pub outcome_unknown: u32,
    pub has_retry_after_ms: u32,
    pub retry_after_ms: u64,
    pub broker_code: [c_char; 64],
    pub message: [c_char; 256],
}
impl Default for Error {
    fn default() -> Self {
        Self {
            code: 0,
            outcome_unknown: 0,
            has_retry_after_ms: 0,
            retry_after_ms: 0,
            broker_code: [0; 64],
            message: [0; 256],
        }
    }
}
impl Error {
    pub fn failure(f: &Failure) -> Self {
        let mut e = Self {
            code: f.code,
            outcome_unknown: u32::from(f.unknown),
            has_retry_after_ms: u32::from(f.retry.is_some()),
            retry_after_ms: f.retry.unwrap_or(0),
            ..Self::default()
        };
        copy_text(&f.message, &mut e.message);
        copy_text(&f.broker, &mut e.broker_code);
        e
    }
}
fn copy_text(s: &str, out: &mut [c_char]) {
    let mut n = s.len().min(out.len() - 1);
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    for (to, from) in out.iter_mut().zip(s.as_bytes()[..n].iter()) {
        *to = if *from == 0 {
            b'?' as c_char
        } else {
            *from as c_char
        };
    }
}
/// Caller supplies readable, properly aligned objects of this type for the duration of a call.
pub unsafe fn reference<'a, T>(p: *const T) -> Result<&'a T> {
    if p.is_null() {
        return Err(Failure::invalid("NULL pointer"));
    }
    // SAFETY: non-NULL checked; allocation/alignment/lifetime are the C caller's contract.
    Ok(unsafe { &*p })
}
pub unsafe fn output<T>(p: *mut T, value: T) -> Result<()> {
    if p.is_null() {
        return Err(Failure::invalid("NULL output"));
    }
    // SAFETY: caller provides writable, aligned output storage.
    unsafe { p.write(value) };
    Ok(())
}
pub unsafe fn data<'a>(p: *const u8, len: usize, max: usize) -> Result<&'a [u8]> {
    if len > max || len > isize::MAX as usize || (len != 0 && p.is_null()) {
        return Err(Failure::invalid("invalid buffer length/pointer"));
    }
    if len == 0 {
        return Ok(&[]);
    }
    // SAFETY: bounded length; readable allocation is required by the caller's contract.
    Ok(unsafe { slice::from_raw_parts(p, len) })
}
pub unsafe fn text<'a>(p: *const c_char, max: usize) -> Result<Option<&'a str>> {
    if p.is_null() {
        return Ok(None);
    }
    // SAFETY: caller supplies readable NUL-terminated storage.
    let bytes = unsafe { CStr::from_ptr(p) }.to_bytes();
    if bytes.len() > max {
        return Err(Failure::invalid("string too long"));
    }
    Ok(Some(
        std::str::from_utf8(bytes).map_err(|_| Failure::invalid("invalid UTF-8"))?,
    ))
}
pub unsafe fn required<'a>(p: *const c_char, max: usize) -> Result<&'a str> {
    // SAFETY: forwarded caller contract.
    let s = unsafe { text(p, max) }?
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Failure::invalid("missing string"))?;
    Ok(s)
}
pub unsafe fn from_event(value: Event) -> Result<crate::payload::Event> {
    use crate::payload::{Event, Parameter, ParameterValue as V};
    if value.parameter_count > 3 {
        return Err(Failure::invalid("at most three event parameters"));
    }
    let mut parameters = Vec::with_capacity(value.parameter_count as usize);
    for p in &value.parameters[..value.parameter_count as usize] {
        // SAFETY: the C caller initializes the union member selected by kind.
        let v = unsafe {
            match p.kind {
                1 => V::U32(p.value.u32_value),
                2 => V::U64(p.value.u64_value),
                3 => V::I32(p.value.i32_value),
                4 => V::I64(p.value.i64_value),
                5 => V::F32(p.value.f32_value),
                6 => V::F64(p.value.f64_value),
                7 => {
                    if p.value.bool_value > 1 {
                        return Err(Failure::invalid("bool must be 0/1"));
                    }
                    V::Bool(p.value.bool_value != 0)
                }
                _ => return Err(Failure::invalid("invalid parameter tag")),
            }
        };
        parameters.push(Parameter { value: Some(v) });
    }
    Ok(Event {
        catalog: value.catalog,
        catalog_version: value.catalog_version,
        category: value.category,
        action: value.action,
        status: value.status,
        parameters,
    })
}
pub fn event_view(value: &crate::payload::Event) -> Event {
    use crate::payload::ParameterValue as V;
    let mut out = Event {
        catalog: value.catalog,
        catalog_version: value.catalog_version,
        category: value.category,
        action: value.action,
        status: value.status,
        parameter_count: value.parameters.len() as u32,
        ..Event::default()
    };
    for (to, p) in out.parameters.iter_mut().zip(&value.parameters) {
        *to = match p.value {
            Some(V::U32(v)) => Parameter {
                kind: 1,
                value: ParameterValue { u32_value: v },
            },
            Some(V::U64(v)) => Parameter {
                kind: 2,
                value: ParameterValue { u64_value: v },
            },
            Some(V::I32(v)) => Parameter {
                kind: 3,
                value: ParameterValue { i32_value: v },
            },
            Some(V::I64(v)) => Parameter {
                kind: 4,
                value: ParameterValue { i64_value: v },
            },
            Some(V::F32(v)) => Parameter {
                kind: 5,
                value: ParameterValue { f32_value: v },
            },
            Some(V::F64(v)) => Parameter {
                kind: 6,
                value: ParameterValue { f64_value: v },
            },
            Some(V::Bool(v)) => Parameter {
                kind: 7,
                value: ParameterValue {
                    bool_value: u32::from(v),
                },
            },
            None => Parameter::default(),
        };
    }
    out
}
