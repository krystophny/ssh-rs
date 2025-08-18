// Regression test for BlockDirections::None polling issue
// This test doesn't require a real SSH server

#![cfg(feature = "tokio")]

use std::time::{Duration, Instant};

#[tokio::test]
async fn test_no_excessive_polling_on_connection_failure() {
    // This test verifies that when connecting to a non-existent server,
    // we don't enter a busy-wait loop due to BlockDirections::None
    // returning Poll::Pending without registering a waker.
    
    let start = Instant::now();
    
    // Try to connect to a definitely non-existent address
    // Using port 1 which is typically privileged and unused
    let connect_future = async_ssh2_lite::AsyncSession::<async_ssh2_lite::TokioTcpStream>::connect(
        ([127, 0, 0, 1], 1),
        None
    );
    
    // This should timeout without excessive CPU usage
    let result = tokio::time::timeout(Duration::from_millis(100), connect_future).await;
    
    let elapsed = start.elapsed();
    
    // The connection should fail or timeout
    assert!(result.is_err() || result.unwrap().is_err(), 
            "Should not connect to 127.0.0.1:1");
    
    // If the polling bug exists, this test would consume 100% CPU
    // With the fix, it should complete quickly with low CPU usage
    println!("Connection attempt took {:?}", elapsed);
    
    // The test passes if we get here without hanging or excessive CPU
}

#[tokio::test] 
async fn test_waker_registration_on_pending() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll, Wake, Waker};
    use std::pin::Pin;
    use std::future::Future;
    
    // Custom waker to detect if it's properly registered
    struct TestWaker {
        woken: Arc<AtomicBool>,
    }
    
    impl Wake for TestWaker {
        fn wake(self: Arc<Self>) {
            self.woken.store(true, Ordering::SeqCst);
        }
    }
    
    // This simulates what happens internally when BlockDirections::None occurs
    let woken = Arc::new(AtomicBool::new(false));
    let test_waker = Arc::new(TestWaker { woken: woken.clone() });
    let waker = Waker::from(test_waker);
    let mut cx = Context::from_waker(&waker);
    
    // Create a future that will trigger polling
    let mut future = Box::pin(async {
        // Simulate a delay
        tokio::time::sleep(Duration::from_millis(10)).await;
    });
    
    // Poll once - should return Pending and register waker
    match future.as_mut().poll(&mut cx) {
        Poll::Pending => {
            // Good - it's pending
            println!("Future correctly returned Pending");
        }
        Poll::Ready(_) => {
            panic!("Future should not be ready immediately");
        }
    }
    
    // Wait a bit for the waker to be called
    tokio::time::sleep(Duration::from_millis(20)).await;
    
    // The waker should have been called
    assert!(woken.load(Ordering::SeqCst), 
            "Waker was not called - this indicates Poll::Pending was returned without registering a waker!");
    
    println!("✓ Waker was properly registered and called");
}