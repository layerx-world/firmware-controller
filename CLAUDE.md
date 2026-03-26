# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this
repository.

## Project Overview

This is a workspace with two crates that provide the `#[controller]` attribute macro for
firmware/actor development:

* **`firmware-controller`** (main crate): Re-exports the macro from `-macros` and provides
  `#[doc(hidden)]` re-exports of runtime dependencies (`futures`, `embassy-sync`, `embassy-time`,
  `tokio`, `tokio-stream`) so generated code can reference them without requiring users to add
  these dependencies manually.
* **`firmware-controller-macros`** (proc-macro crate): Contains the actual procedural macro
  implementation. Generated code references external crates through
  `::firmware_controller::__private::` re-exports.

By default it targets `no_std` environments using embassy. With the `tokio` feature, it generates
code for `std` environments using tokio. The macro generates boilerplate for decoupling component
interactions through:

* A controller struct that manages peripheral state.
* Client API for sending commands to the controller.
* Signal mechanism for broadcasting events (PubSubChannel).
* Watch-based subscriptions for state change notifications (yields current value first).

The macro is applied to a module containing both the controller struct definition and its impl
block, allowing coordinated code generation of the controller infrastructure, client API, and
communication channels.

## Build & Test Commands

```bash
# Run all tests with default (embassy) backend
cargo test --locked

# Run all tests with tokio backend
cargo test --locked --no-default-features --features tokio

# Run a specific test
cargo test --locked <test_name>

# Check formatting (requires nightly)
cargo +nightly fmt --all -- --check

# Auto-format code (requires nightly)
cargo +nightly fmt --all

# Run clippy for both backends (CI fails on warnings)
cargo clippy --workspace --locked -- -D warnings
cargo clippy --workspace --locked --no-default-features --features tokio -- -D warnings

# Build the crate
cargo build --locked

# Build documentation
cargo doc --locked
```

## Architecture

### Workspace Layout

```
firmware-controller/                    # workspace root
├── firmware-controller/                # main/facade crate
│   ├── src/lib.rs                      # re-exports macro + #[doc(hidden)] deps
│   └── tests/integration.rs
└── firmware-controller-macros/         # proc-macro crate
    └── src/
        ├── lib.rs                      # macro entry point
        ├── util.rs                     # case conversion helpers
        └── controller/
            ├── mod.rs                  # module orchestration + private_mod_path()
            ├── item_struct.rs          # struct field processing
            └── item_impl.rs            # impl block processing
```

### Backend Selection

The crate has two mutually exclusive features: `embassy` (default) and `tokio`. The main crate
forwards these features to the macros crate and conditionally depends on the corresponding runtime
crates. Code generation functions use `#[cfg(feature = "...")]` in the proc macro code (not in
generated code) to select which token streams to emit. When `tokio` is enabled:

* `embassy_sync::channel::Channel` -> `tokio::sync::mpsc` + `tokio::sync::oneshot`
  (request/response actor pattern)
* `embassy_sync::watch::Watch` -> `tokio::sync::watch` (via `std::sync::OnceLock`)
* `embassy_sync::pubsub::PubSubChannel` -> `tokio::sync::broadcast`
  (via `std::sync::LazyLock`, with `tokio_stream::wrappers::BroadcastStream`)
* Watch subscribers use `tokio_stream::wrappers::WatchStream`.
* `embassy_time::Ticker` -> `tokio::time::interval`
* `futures::select_biased!` -> `tokio::select! { biased; ... }`
* Static channels use `std::sync::LazyLock` since tokio channels lack const constructors.

### Re-export Pattern

Generated code references external crates through the main crate's `__private` module:
`::firmware_controller::__private::embassy_sync::...` etc. The `private_mod_path()` function in
`controller/mod.rs` returns this path as a `TokenStream`, and each code-generation function binds
it to a local `__priv` variable for use in `quote!` blocks.

### Macro Entry Point (`firmware-controller-macros/src/lib.rs`)
The `controller` attribute macro parses the input as an `ItemMod` (module) and calls
`controller::expand_module()`.

### Module Processing (`firmware-controller-macros/src/controller/mod.rs`)
The `expand_module()` function:
* Validates the module has a body with exactly one struct and one impl block.
* Extracts the struct and impl items from the module.
* Validates that the impl block matches the struct name.
* Calls `item_struct::expand()` and `item_impl::expand()` to process each component.
* Combines the generated code back into the module structure along with any other items.

Channel capacities and subscriber limits are also defined here:
* `ALL_CHANNEL_CAPACITY`: 8 (method/getter/setter request channels)
* `SIGNAL_CHANNEL_CAPACITY`: 8 (signal PubSubChannel/broadcast queue size)
* `BROADCAST_MAX_PUBLISHERS`: 1 (signals only, embassy only)
* `BROADCAST_MAX_SUBSCRIBERS`: 16 (Watch for published fields, PubSubChannel for signals,
  embassy only)

### Struct Processing (`firmware-controller-macros/src/controller/item_struct.rs`)
Processes the controller struct definition. Supports three field attributes:

**`#[controller(publish)]`** - Enables state change subscriptions:
* Uses `embassy_sync::watch::Watch` (or `tokio::sync::watch`) channel (stores latest value).
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

The generated `new()` method returns `Option<Self>`, enforcing singleton semantics via a static
`AtomicBool`. It returns `Some` on the first call and `None` on subsequent calls. It initializes
both user fields and generated sender fields, and sends initial values to Watch channels so
subscribers get them immediately.

### Impl Processing (`firmware-controller-macros/src/controller/item_impl.rs`)
Processes the controller impl block. Distinguishes between:

**Proxied methods** (normal methods):
* Creates request/response channels for each method. With tokio, uses `mpsc` + `oneshot` for the
  request/response actor pattern.
* Generates matching client-side methods that send requests and await responses.
* Adds arms to the controller's `run()` method select loop to handle requests.

**Signal methods** (marked with `#[controller(signal)]`):
* Methods have no body in the user's impl block.
* Uses `embassy_sync::pubsub::PubSubChannel` (or `tokio::sync::broadcast`) for broadcast.
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
* Uses `embassy_time::Ticker::every()` (or `tokio::time::interval()`) for timing.

**Getter/setter methods** (from struct field attributes):
* Receives getter/setter field info from struct processing.
* Generates client-side getter methods that request current field value.
* Generates client-side setter methods that update field value (and broadcast if published).

The generated `run()` method contains a `select_biased!` (or `tokio::select! { biased; ... }`)
loop that receives method calls from clients, dispatches them to the user's implementations, and
handles periodic poll method calls.

### Utilities (`firmware-controller-macros/src/util.rs`)
Case conversion functions (`pascal_to_snake_case`, `snake_to_pascal_case`) used for generating
type and method names.

## Dependencies

The main crate directly depends on all runtime crates needed by generated code. Users only need
`firmware-controller` in their `Cargo.toml`. Dev dependencies (`embassy-executor`, `tokio` with
test features, etc.) are only needed for the test suite.

## Key Limitations

* Methods must be async and cannot use reference parameters/return types.
* Maximum 16 subscribers per state/signal stream.
* Published fields must implement `Clone`. With `tokio`, they must also implement `Send + Sync`.
* Signal argument types must implement `Clone`. With `tokio`, they must also implement
  `Send + 'static`.
* Published field streams yield current value on first poll; intermediate values may be missed if
  not polled between changes.
* Signal streams must be continuously polled or notifications are missed.
