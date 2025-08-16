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
                    // Session needs both read and write capability
                    // Use a timeout to prevent indefinite blocking
                    let ready_result = tokio::time::timeout(
                        std::time::Duration::from_millis(100),
                        self.ready(tokio::io::Interest::READABLE | tokio::io::Interest::WRITABLE)
                    ).await;
                    
                    match ready_result {
                        Ok(Ok(_)) => {}, // Both ready
                        Ok(Err(e)) => return Err(e),
                        Err(_) => {
                            // Timeout - try either readable or writable
                            tokio::select! {
                                res = self.readable() => res?,
                                res = self.writable() => res?,
                            }
                        }
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
                // Session needs both read and write
                // Try to get both ready, but fall back to either if needed
                match (self.poll_write_ready(cx), self.poll_read_ready(cx)) {
                    (Poll::Ready(Ok(_)), Poll::Ready(Ok(_))) => {
                        // Both ready - great!
                    }
                    (Poll::Ready(Ok(_)), _) => {
                        // Write ready - use it
                        ready!(self.poll_write_ready(cx))?;
                    }
                    (_, Poll::Ready(Ok(_))) => {
                        // Read ready - use it
                        ready!(self.poll_read_ready(cx))?;
                    }
                    _ => {
                        // Neither ready yet, register for both
                        let _ = self.poll_write_ready(cx);
                        let _ = self.poll_read_ready(cx);
                    }
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
                    // Session needs both read and write capability
                    // Use a timeout to prevent indefinite blocking
                    let ready_result = tokio::time::timeout(
                        std::time::Duration::from_millis(100),
                        self.ready(tokio::io::Interest::READABLE | tokio::io::Interest::WRITABLE)
                    ).await;
                    
                    match ready_result {
                        Ok(Ok(_)) => {}, // Both ready
                        Ok(Err(e)) => return Err(e),
                        Err(_) => {
                            // Timeout - try either readable or writable
                            tokio::select! {
                                res = self.readable() => res?,
                                res = self.writable() => res?,
                            }
                        }
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
                // Session needs both read and write
                // Try to get both ready, but fall back to either if needed
                match (self.poll_write_ready(cx), self.poll_read_ready(cx)) {
                    (Poll::Ready(Ok(_)), Poll::Ready(Ok(_))) => {
                        // Both ready - great!
                    }
                    (Poll::Ready(Ok(_)), _) => {
                        // Write ready - use it
                        ready!(self.poll_write_ready(cx))?;
                    }
                    (_, Poll::Ready(Ok(_))) => {
                        // Read ready - use it
                        ready!(self.poll_read_ready(cx))?;
                    }
                    _ => {
                        // Neither ready yet, register for both
                        let _ = self.poll_write_ready(cx);
                        let _ = self.poll_read_ready(cx);
                    }
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
