//! Procedural macros for `firmware-controller`. Do not use directly.
//!
//! See the [`firmware-controller`](https://docs.rs/firmware-controller) crate
//! for documentation.

#[cfg(not(any(feature = "embassy", feature = "tokio")))]
compile_error!("Either the `embassy` or `tokio` feature must be enabled");

#[cfg(all(feature = "embassy", feature = "tokio"))]
compile_error!("The `embassy` and `tokio` features are mutually exclusive");

use proc_macro::TokenStream;
use syn::{parse_macro_input, punctuated::Punctuated, ItemMod, Meta, Token};

mod controller;
mod util;

/// See the [`firmware-controller`](https://docs.rs/firmware-controller) crate
/// for documentation.
#[proc_macro_attribute]
pub fn controller(attr: TokenStream, item: TokenStream) -> TokenStream {
    let _args = parse_macro_input!(attr with Punctuated<Meta, Token![,]>::parse_terminated);

    let input = parse_macro_input!(item as ItemMod);
    controller::expand_module(input)
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}
