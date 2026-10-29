//! Request and response bodies, streamed both ways and counted against the byte budget.

use std::pin::Pin;
use std::task::{Context, Poll};

use http_body::{Frame, SizeHint};
use http_body_util::BodyExt as _;
use hyper::body::{Bytes, Incoming};
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::{Cancel, Error, guard};

/// What the extension sends.
#[derive(Debug, Default)]
pub enum Body {
    #[default]
    Empty,
    /// Bytes in hand. Sent with a `content-length`, and sent again when a 307 or 308 redirect asks for it.
    Full(Bytes),
    /// Chunks as the guest produces them; the body ends when the sender is dropped. A streamed body cannot be sent
    /// twice, so a 307 or 308 redirect after one is an error.
    Stream(mpsc::Receiver<Bytes>),
}

/// The request body as hyper polls it. A stream that goes over the budget ends the request with [`TooLarge`].
pub(crate) enum Outgoing {
    Empty,
    Full(Option<Bytes>),
    Stream { rx: mpsc::Receiver<Bytes>, sent: u64, limit: u64 },
}

/// The error a streamed request body ends with when it goes over the budget.
#[derive(Debug)]
pub(crate) struct TooLarge(pub u64);

impl std::fmt::Display for TooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the request body is larger than {} bytes", self.0)
    }
}

impl std::error::Error for TooLarge {}

impl http_body::Body for Outgoing {
    type Data = Bytes;
    type Error = TooLarge;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, TooLarge>>> {
        match self.get_mut() {
            Outgoing::Empty => Poll::Ready(None),
            Outgoing::Full(b) => Poll::Ready(b.take().map(|b| Ok(Frame::data(b)))),
            Outgoing::Stream { rx, sent, limit } => match rx.poll_recv(cx) {
                Poll::Ready(Some(chunk)) => {
                    *sent += chunk.len() as u64;
                    Poll::Ready(Some(if *sent > *limit { Err(TooLarge(*limit)) } else { Ok(Frame::data(chunk)) }))
                }
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Pending => Poll::Pending,
            },
        }
    }

    fn is_end_stream(&self) -> bool {
        match self {
            Outgoing::Empty => true,
            Outgoing::Full(b) => b.is_none(),
            Outgoing::Stream { .. } => false,
        }
    }

    fn size_hint(&self) -> SizeHint {
        match self {
            Outgoing::Empty => SizeHint::with_exact(0),
            Outgoing::Full(b) => SizeHint::with_exact(b.as_ref().map_or(0, |b| b.len() as u64)),
            Outgoing::Stream { .. } => SizeHint::default(),
        }
    }
}

/// What the server sends back, read chunk by chunk. Reading is bound by the request's deadline, its [`Cancel`] and
/// the response byte budget. Dropping it closes the connection.
pub struct ResponseBody {
    pub(crate) incoming: Incoming,
    pub(crate) received: u64,
    pub(crate) limit: u64,
    pub(crate) deadline: Instant,
    pub(crate) cancel: Cancel,
    pub(crate) url: String,
}

impl ResponseBody {
    /// The next chunk, or `None` at the end. Trailers are skipped.
    pub async fn chunk(&mut self) -> Result<Option<Bytes>, Error> {
        loop {
            let url = &self.url;
            let frame = guard(
                async { self.incoming.frame().await.transpose().map_err(|e| Error::Other(format!("{url}: the response broke off ({e})"))) },
                self.deadline,
                &self.cancel,
                url,
            )
            .await?;
            let Some(frame) = frame else { return Ok(None) };
            let Ok(data) = frame.into_data() else { continue };
            self.received += data.len() as u64;
            if self.received > self.limit {
                return Err(Error::Other(format!(
                    "{}: the response is larger than {} bytes, the limit for one request; ask the server for less, for \
                     example one page at a time",
                    self.url, self.limit
                )));
            }
            return Ok(Some(data));
        }
    }

    /// The whole body.
    pub async fn bytes(mut self) -> Result<Vec<u8>, Error> {
        let mut out = Vec::new();
        while let Some(chunk) = self.chunk().await? {
            out.extend_from_slice(&chunk);
        }
        Ok(out)
    }
}

impl std::fmt::Debug for ResponseBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResponseBody").field("url", &self.url).field("received", &self.received).finish_non_exhaustive()
    }
}
