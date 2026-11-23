//! Bounded binary WebSocket byte stream for end-to-end TLS. No message interpretation.
use bytes::Bytes;
use futures_util::{Sink, Stream};
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};
pub const CHUNK: usize = 8192;
pub struct WebSocketIo<S> {
    socket: WebSocketStream<S>,
    pending: Bytes,
    flush: bool,
    eof: bool,
}
impl<S> WebSocketIo<S> {
    pub fn new(socket: WebSocketStream<S>) -> Self {
        Self {
            socket,
            pending: Bytes::new(),
            flush: false,
            eof: false,
        }
    }
}
fn error(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}
impl<S: AsyncRead + AsyncWrite + Unpin> AsyncRead for WebSocketIo<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if out.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        for _ in 0..32 {
            if !self.pending.is_empty() {
                let n = out.remaining().min(self.pending.len());
                out.put_slice(&self.pending.split_to(n));
                return Poll::Ready(Ok(()));
            }
            if self.eof {
                return Poll::Ready(Ok(()));
            }
            if self.flush {
                match Pin::new(&mut self.socket).poll_flush(cx) {
                    Poll::Ready(Ok(())) => self.flush = false,
                    Poll::Ready(Err(e)) => return Poll::Ready(Err(error(e))),
                    Poll::Pending => return Poll::Pending,
                }
            }
            match Pin::new(&mut self.socket).poll_next(cx) {
                Poll::Ready(Some(Ok(Message::Binary(bytes)))) if bytes.len() <= CHUNK => {
                    self.pending = bytes
                }
                Poll::Ready(Some(Ok(Message::Ping(_)))) => self.flush = true,
                Poll::Ready(Some(Ok(Message::Pong(_)))) => {}
                Poll::Ready(Some(Ok(Message::Close(_)))) | Poll::Ready(None) => self.eof = true,
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Err(error(e))),
                Poll::Ready(Some(Ok(_))) => return Poll::Ready(Err(error("Invalid tunnel frame"))),
                Poll::Pending => return Poll::Pending,
            }
        }
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}
impl<S: AsyncRead + AsyncWrite + Unpin> AsyncWrite for WebSocketIo<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        if bytes.is_empty() {
            return Poll::Ready(Ok(0));
        }
        match Pin::new(&mut self.socket).poll_ready(cx) {
            Poll::Ready(Ok(())) => {}
            Poll::Ready(Err(e)) => return Poll::Ready(Err(error(e))),
            Poll::Pending => return Poll::Pending,
        }
        let n = bytes.len().min(CHUNK);
        if let Err(e) = Pin::new(&mut self.socket)
            .start_send(Message::Binary(Bytes::copy_from_slice(&bytes[..n])))
        {
            return Poll::Ready(Err(error(e)));
        }
        Poll::Ready(Ok(n))
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.socket).poll_flush(cx).map_err(error)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.socket).poll_close(cx).map_err(error)
    }
}
