use firmware_controller::controller;
use futures::StreamExt;

#[derive(Debug, PartialEq, Copy, Clone)]
pub enum State {
    Idle,
    Active,
    Error,
}

#[derive(Debug, PartialEq, Copy, Clone)]
pub enum Mode {
    Normal,
    Debug,
}

#[derive(Debug, PartialEq)]
pub enum TestError {
    InvalidState,
    OperationFailed,
}

#[controller]
mod test_controller {
    use super::*;

    pub struct Controller {
        #[controller(publish, getter = "get_current_state", setter = "change_state")]
        state: State,
        #[controller(publish, getter, setter)]
        mode: Mode,
        #[controller(setter)]
        counter: u32,
    }

    impl Controller {
        #[controller(signal)]
        pub async fn error_occurred(&self, code: u32, message: heapless::String<32>);

        #[controller(signal)]
        pub async fn operation_complete(&self);

        pub async fn increment(&mut self) -> u32 {
            self.counter += 1;
            self.counter
        }

        pub async fn get_counter(&self) -> u32 {
            self.counter
        }

        pub async fn activate(&mut self) -> Result<(), TestError> {
            if self.state != State::Idle {
                return Err(TestError::InvalidState);
            }
            self.set_state(State::Active).await;
            self.operation_complete().await;
            Ok(())
        }

        pub async fn trigger_error(&mut self) -> Result<(), TestError> {
            self.set_state(State::Error).await;
            self.error_occurred(42, "Test error".try_into().unwrap())
                .await;
            Err(TestError::OperationFailed)
        }

        pub async fn return_nothing(&self) {}
    }
}

use test_controller::*;

#[cfg(feature = "embassy")]
#[test]
fn test_controller_basic_functionality() {
    let controller = Controller::new(State::Idle, Mode::Normal, 0).unwrap();

    std::thread::spawn(move || {
        let executor = Box::leak(Box::new(embassy_executor::Executor::new()));
        executor.run(move |spawner| {
            spawner.spawn(controller_embassy_task(controller)).unwrap();
        });
    });

    futures::executor::block_on(async {
        run_basic_test().await;
    });
}

#[cfg(feature = "tokio")]
#[tokio::test]
async fn test_controller_basic_functionality() {
    let controller = Controller::new(State::Idle, Mode::Normal, 0).unwrap();
    tokio::spawn(controller_task(controller));
    tokio::task::yield_now().await;

    run_basic_test().await;
}

