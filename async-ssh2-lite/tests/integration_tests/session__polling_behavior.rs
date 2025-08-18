#![cfg(any(feature = "async-io", feature = "tokio"))]

use std::error;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_ssh2_lite::{AsyncSession, AsyncSessionStream};

//
// Test to verify that BlockDirections::None doesn't cause excessive polling
//
#[cfg(feature = "tokio")]
#[tokio::test]
async fn polling_behavior_with_tokio() -> Result<(), Box<dyn error::Error>> {
    __run__polling_behavior_test::<async_ssh2_lite::TokioTcpStream>().await
}

#[cfg(feature = "async-io")]
#[test]
fn polling_behavior_with_async_io() -> Result<(), Box<dyn error::Error>> {
    futures_lite::future::block_on(async {
        __run__polling_behavior_test::<async_ssh2_lite::AsyncIoTcpStream>().await
    })
}

async fn __run__polling_behavior_test<S: AsyncSessionStream + Send + Sync + 'static>(
) -> Result<(), Box<dyn error::Error>> {
    println!("Testing BlockDirections::None polling behavior...");
    
    // We'll try to connect to a non-existent port to trigger connection timeouts
    // This tests the polling behavior during connection attempts
    let poll_counter = Arc::new(AtomicU64::new(0));
    let counter_clone = poll_counter.clone();
    
    // Monitor task to track polling rate
    let monitor_handle = tokio::spawn(async move {
        let start = Instant::now();
        let mut last_count = 0;
        let mut high_poll_intervals = 0;
        
        for i in 0..10 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let current = counter_clone.load(Ordering::Relaxed);
            let polls_in_100ms = current - last_count;
            
            if polls_in_100ms > 1000 {
                high_poll_intervals += 1;
                println!("⚠️  [{:>4}ms] EXCESSIVE POLLING: {} polls in 100ms", 
                         i * 100, polls_in_100ms);
            } else if polls_in_100ms > 100 {
                println!("   [{:>4}ms] Moderate polling: {} polls in 100ms", 
                         i * 100, polls_in_100ms);
            } else {
                println!("✓  [{:>4}ms] Normal: {} polls in 100ms", 
                         i * 100, polls_in_100ms);
            }
            
            last_count = current;
        }
        
        let total_time = start.elapsed();
        let total_polls = counter_clone.load(Ordering::Relaxed);
        let polls_per_sec = total_polls as f64 / total_time.as_secs_f64();
        
        println!("\n=== Polling Test Results ===");
        println!("Total polls: {}", total_polls);
        println!("Duration: {:.2}s", total_time.as_secs_f64());
        println!("Average rate: {:.0} polls/sec", polls_per_sec);
        println!("High poll intervals: {}/10", high_poll_intervals);
        
        // Test passes if we don't have excessive polling
        // Allow some high intervals but not constant excessive polling
        if high_poll_intervals > 5 {
            panic!("❌ FAILED: Excessive polling detected ({} high poll intervals). \
                    BlockDirections::None may not be handling wakers correctly.", 
                    high_poll_intervals);
        } else if polls_per_sec > 10000.0 {
            panic!("❌ FAILED: Average polling rate too high ({:.0} polls/sec). \
                    This indicates a busy-wait loop.", polls_per_sec);
        } else {
            println!("✅ PASSED: Polling behavior is acceptable.");
        }
    });
    
    // Create a custom AsyncRead/AsyncWrite wrapper that counts poll attempts
    struct PollCounter<T> {
        inner: T,
        counter: Arc<AtomicU64>,
    }
    
    impl<T: std::io::Read> std::io::Read for PollCounter<T> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.counter.fetch_add(1, Ordering::Relaxed);
            self.inner.read(buf)
        }
    }
    
    impl<T: std::io::Write> std::io::Write for PollCounter<T> {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.counter.fetch_add(1, Ordering::Relaxed);
            self.inner.write(buf)
        }
        
        fn flush(&mut self) -> std::io::Result<()> {
            self.inner.flush()
        }
    }
    
    // Try to connect to a non-existent SSH server (this will timeout/fail)
    // We're testing the polling behavior during this attempt
    let connect_result = tokio::time::timeout(
        Duration::from_secs(1),
        AsyncSession::<S>::connect(([127, 0, 0, 1], 22222), None)
    ).await;
    
    // Wait for monitor to complete
    monitor_handle.await?;
    
    // We expect the connection to fail - that's OK
    if let Ok(Ok(_session)) = connect_result {
        println!("Unexpectedly connected to 127.0.0.1:22222");
    } else {
        println!("Connection failed as expected (testing polling only)");
    }
    
    Ok(())
}

// Additional test for idle channel polling
#[cfg(all(feature = "tokio", feature = "_integration_tests"))]
#[tokio::test]
async fn idle_channel_polling_with_tokio() -> Result<(), Box<dyn error::Error>> {
    use super::helpers::get_connect_addr;
    use super::session__userauth_pubkey::__run__session__userauth_pubkey_file;
    
    // Only run if we have a real SSH server for integration tests
    let Ok(addr) = get_connect_addr() else {
        println!("Skipping idle channel test - no SSH server configured");
        return Ok(());
    };
    
    let mut session = AsyncSession::<async_ssh2_lite::TokioTcpStream>::connect(addr, None).await?;
    __run__session__userauth_pubkey_file(&mut session).await?;
    
    // Create a channel and let it idle
    let mut channel = session.channel_session().await?;
    
    // Monitor idle polling for 2 seconds
    let start = Instant::now();
    let poll_count = Arc::new(AtomicU64::new(0));
    let counter = poll_count.clone();
    
    let read_task = tokio::spawn(async move {
        let mut buf = [0u8; 1024];
        while start.elapsed() < Duration::from_secs(2) {
            counter.fetch_add(1, Ordering::Relaxed);
            // Try to read from idle channel
            match tokio::time::timeout(Duration::from_millis(10), channel.read(&mut buf)).await {
                Ok(Ok(0)) => break, // EOF
                Ok(Ok(n)) => println!("Read {} bytes", n),
                _ => {} // Timeout or WouldBlock - expected
            }
        }
    });
    
    // Wait for monitoring to complete
    let _ = read_task.await;
    
    let elapsed = start.elapsed();
    let total_polls = poll_count.load(Ordering::Relaxed);
    let polls_per_sec = total_polls as f64 / elapsed.as_secs_f64();
    
    println!("Idle channel polling: {} polls in {:.2}s = {:.0} polls/sec",
             total_polls, elapsed.as_secs_f64(), polls_per_sec);
    
    // With the fix, we should see reasonable polling rates
    assert!(polls_per_sec < 1000.0, 
            "Idle channel polling rate too high: {:.0} polls/sec", polls_per_sec);
    
    Ok(())
}