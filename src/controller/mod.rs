pub(crate) mod item_impl;
pub(crate) mod item_struct;

use proc_macro2::TokenStream;
use quote::quote;
use syn::{spanned::Spanned, Item, ItemMod, Result};

const ALL_CHANNEL_CAPACITY: usize = 8;
const SIGNAL_CHANNEL_CAPACITY: usize = 8;
#[cfg(feature = "embassy")]
const BROADCAST_MAX_PUBLISHERS: usize = 1;
#[cfg(feature = "embassy")]
const BROADCAST_MAX_SUBSCRIBERS: usize = 16;

pub(crate) fn expand_module(input: ItemMod) -> Result<TokenStream> {
    let vis = &input.vis;
    let mod_name = &input.ident;
    let span = input.span();

    let (_, items) = input
        .content
        .ok_or_else(|| syn::Error::new(span, "Module must have a body"))?;

    let mut struct_item = None;
    let mut impl_item = None;
    let mut other_items = Vec::new();

    for item in items {
        match item {
            Item::Struct(s) => {
                if struct_item.is_some() {
                    return Err(syn::Error::new(
                        s.span(),
                        "Module must contain exactly one struct definition",
                    ));
                }
                struct_item = Some(s);
            }
            Item::Impl(i) => {
                if impl_item.is_some() {
                    return Err(syn::Error::new(
                        i.span(),
                        "Module must contain exactly one impl block",
                    ));
                }
                impl_item = Some(i);
            }
            other => other_items.push(other),
        }
    }

    let struct_item = struct_item.ok_or_else(|| {
        syn::Error::new(
            span,
            "Module must contain a struct definition for the controller",
        )
    })?;

    let impl_item = impl_item.ok_or_else(|| {
        syn::Error::new(span, "Module must contain an impl block for the controller")
    })?;

    let struct_name = &struct_item.ident;
    if let syn::Type::Path(type_path) = &*impl_item.self_ty {
        if let Some(ident) = type_path.path.get_ident() {
            if ident != struct_name {
                return Err(syn::Error::new(
                    impl_item.span(),
                    format!(
                        "Impl block is for type '{}' but controller struct is named '{}'",
                        ident, struct_name
                    ),
                ));
            }
        }
    }

    let expanded_struct = item_struct::expand(struct_item)?;
    let expanded_impl = item_impl::expand(
        impl_item,
        &expanded_struct.published_fields,
        &expanded_struct.getter_fields,
        &expanded_struct.setter_fields,
    )?;
    let struct_tokens = expanded_struct.tokens;

    Ok(quote! {
        #vis mod #mod_name {
            #(#other_items)*

            #struct_tokens

            #expanded_impl
        }
    })
}
