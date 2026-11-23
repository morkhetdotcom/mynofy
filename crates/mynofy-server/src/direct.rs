//! Encode-only helpers for the existing protobuf schema. Generated decoding remains authoritative.
use bytes::{BufMut, BytesMut};
use prost::encoding::{WireType, encode_key, encode_varint, encoded_len_varint, key_len};
pub(crate) fn field_len(tag: u32, len: usize) -> usize {
    key_len(tag) + encoded_len_varint(len as u64) + len
}
pub(crate) fn header(tag: u32, len: usize, out: &mut BytesMut) {
    encode_key(tag, WireType::LengthDelimited, out);
    encode_varint(len as u64, out);
}
pub(crate) fn data(tag: u32, value: &[u8], out: &mut BytesMut) {
    header(tag, value.len(), out);
    out.put_slice(value);
}
pub(crate) fn number(tag: u32, value: u64, out: &mut BytesMut) {
    encode_key(tag, WireType::Varint, out);
    encode_varint(value, out);
}
