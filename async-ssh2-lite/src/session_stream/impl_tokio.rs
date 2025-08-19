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
            let dirs = sess.block_directions();
            match dirs {
                BlockDirections::None => {
                    println!("Block None - operation blocks but no I/O needed");
                    // This should be rare - operation blocks but libssh2 says no I/O needed
                    // Use a longer delay to avoid spinning, then retry
                    sleep_async_fn(Duration::from_millis(10)).await;
                }
                BlockDirections::Inbound => {
                    println!("Block inbound - waiting for readable");
                    // Session needs to read data (could be for any channel)
                    self.readable().await?;
                    println!("Socket is now readable");
                    
                    // After socket is readable, immediately retry the operation
                    // This is critical for libssh2 to make progress
                    match op() {
                        Ok(x) => {
                            println!("Operation succeeded after readable!");
                            return Ok(x);
                        }
                        Err(err) if !ssh2_error_is_would_block(&err) => {
                            return Err(err.into());
                        }
                        _ => {
                            println!("Still blocks after readable, continuing loop");
                        }
                    }
                }
                BlockDirections::Outbound => {
                    // Session needs to write data (could be for any channel)
                    println!("Block outbound - waiting for writable");
                    self.writable().await?;
                    println!("Socket is now writable");
                    
                    // After socket is writable, immediately retry
                    match op() {
                        Ok(x) => {
                            println!("Operation succeeded after writable!");
                            return Ok(x);
                        }
                        Err(err) if !ssh2_error_is_would_block(&err) => {
                            return Err(err.into());
                        }
                        _ => {
                            println!("Still blocks after writable, continuing loop");
                        }
                    }
                }
                BlockDirections::Both => {
                    println!("Block Both - waiting for read+write");
                    // Session needs both read and write
                    self.ready(tokio::io::Interest::READABLE | tokio::io::Interest::WRITABLE)
                        .await?;
                    println!("Socket is now ready for both");
                    
                    // After socket is ready, immediately retry
                    match op() {
                        Ok(x) => {
                            println!("Operation succeeded after ready!");
                            return Ok(x);
                        }
                        Err(err) if !ssh2_error_is_would_block(&err) => {
                            return Err(err.into());
                        }
                        _ => {
                            println!("Still blocks after ready, continuing loop");
                        }
                    }
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
        // Try the operation first
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

        // Socket is now ready, keep trying the operation until it succeeds
        // or the socket is no longer ready
        loop {
            match op() {
                Err(err) if err.kind() == IoErrorKind::WouldBlock => {
                    // Check if we still need the same I/O
                    let current_dirs = sess.block_directions();
                    println!("After ready, op still blocks. Current directions: {:?}", current_dirs);
                    
                    // If directions became None, we need to re-evaluate  
                    if current_dirs == BlockDirections::None {
                        println!("Block directions changed to None");
                        // Schedule immediate wake to re-evaluate
                        let waker = cx.waker().clone();
                        tokio::spawn(async move {
                            tokio::task::yield_now().await;
                            waker.wake();
                        });
                        return Poll::Pending;
                    }
                    
                    // Socket was marked ready but operation still blocks
                    // This can happen when:
                    // 1. libssh2's internal buffers need processing
                    // 2. The socket readiness was spurious
                    // Try the operation one more time immediately
                    println!("Socket ready but op blocks, retrying once for {:?}", current_dirs);
                    
                    match op() {
                        Err(err) if err.kind() == IoErrorKind::WouldBlock => {
                            // Still blocks after retry - yield briefly to let libssh2 process
                            println!("Still blocks after retry, yielding");
                            let waker = cx.waker().clone();
                            tokio::spawn(async move {
                                tokio::task::yield_now().await;
                                waker.wake();
                            });
                            return Poll::Pending;
                        }
                        ret => return Poll::Ready(ret),
                    }
                }
                ret => return Poll::Ready(ret),
            }
        }
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
                    println!("UnixStream Block None - operation blocks but no I/O needed");
                    // This should be rare - use a longer delay to avoid spinning
                    sleep_async_fn(Duration::from_millis(10)).await;
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
        // Try the operation first
        match op() {
            Err(err) if err.kind() == IoErrorKind::WouldBlock => {}
            ret => return Poll::Ready(ret),
        }

        // Wait for whatever I/O the session needs, not what we expect.
        // The session knows the aggregate state of all channels.
        match sess.block_directions() {
            BlockDirections::None => {
                println!("poll Block None - scheduling longer delay");
                // No I/O needed - use a longer delay to avoid spinning
                let waker = cx.waker().clone();
                let dur = Duration::from_millis(50); // Much longer delay for None case
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

        // Socket is now ready, keep trying the operation
        loop {
            match op() {
                Err(err) if err.kind() == IoErrorKind::WouldBlock => {
                    // Check if we still need I/O
                    let current_dirs = sess.block_directions();
                    
                    // If directions became None, we need to re-evaluate  
                    if current_dirs == BlockDirections::None {
                        // Schedule immediate wake to re-evaluate
                        let waker = cx.waker().clone();
                        tokio::spawn(async move {
                            tokio::task::yield_now().await;
                            waker.wake();
                        });
                        return Poll::Pending;
                    }
                    
                    // Socket was marked ready but operation still blocks
                    // Try the operation one more time immediately
                    match op() {
                        Err(err) if err.kind() == IoErrorKind::WouldBlock => {
                            // Still blocks after retry - yield briefly to let libssh2 process
                            let waker = cx.waker().clone();
                            tokio::spawn(async move {
                                tokio::task::yield_now().await;
                                waker.wake();
                            });
                            return Poll::Pending;
                        }
                        ret => return Poll::Ready(ret),
                    }
                }
                ret => return Poll::Ready(ret),
            }
        }
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
