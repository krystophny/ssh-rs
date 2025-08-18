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
                    println!("Block None");
                    continue;
                }
                BlockDirections::Inbound => {
                    println!("Block inbound");
                    // Session needs to read data (could be for any channel)
                    self.readable().await?
                }
                BlockDirections::Outbound => {
                    // Session needs to write data (could be for any channel)
                    println!("Block outbound");
                    self.writable().await?
                }
                BlockDirections::Both => {
                    println!("Block Both");
                    // Session needs both read and write
                    self.ready(tokio::io::Interest::READABLE | tokio::io::Interest::WRITABLE)
                        .await?;
                }
            }

            if let Some(dur) = sleep_dur {
                sleep_async_fn(dur).await;
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
                println!("poll Block None");
                // No I/O needed - schedule a waker to retry later
                let waker = cx.waker().clone();
                let dur = sleep_dur.unwrap_or(Duration::from_millis(1));
                tokio::spawn(async move {
                    sleep_async_fn(dur).await;
                    waker.wake();
                });
                return Poll::Pending;
            }
            BlockDirections::Inbound => {
                println!("poll Block Inbound");
                // Session needs to read data (could be for any channel)
                ready!(self.poll_read_ready(cx))?;
            }
            BlockDirections::Outbound => {
                println!("poll Block Outbound");
                // Session needs to write data (could be for any channel)
                ready!(self.poll_write_ready(cx))?;
            }
            BlockDirections::Both => {
                println!("poll Block both");
                // Session needs both read and write
                ready!(self.poll_write_ready(cx))?;
                ready!(self.poll_read_ready(cx))?;
            }
        }

        // The socket readiness checks above (poll_read_ready/poll_write_ready) already
        // registered the waker with the reactor, so we just return Pending
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
                BlockDirections::None => continue,
                BlockDirections::Inbound => {
                    // Session needs to read data (could be for any channel)
                    self.readable().await?
                }
                BlockDirections::Outbound => {
                    // Session needs to write data (could be for any channel)
                    self.writable().await?
                }
                BlockDirections::Both => {
                    // Session needs both read and write
                    self.ready(tokio::io::Interest::READABLE | tokio::io::Interest::WRITABLE)
                        .await?;
                }
            }

            if let Some(dur) = sleep_dur {
                sleep_async_fn(dur).await;
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
                // No I/O needed - schedule a waker to retry later
                let waker = cx.waker().clone();
                let dur = sleep_dur.unwrap_or(Duration::from_millis(1));
                tokio::spawn(async move {
                    sleep_async_fn(dur).await;
                    waker.wake();
                });
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
                ready!(self.poll_write_ready(cx))?;
                ready!(self.poll_read_ready(cx))?;
            }
        }

        // The socket readiness checks above (poll_read_ready/poll_write_ready) already
        // registered the waker with the reactor, so we just return Pending
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
