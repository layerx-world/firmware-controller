#![doc = include_str!("../../README.md")]

#[cfg(not(any(feature = "embassy", feature = "tokio")))]
compile_error!("Either the `embassy` or `tokio` feature must be enabled");

#[cfg(all(feature = "embassy", feature = "tokio"))]
compile_error!("The `embassy` and `tokio` features are mutually exclusive");

pub use firmware_controller_macros::controller;

/// Re-exports for use by generated code. Not part of the public API.
#[doc(hidden)]
pub mod __private {
    pub use futures;

    #[cfg(feature = "embassy")]
    pub use embassy_sync;

    #[cfg(feature = "embassy")]
    pub use embassy_time;

    #[cfg(feature = "tokio")]
    pub use tokio;

    #[cfg(feature = "tokio")]
    pub use tokio_stream;
}