async fn run_basic_test() {
    let mut client = ControllerClient::new();

    // Test 1: Subscribe to state changes.
    let mut state_stream = client.receive_state_changed().expect("Failed to subscribe");

    // Test 1a: First poll returns the initial (current) value.
    let initial_state = state_stream
        .next()
        .await
        .expect("Should receive initial state");
    assert_eq!(initial_state, State::Idle, "Initial state should be Idle");

    // Test 2: Subscribe to signals.
    let mut error_stream = client
        .receive_error_occurred()
        .expect("Failed to subscribe to error");
    let mut complete_stream = client
        .receive_operation_complete()
        .expect("Failed to subscribe to complete");

    // Test 3: Call a method and verify return value.
    let counter = client.get_counter().await;
    assert_eq!(counter, 0, "Initial counter should be 0");

    // Test 4: Call increment and verify it increases.
    let counter = client.increment().await;
    assert_eq!(counter, 1, "Counter should be 1 after increment");

    let counter = client.increment().await;
    assert_eq!(counter, 2, "Counter should be 2 after second increment");

    // Test 5: Call method that changes state and emits signal.
    let activate_result = client.activate().await;
    assert!(
        activate_result.is_ok(),
        "Activate should succeed from Idle state"
    );

    // Verify we received the state change.
    let new_state = state_stream
        .next()
        .await
        .expect("Should receive state change");
    assert_eq!(new_state, State::Active, "New state should be Active");

    // Verify we received the operation_complete signal.
    let _complete = complete_stream
        .next()
        .await
        .expect("Should receive operation complete signal");

    // Test 6: Call method that returns error.
    let error_result = client.trigger_error().await;
    assert!(
        error_result.is_err(),
        "trigger_error should return an error"
    );
    assert_eq!(
        error_result.unwrap_err(),
        TestError::OperationFailed,
        "Should return OperationFailed error"
    );

    // Verify state changed to Error.
    let new_state = state_stream
        .next()
        .await
        .expect("Should receive state change");
    assert_eq!(new_state, State::Error, "New state should be Error");

    // Verify we received the error signal.
    let error_signal = error_stream
        .next()
        .await
        .expect("Should receive error signal");
    assert_eq!(error_signal.code, 42, "Error code should be 42");
    assert_eq!(
        error_signal.message.as_str(),
        "Test error",
        "Error message should match"
    );

    // Test 7: Try to activate again (should fail due to invalid state).
    let activate_result = client.activate().await;
    assert!(
        activate_result.is_err(),
        "Activate should fail from Error state"
    );
    assert_eq!(
        activate_result.unwrap_err(),
        TestError::InvalidState,
        "Should return InvalidState error"
    );

    // Test 8: Use setter to change mode.
    client.set_mode(Mode::Debug).await;

    // Test 9: Call method with no return value.
    client.return_nothing().await;

    // Test 10: Use getter with custom name to get state.
    let state = client.get_current_state().await;
    assert_eq!(state, State::Error, "State should be Error");

    // Test 11: Use getter with default field name to get mode.
    let mode = client.mode().await;
    assert_eq!(mode, Mode::Debug, "Mode should be Debug");

    // Test 12: Use setter with custom name (new syntax).
    client.change_state(State::Idle).await;
    let state = client.get_current_state().await;
    assert_eq!(
        state,
        State::Idle,
        "State should be Idle after change_state"
    );

    // Test 13: Use setter without publish (independent setter).
    client.set_counter(100).await;
    let counter = client.get_counter().await;
    assert_eq!(counter, 100, "Counter should be 100 after set_counter");
}

#[cfg(feature = "tokio")]
async fn controller_task(controller: Controller) {
    controller.run().await;
}

#[cfg(feature = "embassy")]
#[embassy_executor::task]
async fn controller_embassy_task(controller: Controller) {
    controller.run().await;
}

/// Test that visibility specifiers on struct fields are preserved.
#[controller]
mod visibility_test_controller {
    pub struct Controller {
        #[controller(getter)]
        pub public_field: u32,
        #[controller(getter)]
        pub(crate) crate_field: i32,
        #[controller(getter)]
        private_field: bool,
    }

    impl Controller {}
}

#[cfg(feature = "embassy")]
#[test]
fn test_visibility_on_fields() {
    let controller = visibility_test_controller::Controller::new(42, -1, true).unwrap();
    assert_eq!(controller.public_field, 42);
    assert_eq!(controller.crate_field, -1);

    std::thread::spawn(move || {
        let executor = Box::leak(Box::new(embassy_executor::Executor::new()));
        executor.run(move |spawner| {
            spawner
                .spawn(visibility_controller_task(controller))
                .unwrap();
        });
    });

    futures::executor::block_on(async {
        run_visibility_test().await;
    });
}

#[cfg(feature = "tokio")]
#[tokio::test]
async fn test_visibility_on_fields() {
    let controller = visibility_test_controller::Controller::new(42, -1, true).unwrap();
    assert_eq!(controller.public_field, 42);
    assert_eq!(controller.crate_field, -1);

    tokio::spawn(async move { controller.run().await });
    tokio::task::yield_now().await;

    run_visibility_test().await;
}

async fn run_visibility_test() {
    let client = visibility_test_controller::ControllerClient::new();
    assert_eq!(client.public_field().await, 42);
    assert_eq!(client.crate_field().await, -1);
    assert_eq!(client.private_field().await, true);
}

#[cfg(feature = "embassy")]
#[embassy_executor::task]
async fn visibility_controller_task(controller: visibility_test_controller::Controller) {
    controller.run().await;
}

