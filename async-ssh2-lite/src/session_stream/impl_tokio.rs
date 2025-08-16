use core::{
    task::{Context, Poll},
    time::Duration,
};
use std::io::{Error as IoError, ErrorKind as IoErrorKind};

use async_trait::async_trait;
use futures_util::ready;
use ssh2::{BlockDirections, Error as Ssh2Error, Session};
use tokio::net::TcpStream;
#[cfg(unix)]
use tokio::net::UnixStream;

use super::AsyncSessionStream;
use crate::{error::Error, util::ssh2_error_is_would_block};

//
#[async_trait]
impl AsyncSessionStream for TcpStream {
    async fn x_with<R>(
        &self,
        mut op: impl FnMut() -> Result<R, Ssh2Error> + Send,
        sess: &Session,
        _expected_block_directions: BlockDirections,
        sleep_dur: Option<Duration>,
    ) -> Result<R, Error> {
        loop {
            match op() {
                Ok(x) => return Ok(x),
                Err(err) => {
                    if !ssh2_error_is_would_block(&err) {
                        return Err(err.into());
                    }
                }
            }

            // Wait for whatever I/O the session needs, not what we expect.
            // The session knows the aggregate state of all channels.
            match sess.block_directions() {
                BlockDirections::None => {
                    // No I/O needed right now. Sleep if requested.
                    if let Some(dur) = sleep_dur {
                        sleep_async_fn(dur).await;
                    } else {
                        // Without a sleep duration, yield to avoid busy loop
                        tokio::task::yield_now().await;
                    }
                    continue;
                }
                BlockDirections::Inbound => {
                    // Session needs to read data (could be for any channel)
                    self.readable().await?
                }
                BlockDirections::Outbound => {
                    // Session needs to write data (could be for any channel)
                    self.writable().await?
                }
                BlockDirections::Both => {
                    // Session needs both read and write, but we should wait for either
                    // rather than both simultaneously to avoid deadlock
                    tokio::select! {
                        res = self.readable() => res?,
                        res = self.writable() => res?,
                    }
                }
            }

        }
    }

    fn poll_x_with<R>(
        &self,
        cx: &mut Context,
        mut op: impl FnMut() -> Result<R, IoError> + Send,
        sess: &Session,
        _expected_block_directions: BlockDirections,
        sleep_dur: Option<Duration>,
    ) -> Poll<Result<R, IoError>> {
        match op() {
            Err(err) if err.kind() == IoErrorKind::WouldBlock => {}
            ret => return Poll::Ready(ret),
        }

        // Wait for whatever I/O the session needs, not what we expect.
        // The session knows the aggregate state of all channels.
        match sess.block_directions() {
            BlockDirections::None => {
                // No I/O needed right now. Schedule a wake-up if requested,
                // otherwise just return Pending without immediate wake.
                if let Some(dur) = sleep_dur {
                    let waker = cx.waker().clone();
                    tokio::spawn(async move {
                        sleep_async_fn(dur).await;
                        waker.wake();
                    });
                }
                return Poll::Pending;
            }
            BlockDirections::Inbound => {
                // Session needs to read data (could be for any channel)
                ready!(self.poll_read_ready(cx))?;
            }
            BlockDirections::Outbound => {
                // Session needs to write data (could be for any channel)  
                ready!(self.poll_write_ready(cx))?;
            }
            BlockDirections::Both => {
                // Session needs both read and write, check if either is ready
                // This prevents deadlock when only one direction is available
                if self.poll_write_ready(cx).is_ready() {
                    ready!(self.poll_write_ready(cx))?;
                } else if self.poll_read_ready(cx).is_ready() {
                    ready!(self.poll_read_ready(cx))?;
                } else {
                    // Neither is ready, register for both and return Pending
                    let _ = self.poll_write_ready(cx);
                    let _ = self.poll_read_ready(cx);
                }
            }
        }

        // The I/O readiness is registered, return Pending.
        // Tokio reactor will wake us when the socket is ready.
        Poll::Pending
    }
}

