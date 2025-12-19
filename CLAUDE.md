# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is a procedural macro crate that provides the `#[controller]` attribute macro for firmware development in `no_std` environments. The macro generates boilerplate for decoupling component interactions through:

* A controller struct that manages peripheral state.
* Client API for sending commands to the controller.
* Signal mechanism for broadcasting events (PubSubChannel).
* Watch-based subscriptions for state change notifications (yields current value first).

The macro is applied to a module containing both the controller struct definition and its impl block, allowing coordinated code generation of the controller infrastructure, client API, and communication channels.

## Build & Test Commands

```bash
# Run all tests (includes doc tests from README)
cargo test --locked

# Run a specific test
cargo test --locked <test_name>

# Check formatting (requires nightly)
cargo +nightly fmt -- --check

# Auto-format code (requires nightly)
cargo +nightly fmt

# Run clippy (CI fails on warnings)
cargo clippy --locked -- -D warnings

# Build the crate
cargo build --locked

# Build documentation
cargo doc --locked
```

## Architecture

### Macro Entry Point (`src/lib.rs`)
The `controller` attribute macro parses the input as an `ItemMod` (module) and calls `controller::expand_module()`.

### Module Processing (`src/controller/mod.rs`)
The `expand_module()` function:
* Validates the module has a body with exactly one struct and one impl block.
* Extracts the struct and impl items from the module.
* Validates that the impl block matches the struct name.
* Calls `item_struct::expand()` and `item_impl::expand()` to process each component.
* Combines the generated code back into the module structure along with any other items.

Channel capacities and subscriber limits are also defined here:
* `ALL_CHANNEL_CAPACITY`: 8 (method/getter/setter request channels)
* `SIGNAL_CHANNEL_CAPACITY`: 8 (signal PubSubChannel queue size)
* `BROADCAST_MAX_PUBLISHERS`: 1 (signals only)
* `BROADCAST_MAX_SUBSCRIBERS`: 16 (Watch for published fields, PubSubChannel for signals)

### Struct Processing (`src/controller/item_struct.rs`)
Processes the controller struct definition. Supports three field attributes:

**`#[controller(publish)]`** - Enables state change subscriptions:
* Uses `embassy_sync::watch::Watch` channel (stores latest value).
* Generates internal setter (`set_<field>`) that broadcasts changes.
* Creates `<StructName><FieldName>` subscriber stream type.
* Stream yields current value on first poll, then subsequent changes.

**`#[controller(getter)]` or `#[controller(getter = "name")]`**:
* Generates a client-side getter method to read the field value.
* Default name is the field name; custom name can be specified.

**`#[controller(setter)]` or `#[controller(setter = "name")]`**:
* Generates a client-side setter method to update the field value.
* Default name is `set_<field>`; custom name can be specified.
* Can be combined with `publish` to also broadcast changes.

The generated `new()` method initializes both user fields and generated sender fields, and sends
initial values to Watch channels so subscribers get them immediately.

### Impl Processing (`src/controller/item_impl.rs`)
Processes the controller impl block. Distinguishes between:

**Proxied methods** (normal methods):
* Creates request/response channels for each method.
* Generates matching client-side methods that send requests and await responses.
* Adds arms to the controller's `run()` method select loop to handle requests.

**Signal methods** (marked with `#[controller(signal)]`):
* Methods have no body in the user's impl block.
* Uses `embassy_sync::pubsub::PubSubChannel` for broadcast.
* Generates method implementation that broadcasts to subscribers.
* Creates `<StructName><MethodName>` stream type and `<StructName><MethodName>Args` struct.
* Signal methods are NOT exposed in the client API (controller emits them directly).

**Poll methods** (marked with `#[controller(poll_*)]`):
* Methods are called periodically at the specified interval.
* Three time unit attributes are supported:
  * `#[controller(poll_seconds = N)]` - Poll every N seconds.
  * `#[controller(poll_millis = N)]` - Poll every N milliseconds.
  * `#[controller(poll_micros = N)]` - Poll every N microseconds.
* Methods with the same timeout value (same unit and value) are grouped into a single ticker arm.
* All methods in a group are called sequentially when the ticker fires (in declaration order).
* Poll methods are NOT exposed in the client API (internal to the controller).
* Uses `embassy_time::Ticker::every()` for timing.

**Getter/setter methods** (from struct field attributes):
* Receives getter/setter field info from struct processing.
* Generates client-side getter methods that request current field value.
* Generates client-side setter methods that update field value (and broadcast if published).

The generated `run()` method contains a `select_biased!` loop that receives method calls from
clients, dispatches them to the user's implementations, and handles periodic poll method calls.

### Utilities (`src/util.rs`)
Case conversion functions (`pascal_to_snake_case`, `snake_to_pascal_case`) used for generating type and method names.

## Dependencies

User code must have these dependencies (per README):
* `futures` with `async-await` feature.
* `embassy-sync` for channels and synchronization.
* `embassy-time` for poll method timing (only required if using poll methods).

Dev dependencies include `embassy-executor` and `embassy-time` for testing.

## Key Limitations

* Singleton operation: multiple controller instances interfere with each other.
* Methods must be async and cannot use reference parameters/return types.
* Maximum 16 subscribers per state/signal stream.
* Published fields must implement `Clone`.
* Published field streams yield current value on first poll; intermediate values may be missed if
  not polled between changes.
* Signal streams must be continuously polled or notifications are missed.