use std::sync::atomic::{AtomicU32, Ordering};

static POLL_A_COUNT: AtomicU32 = AtomicU32::new(0);
static POLL_B_COUNT: AtomicU32 = AtomicU32::new(0);
static POLL_C_COUNT: AtomicU32 = AtomicU32::new(0);

/// Test poll methods with timeouts.
#[controller]
mod poll_test_controller {
    use super::*;

    pub struct Controller {
        #[controller(getter)]
        pub value: u32,
    }

    impl Controller {
        // Two methods with the same poll interval (50ms) - should be grouped.
        #[controller(poll_millis = 50)]
        pub async fn poll_a(&mut self) {
            POLL_A_COUNT.fetch_add(1, Ordering::SeqCst);
        }

        #[controller(poll_millis = 50)]
        pub async fn poll_b(&mut self) {
            POLL_B_COUNT.fetch_add(1, Ordering::SeqCst);
        }

        // Different poll interval (100ms).
        #[controller(poll_millis = 100)]
        pub async fn poll_c(&mut self) {
            POLL_C_COUNT.fetch_add(1, Ordering::SeqCst);
        }
    }
}

/// Test that poll methods are called at the expected intervals.
#[cfg(feature = "embassy")]
#[test]
fn poll_methods() {
    use embassy_time::{Duration, MockDriver};

    let driver = MockDriver::get();
    driver.reset();
    POLL_A_COUNT.store(0, Ordering::SeqCst);
    POLL_B_COUNT.store(0, Ordering::SeqCst);
    POLL_C_COUNT.store(0, Ordering::SeqCst);

    let controller = poll_test_controller::Controller::new(42).unwrap();
    assert_eq!(controller.value, 42);

    std::thread::spawn(move || {
        let executor = Box::leak(Box::new(embassy_executor::Executor::new()));
        executor.run(move |spawner| {
            spawner.spawn(poll_controller_task(controller)).unwrap();
        });
    });

    // Give the executor a moment to start.
    std::thread::sleep(std::time::Duration::from_millis(10));

    futures::executor::block_on(run_poll_test(|millis| async move {
        driver.advance(Duration::from_millis(millis));
        std::thread::sleep(std::time::Duration::from_millis(10));
    }));
}

#[cfg(feature = "tokio")]
#[tokio::test(start_paused = true)]
async fn poll_methods() {
    POLL_A_COUNT.store(0, Ordering::SeqCst);
    POLL_B_COUNT.store(0, Ordering::SeqCst);
    POLL_C_COUNT.store(0, Ordering::SeqCst);

    let controller = poll_test_controller::Controller::new(42).unwrap();
    assert_eq!(controller.value, 42);

    tokio::spawn(async move { controller.run().await });

    // Yield to let the controller task start and skip the initial ticks.
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }

    run_poll_test(|millis| async move {
        tokio::time::advance(std::time::Duration::from_millis(millis)).await;
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
    })
    .await;
}

async fn run_poll_test<F, Fut>(advance_and_settle: F)
where
    F: Fn(u64) -> Fut,
    Fut: core::future::Future<Output = ()>,
{
    // Advance 50ms - poll_a and poll_b should fire once.
    advance_and_settle(50).await;
    assert_poll_counts(1, 1, 0);

    // Advance another 50ms (total 100ms).
    advance_and_settle(50).await;
    assert_poll_counts(2, 2, 1);

    // Advance another 100ms (total 200ms).
    advance_and_settle(100).await;
    assert_poll_counts(4, 4, 2);
}

fn assert_poll_counts(a: u32, b: u32, c: u32) {
    assert_eq!(POLL_A_COUNT.load(Ordering::SeqCst), a);
    assert_eq!(POLL_B_COUNT.load(Ordering::SeqCst), b);
    assert_eq!(POLL_C_COUNT.load(Ordering::SeqCst), c);
}

#[cfg(feature = "embassy")]
#[embassy_executor::task]
async fn poll_controller_task(controller: poll_test_controller::Controller) {
    controller.run().await;
}