#[cfg(unix)]
#[async_trait]
impl AsyncSessionStream for UnixStream {
    async fn x_with<R>(
        &self,
        mut op: impl FnMut() -> Result<R, Ssh2Error> + Send,
        sess: &Session,
        _expected_block_directions: BlockDirections,
        sleep_dur: Option<Duration>,
    ) -> Result<R, Error> {
        loop {
            match op() {
                Ok(x) => return Ok(x),
                Err(err) => {
                    if !ssh2_error_is_would_block(&err) {
                        return Err(err.into());
                    }
                }
            }

            // Wait for whatever I/O the session needs, not what we expect.
            // The session knows the aggregate state of all channels.
            match sess.block_directions() {
                BlockDirections::None => {
                    // No I/O needed right now. Sleep if requested.
                    if let Some(dur) = sleep_dur {
                        sleep_async_fn(dur).await;
                    } else {
                        // Without a sleep duration, yield to avoid busy loop
                        tokio::task::yield_now().await;
                    }
                    continue;
                }
                BlockDirections::Inbound => {
                    // Session needs to read data (could be for any channel)
                    self.readable().await?
                }
                BlockDirections::Outbound => {
                    // Session needs to write data (could be for any channel)
                    self.writable().await?
                }
                BlockDirections::Both => {
                    // Session needs both read and write, but we should wait for either
                    // rather than both simultaneously to avoid deadlock
                    tokio::select! {
                        res = self.readable() => res?,
                        res = self.writable() => res?,
                    }
                }
            }

        }
    }

    fn poll_x_with<R>(
        &self,
        cx: &mut Context,
        mut op: impl FnMut() -> Result<R, IoError> + Send,
        sess: &Session,
        _expected_block_directions: BlockDirections,
        sleep_dur: Option<Duration>,
    ) -> Poll<Result<R, IoError>> {
        match op() {
            Err(err) if err.kind() == IoErrorKind::WouldBlock => {}
            ret => return Poll::Ready(ret),
        }

        // Wait for whatever I/O the session needs, not what we expect.
        // The session knows the aggregate state of all channels.
        match sess.block_directions() {
            BlockDirections::None => {
                // No I/O needed right now. Schedule a wake-up if requested,
                // otherwise just return Pending without immediate wake.
                if let Some(dur) = sleep_dur {
                    let waker = cx.waker().clone();
                    tokio::spawn(async move {
                        sleep_async_fn(dur).await;
                        waker.wake();
                    });
                }
                return Poll::Pending;
            }
            BlockDirections::Inbound => {
                // Session needs to read data (could be for any channel)
                ready!(self.poll_read_ready(cx))?;
            }
            BlockDirections::Outbound => {
                // Session needs to write data (could be for any channel)  
                ready!(self.poll_write_ready(cx))?;
            }
            BlockDirections::Both => {
                // Session needs both read and write, check if either is ready
                // This prevents deadlock when only one direction is available
                if self.poll_write_ready(cx).is_ready() {
                    ready!(self.poll_write_ready(cx))?;
                } else if self.poll_read_ready(cx).is_ready() {
                    ready!(self.poll_read_ready(cx))?;
                } else {
                    // Neither is ready, register for both and return Pending
                    let _ = self.poll_write_ready(cx);
                    let _ = self.poll_read_ready(cx);
                }
            }
        }

        // The I/O readiness is registered, return Pending.
        // Tokio reactor will wake us when the socket is ready.
        Poll::Pending
    }
}

//
//
//
async fn sleep_async_fn(dur: Duration) {
    sleep(dur).await;
}

fn sleep(dur: Duration) -> tokio::time::Sleep {
    tokio::time::sleep(tokio::time::Duration::from_millis(dur.as_millis() as u64))
}
