//! Incremental, cancellable input for the existing Chat SSE frame reader.
use bytes::Bytes;
use std::io::{self, Cursor, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::Arc;
use std::time::Duration;

use crate::gateway::upstream::{GatewayByteStream, GatewayByteStreamItem};

const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(100);

pub(super) struct ChatByteStreamReader {
    upstream: GatewayByteStream,
    buffered: Cursor<Bytes>,
    shutdown: Arc<AtomicBool>,
}

impl ChatByteStreamReader {
    pub(super) fn new(upstream: GatewayByteStream) -> (Self, Arc<AtomicBool>) {
        let shutdown = Arc::new(AtomicBool::new(false));
        (
            Self {
                upstream,
                buffered: Cursor::new(Bytes::new()),
                shutdown: Arc::clone(&shutdown),
            },
            shutdown,
        )
    }
}

impl Read for ChatByteStreamReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            if self.shutdown.load(Ordering::Acquire) {
                // The pump exits and drops GatewayByteStream, which signals
                // cancellation to its async HTTP transport even while idle.
                return Ok(0);
            }
            let read = self.buffered.read(buf)?;
            if read > 0 {
                return Ok(read);
            }
            match self.upstream.recv_timeout(CANCEL_POLL_INTERVAL) {
                Ok(GatewayByteStreamItem::Chunk(bytes)) => self.buffered = Cursor::new(bytes),
                Ok(GatewayByteStreamItem::Eof) => return Ok(0),
                Ok(GatewayByteStreamItem::Error(error)) => return Err(io::Error::other(error)),
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "upstream stream disconnected"));
                }
            }
        }
    }
}
