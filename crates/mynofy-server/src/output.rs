//! Per-connection protobuf output storage. Sent Bytes retain ownership until the sink releases them.
use bytes::{Bytes, BytesMut};
use zeroize::Zeroize;

pub struct ResponseBuffer {
    bytes: BytesMut,
    retain: usize,
    sent_capacity: usize,
    sent_len: usize,
}
impl ResponseBuffer {
    pub fn new(max_response_bytes: usize) -> Self {
        Self {
            bytes: BytesMut::new(),
            retain: max_response_bytes.min(64 * 1024),
            sent_capacity: 0,
            sent_len: 0,
        }
    }
    pub fn without_retention() -> Self {
        Self::new(0)
    }
    pub fn prepare(&mut self) -> &mut BytesMut {
        self.bytes.as_mut().zeroize();
        self.bytes.clear();
        &mut self.bytes
    }
    pub fn take(&mut self) -> Bytes {
        self.sent_capacity = self.bytes.capacity();
        self.sent_len = self.bytes.len();
        self.bytes.split().freeze()
    }
    /// Call only after sending/flushing. Failed reclaim never overwrites outstanding data.
    pub fn sent(&mut self) {
        if self.sent_len != 0 && self.bytes.try_reclaim(self.sent_capacity) {
            self.bytes.resize(self.sent_len, 0);
            self.bytes.as_mut().zeroize();
            self.bytes.clear();
        }
        if self.sent_capacity > self.retain {
            self.bytes = BytesMut::new();
        }
        self.sent_capacity = 0;
        self.sent_len = 0;
    }
}
impl Drop for ResponseBuffer {
    fn drop(&mut self) {
        self.sent();
        self.bytes.as_mut().zeroize();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reuses_released_storage_and_keeps_outstanding_frames_intact() {
        let mut buffer = ResponseBuffer::new(65536);
        buffer.prepare().extend_from_slice(&[1; 1024]);
        let bytes = buffer.take();
        let pointer = bytes.as_ptr();
        drop(bytes);
        buffer.sent();
        buffer.prepare().extend_from_slice(&[2; 1024]);
        let pending = buffer.take();
        assert_eq!(pending.as_ptr(), pointer);
        buffer.sent(); // A sink may still retain a reference: safe even in this case.
        buffer.prepare().extend_from_slice(&[3; 1024]);
        assert_eq!(pending.as_ref(), &[2; 1024]);
    }
    #[test]
    fn drops_large_peak_capacity_after_send() {
        let mut buffer = ResponseBuffer::new(1024 * 1024);
        buffer.prepare().resize(256 * 1024, 0);
        drop(buffer.take());
        buffer.sent();
        assert_eq!(buffer.bytes.capacity(), 0);
    }
}
