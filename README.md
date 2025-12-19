<div align="center">

# Firmware Controller

This crate provides a macro named `controller` that makes it easy to decouple interactions between
components in a `no_std` environment.

[Intro](#intro) •
[Usage](#usage) •
[Details](#details)

</div>

# Intro

This crate provides a macro named `controller` that makes it easy to write controller logic for 
firmware.

The controller is responsible for control of all the peripherals based on commands it receives from
other parts of the code. It also notifies peers about state changes and events via signals.
This macro generates all the boilerplate code and client-side API for you.

# Usage

It's best described by an example so let's take example of a very simple firmware that controls an 
LED:

```rust,no_run
use firmware_controller::controller;

#[derive(Debug)]
pub enum MyFirmwareError {
  InvalidState,
}

#[derive(Debug, PartialEq, Copy, Clone)]
pub enum State {
    Enabled,
    Disabled,
}

#[controller]
mod controller {
    use super::*;

    // The controller struct. This is where you define the state of your firmware.
    pub struct Controller {
        #[controller(publish)]
        state: State,
        // Other fields. Note: Not all of them need to be published.
    }

    // The controller implementation. This is where you define the logic of your firmware.
    impl Controller {
        // The `signal` attribute marks this method signature (note: no implementation body) as a
        // signal, that you can use to notify other parts of your code about specific events.
        #[controller(signal)]
        pub async fn power_error(&self, description: heapless::String<64>);

        pub async fn enable_power(&mut self) -> Result<(), MyFirmwareError> {
            if self.state != State::Disabled {
                return Err(MyFirmwareError::InvalidState);
            }

            // Any other logic you want to run when enabling power.

            self.set_state(State::Enabled).await;
            self.power_error("Dummy error just for the showcase".try_into().unwrap())
                .await;

            Ok(())
        }

        pub async fn disable_power(&mut self) -> Result<(), MyFirmwareError> {
            if self.state != State::Enabled {
                return Err(MyFirmwareError::InvalidState);
            }

            // Any other logic you want to run when enabling power.

            self.set_state(State::Disabled).await;

            Ok(())
        }

        // Method that doesn't return anything.
        pub async fn return_nothing(&self) {
        }
    }
}

use controller::*;

#[embassy_executor::main]
async fn main(spawner: embassy_executor::Spawner) {
    let mut controller = Controller::new(State::Disabled);

    // Spawn the client task.
    spawner.spawn(client());

    // Run the controller logic.
    controller.run().await;
}

// This is just a very silly client that keeps flipping the power state every 1 second.
#[embassy_executor::task]
async fn client() {
    use futures::{future::Either, stream::select, StreamExt};
    use embassy_time::{Timer, Duration};

    let mut client = ControllerClient::new();
    let mut state_stream = client.receive_state_changed().unwrap();
    let error_stream = client.receive_power_error().unwrap().map(Either::Right);

    // First poll returns the current (initial) state.
    let initial_state = state_stream.next().await.unwrap();
    assert_eq!(initial_state, State::Disabled);

    // Now combine streams for event handling.
    let state_changed = state_stream.map(Either::Left);
    let mut stream = select(state_changed, error_stream);

    client.enable_power().await.unwrap();
    while let Some(event) = stream.next().await {
        match event {
            Either::Left(State::Enabled) => {
                // This is fine in this very simple example where we've only one client in a single
                // task. In a real-world application, you should ensure that the stream is polled
                // continuously. Otherwise, you might miss notifications.
                Timer::after(Duration::from_secs(1)).await;

                client.disable_power().await.unwrap();
            }
            Either::Left(State::Disabled) => {
                Timer::after(Duration::from_secs(1)).await;

                client.enable_power().await.unwrap();
            }
            Either::Right(ControllerPowerErrorArgs { description }) => {
                // Do something with the error.
            }
        }
    }
}
```

# Details

The `controller` macro will generate the following for you:

## Controller struct

* A `new` method that takes the fields of the struct as arguments and returns the struct.
* For each `published` field:
  * Setter for this field, named `set_<field-name>` (e.g., `set_state`), which broadcasts any
    changes made to this field.
* A `run` method with signature `pub async fn run(mut self);` which runs the controller logic,
  proxying calls from the client to the implementations and their return values back to the
  clients (internally via channels). Typically you'd call it at the end of your `main` or run it
  as a task.
* For each `signal` method:
  * The method body, that broadcasts the signal to all clients that are listening to it.

## Client API

A client struct named `<struct-name>Client` (`ControllerClient` in the example) with the following
methods:

* All methods defined in the controller impl (except signal methods), which proxy calls to the
  controller and return the results.
* For each `published` field:
  * `receive_<field-name>_changed()` method (e.g., `receive_state_changed()`) that returns a
    stream of state values. The first value yielded is the current state at subscription time,
    and subsequent values are emitted when the field changes. The stream yields values of the
    field type directly (e.g., `State`).
* For each field with a `getter` attribute (e.g., `#[controller(getter)]` or
  `#[controller(getter = "custom_name")]`), a getter method is generated on the client. The default
  name is the field name; a custom name can be specified.
* For each field with a `setter` attribute (e.g., `#[controller(setter)]` or
  `#[controller(setter = "custom_name")]`), a public setter method is generated on the client,
  allowing external code to update the field value through the client API. The default setter
  name is `set_<field-name>()`. This can be combined with `publish` to also broadcast changes.
* For each `signal` method:
  * `receive_<method-name>()` method (e.g., `receive_power_error()`) that returns a stream of
    signal events. The stream yields `<struct-name><method-name-in-pascal-case>Args` structs
    (e.g., `ControllerPowerErrorArgs`) containing all signal arguments as public fields.

## Poll Methods

Methods can be marked for periodic execution using poll attributes. These methods are called
automatically by the controller's `run()` loop at the specified interval.

Three time unit attributes are supported:
* `#[controller(poll_seconds = N)]` - Poll every N seconds.
* `#[controller(poll_millis = N)]` - Poll every N milliseconds.
* `#[controller(poll_micros = N)]` - Poll every N microseconds.

Example:
```rust,no_run
use firmware_controller::controller;

#[controller]
mod sensor_controller {
    pub struct Controller {
        #[controller(publish)]
        temperature: f32,
    }

    impl Controller {
        // Called every 5 seconds.
        #[controller(poll_seconds = 5)]
        pub async fn read_temperature(&mut self) {
            // Read from sensor and update state.
            self.set_temperature(42.0).await;
        }

        // Called every 100ms.
        #[controller(poll_millis = 100)]
        pub async fn check_watchdog(&mut self) {
            // Pet the watchdog.
        }

        // Both called every second (grouped together).
        #[controller(poll_seconds = 1)]
        pub async fn log_status(&self) {
            // Log current status.
        }

        #[controller(poll_seconds = 1)]
        pub async fn blink_led(&mut self) {
            // Toggle LED.
        }
    }
}

fn main() {}
```

Key characteristics:
* Poll methods are **not** exposed in the client API. They are internal periodic tasks managed
  entirely by the controller's `run()` loop.
* Methods with the same timeout value (same unit and value) are grouped into a single timer arm
  and called sequentially when the timer fires (in declaration order).
* Uses `embassy_time::Ticker` for timing, which ensures consistent intervals regardless of method
  execution time.

## Dependencies assumed

The `controller` macro assumes that you have the following dependencies in your `Cargo.toml`:

* `futures` with `async-await` feature enabled.
* `embassy-sync`
* `embassy-time` (only required if using poll methods)

## Known limitations & Caveats

* Currently only works as a singleton: you can create multiple instances of the controller but
  if you run them simultaneously, they'll interfere with each others' operation. We hope to remove
  this limitation in the future. Having said that, most firmware applications will only need a
  single controller instance.
* Method args/return type can't be reference types.
* Methods must be async.
* The maximum number of subscribers state change and signal streams is 16. We plan to provide an
  attribute to make this configurable in the future.
* The type of all published fields must implement `Clone`.
* Published field streams yield the current value on first poll, then subsequent changes. Only the
  latest value is stored; intermediate values may be missed if the stream is not polled between
  changes.
* Signal streams must be continuously polled. Otherwise notifications will be missed.
