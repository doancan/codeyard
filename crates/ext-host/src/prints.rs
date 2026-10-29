//! The guest's stdout and stderr (a TypeScript `console.log`, a Rust `println!`): collected during a call and sent to
//! the extension's log when it returns. Past `PRINT_BYTES` per call the rest is dropped, so printing never fails or
//! traps the guest.

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use bytes::Bytes;
use wasmtime_wasi::cli::{IsTerminal, StdoutStream};
use wasmtime_wasi::p2::{OutputStream, Pollable, StreamResult};

const PRINT_BYTES: usize = 64 * 1024;

#[derive(Clone, Default)]
pub struct Prints(Arc<Mutex<Vec<u8>>>);

impl Prints {
    pub fn take(&self) -> Vec<u8> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(|p| p.into_inner()))
    }

    fn push(&self, bytes: &[u8]) {
        let mut buf = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let room = PRINT_BYTES.saturating_sub(buf.len());
        buf.extend_from_slice(&bytes[..bytes.len().min(room)]);
    }
}

impl IsTerminal for Prints {
    fn is_terminal(&self) -> bool {
        false
    }
}

impl StdoutStream for Prints {
    fn p2_stream(&self) -> Box<dyn OutputStream> {
        Box::new(self.clone())
    }

    fn async_stream(&self) -> Box<dyn tokio::io::AsyncWrite + Send + Sync> {
        Box::new(self.clone())
    }
}

#[async_trait::async_trait]
impl Pollable for Prints {
    async fn ready(&mut self) {}
}

impl OutputStream for Prints {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        self.push(&bytes);
        Ok(())
    }

    fn flush(&mut self) -> StreamResult<()> {
        Ok(())
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        Ok(PRINT_BYTES)
    }
}

impl tokio::io::AsyncWrite for Prints {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        self.push(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
