use std::collections::BTreeMap;

use proc_macro2::TokenStream;
use quote::quote;
use syn::{
    parse::{Parse, ParseStream},
    parse_quote,
    spanned::Spanned,
    Attribute, Ident, ImplItem, ImplItemFn, ItemImpl, LitInt, Result, Signature, Token, Visibility,
};

use crate::controller::item_struct::{GetterFieldInfo, PublishedFieldInfo, SetterFieldInfo};
use crate::util::snake_to_pascal_case;

pub(crate) fn expand(
    mut input: ItemImpl,
    published_fields: &[PublishedFieldInfo],
    getter_fields: &[GetterFieldInfo],
    setter_fields: &[SetterFieldInfo],
) -> Result<TokenStream> {
    let struct_name = get_struct_name(&input)?;
    let struct_name_str = struct_name.to_string();
    let methods = get_methods(&mut input, &struct_name)?;

    let signals = methods.iter().filter_map(|m| match m {
        Method::Signal(signal) => Some(signal),
        _ => None,
    });
    let signal_declarations = signals.clone().map(|s| &s.declarations);

    // Extract poll methods and group them by duration.
    let poll_methods: Vec<_> = methods
        .iter()
        .filter_map(|m| match m {
            Method::Poll(poll) => Some(poll),
            _ => None,
        })
        .collect();
    let (poll_ticker_declarations, poll_select_arms) = generate_poll_code(&poll_methods);

    let methods = methods.iter().filter_map(|m| match m {
        Method::Proxied(method) => Some(method),
        _ => None,
    });
    let args_channel_declarations = methods.clone().map(|m| &m.args_channel_declarations);
    let args_channels_rx_tx = methods.clone().map(|m| &m.args_channels_rx_tx);
    let select_arms = methods.clone().map(|m| &m.select_arm);

    // Generate public setters for fields with setter attribute.
    let pub_setters: Vec<_> = setter_fields
        .iter()
        .map(|field| generate_pub_setter(field, &struct_name))
        .collect();
    let pub_setter_channel_declarations = pub_setters.iter().map(|s| &s.channel_declarations);
    let pub_setter_rx_tx = pub_setters.iter().map(|s| &s.rx_tx);
    let pub_setter_select_arms = pub_setters.iter().map(|s| &s.select_arm);
    let pub_setter_client_methods = pub_setters.iter().map(|s| &s.client_method);
    let pub_setter_client_tx_rx_declarations =
        pub_setters.iter().map(|s| &s.client_tx_rx_declarations);
    let pub_setter_client_tx_rx_initializations =
        pub_setters.iter().map(|s| &s.client_tx_rx_initializations);

    // Generate public getters for fields with getter attribute.
    let pub_getters: Vec<_> = getter_fields
        .iter()
        .map(|field| generate_pub_getter(field, &struct_name))
        .collect();
    let pub_getter_channel_declarations = pub_getters.iter().map(|g| &g.channel_declarations);
    let pub_getter_rx_tx = pub_getters.iter().map(|g| &g.rx_tx);
    let pub_getter_select_arms = pub_getters.iter().map(|g| &g.select_arm);
    let pub_getter_client_methods = pub_getters.iter().map(|g| &g.client_method);
    let pub_getter_client_tx_rx_declarations =
        pub_getters.iter().map(|g| &g.client_tx_rx_declarations);
    let pub_getter_client_tx_rx_initializations =
        pub_getters.iter().map(|g| &g.client_tx_rx_initializations);

    let run_method = quote! {
        pub async fn run(mut self) {
            #(#args_channels_rx_tx)*
            #(#pub_setter_rx_tx)*
            #(#pub_getter_rx_tx)*
            #(#poll_ticker_declarations)*

            loop {
                futures::select_biased! {
                    #(#select_arms,)*
                    #(#pub_setter_select_arms,)*
                    #(#pub_getter_select_arms,)*
                    #(#poll_select_arms,)*
                }
            }
        }
    };
    input.items.push(syn::parse2(run_method)?);

    // Generate stream getter methods for published fields.
    let published_field_getters = published_fields.iter().map(|field| {
        let method_name = Ident::new(
            &format!("receive_{}_changed", field.field_name),
            field.field_name.span(),
        );
        let subscriber_type = &field.subscriber_struct_name;
        quote! {
            pub fn #method_name(&self) -> Option<#subscriber_type> {
                #subscriber_type::new()
            }
        }
    });

    // Generate stream getter methods for signals.
    let signal_getters = signals.clone().map(|signal| {
        let method_name = &signal.receive_method_name;
        let subscriber_type = &signal.subscriber_struct_name;
        quote! {
            pub fn #method_name(&self) -> Option<#subscriber_type> {
                #subscriber_type::new()
            }
        }
    });

    let client_name = Ident::new(&format!("{}Client", struct_name_str), input.span());
    let client_methods = methods.clone().map(|m| &m.client_method);
    let client_method_tx_rx_declarations =
        methods.clone().map(|m| &m.client_method_tx_rx_declarations);
    let client_method_tx_rx_initializations = methods
        .clone()
        .map(|m| &m.client_method_tx_rx_initializations);

    Ok(quote! {
        #(#args_channel_declarations)*
        #(#pub_setter_channel_declarations)*
        #(#pub_getter_channel_declarations)*

        #input

        pub struct #client_name {
            #(#client_method_tx_rx_declarations)*
            #(#pub_setter_client_tx_rx_declarations)*
            #(#pub_getter_client_tx_rx_declarations)*
        }

        impl #client_name {
            pub fn new() -> Self {
                Self {
                    #(#client_method_tx_rx_initializations)*
                    #(#pub_setter_client_tx_rx_initializations)*
                    #(#pub_getter_client_tx_rx_initializations)*
                }
            }

            #(#client_methods)*

            #(#pub_setter_client_methods)*

            #(#pub_getter_client_methods)*

            #(#published_field_getters)*

            #(#signal_getters)*
        }

        #(#signal_declarations)*
    })
}

fn get_methods(input: &mut ItemImpl, struct_name: &Ident) -> Result<Vec<Method>> {
    input
        .items
        .iter_mut()
        .filter_map(|item| match item {
            syn::ImplItem::Fn(m) => {
                // Check if this is a poll method first.
                match PollMethod::parse(m) {
                    Ok(Some(poll)) => Some(Ok(Method::Poll(poll))),
                    Ok(None) => {
                        // Not a poll method, treat as proxied.
                        Some(ProxiedMethod::parse(m, struct_name).map(Method::Proxied))
                    }
                    Err(e) => Some(Err(e)),
                }
            }
            syn::ImplItem::Verbatim(tokens) => {
                // Signal methods have a semicolon at the end instead of a body block,
                // thus parse them ourselves and construct an ImplItemFn from that.
                let ImplItemSignal { attrs, vis, sig } =
                    match syn::parse2::<ImplItemSignal>(tokens.clone()) {
                        Ok(decl) => decl,
                        Err(e) => return Some(Err(e)),
                    };
                *item = ImplItem::Fn(ImplItemFn {
                    attrs,
                    vis,
                    defaultness: None,
                    sig,
                    // This empty block will be replaced below.
                    block: parse_quote!({}),
                });
                match item {
                    ImplItem::Fn(m) => Some(Signal::parse(m, struct_name).map(Method::Signal)),
                    _ => unreachable!(),
                }
            }
            _ => None,
        })
        .collect::<Result<Vec<_>>>()
}

fn get_struct_name(input: &ItemImpl) -> Result<Ident> {
    match input.self_ty.as_ref() {
        syn::Type::Path(path) => {
            let path = &path.path;
            if path.segments.len() != 1 {
                return Err(syn::Error::new(
                    path.span(),
                    "Expected single segment in type path",
                ));
            }
            let segment = path.segments.first().unwrap();

            Ok(segment.ident.clone())
        }
        _ => Err(syn::Error::new(
            input.self_ty.span(),
            "Expected type path in impl block",
        )),
    }
}

#[derive(Debug)]
enum Method {
    /// A method that will be proxied.
    Proxied(ProxiedMethod),
    /// A signal method.
    Signal(Signal),
    /// A method that will be called periodically.
    Poll(PollMethod),
}

/// Method that will be called by the client.
// TODO: Better name.
#[derive(Debug)]
struct ProxiedMethod {
    /// The arguments' channel declarations (both for input and output).
    args_channel_declarations: TokenStream,
    /// The input arguments' receiver & output sender declarations.
    args_channels_rx_tx: TokenStream,
    /// The select! arm for proxying the method call from the input channel & method return to the
    /// output channel.
    select_arm: TokenStream,
    /// The client-side method.
    client_method: TokenStream,
    /// The client-side method's input and output channel tx & rx declarations.
    client_method_tx_rx_declarations: TokenStream,
    /// The client-side method's input and output channel tx & rx initializations.
    client_method_tx_rx_initializations: TokenStream,
}

impl ProxiedMethod {
    fn parse(method: &ImplItemFn, struct_name: &Ident) -> Result<Self> {
        let method_args = ProxiedMethodArgs::parse(method)?;

        let (args_channel_declarations, input_channel_name, output_channel_name) =
            method_args.generate_args_channel_declarations(struct_name);
        let (args_channels_rx_tx, select_arm) =
            method_args.generate_args_channel_rx_tx(&input_channel_name, &output_channel_name);
        let (client_method, client_method_tx_rx_declarations, client_method_tx_rx_initializations) =
            method_args.generate_client_method(&input_channel_name, &output_channel_name);

        Ok(Self {
            args_channel_declarations,
            args_channels_rx_tx,
            select_arm,
            client_method,
            client_method_tx_rx_declarations,
            client_method_tx_rx_initializations,
        })
    }
}

#[derive(Debug)]
struct ProxiedMethodArgs<'a> {
    method: &'a ImplItemFn,
    in_args: MethodInputArgs,
    out_type: TokenStream,
}

impl ProxiedMethodArgs<'_> {
    fn parse(method: &ImplItemFn) -> Result<ProxiedMethodArgs<'_>> {
        let in_args = MethodInputArgs::parse(method)?;
        let out_type = match &method.sig.output {
            syn::ReturnType::Type(_, ty) => quote! { #ty },
            syn::ReturnType::Default => quote! { () },
        };

        Ok(ProxiedMethodArgs {
            method,
            in_args,
            out_type,
        })
    }

    fn generate_args_channel_declarations(
        &self,
        struct_name: &Ident,
    ) -> (TokenStream, Ident, Ident) {
        let in_types = &self.in_args.types;
        let out_type = &self.out_type;
        let method_name = &self.method.sig.ident;
        let method_name_str = method_name.to_string();

        let struct_name_caps = struct_name.to_string().to_uppercase();
        let method_name_caps = method_name_str.to_uppercase();
        let input_channel_name = Ident::new(
            &format!("{struct_name_caps}_{method_name_caps}_INPUT_CHANNEL"),
            self.method.span(),
        );
        let output_channel_name = Ident::new(
            &format!("{struct_name_caps}_{method_name_caps}_OUTPUT_CHANNEL"),
            self.method.span(),
        );
        let capacity = super::ALL_CHANNEL_CAPACITY;
        let args_channel_declarations = quote! {
            static #input_channel_name:
                embassy_sync::channel::Channel<
                    embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
                    (#(#in_types),*),
                    #capacity,
                > = embassy_sync::channel::Channel::new();
            static #output_channel_name:
                embassy_sync::channel::Channel<
                    embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
                    #out_type,
                    #capacity,
                > = embassy_sync::channel::Channel::new();
        };

        (
            args_channel_declarations,
            input_channel_name,
            output_channel_name,
        )
    }

    // Also generates the select! arm for the method dispatch.
    fn generate_args_channel_rx_tx(
        &self,
        input_channel_name: &Ident,
        output_channel_name: &Ident,
    ) -> (TokenStream, TokenStream) {
        let in_names = &self.in_args.names;
        let method_name = &self.method.sig.ident;
        let method_name_str = method_name.to_string();
        let input_channel_rx_name =
            Ident::new(&format!("{method_name_str}_rx"), self.method.span());
        let output_channel_tx_name =
            Ident::new(&format!("{method_name_str}_tx"), self.method.span());
        let args_channels_rx_tx = quote! {
            let #input_channel_rx_name = embassy_sync::channel::Channel::receiver(&#input_channel_name);
            let #output_channel_tx_name = embassy_sync::channel::Channel::sender(&#output_channel_name);
        };
        let select_arm = quote! {
            (#(#in_names),*) = futures::FutureExt::fuse(
                embassy_sync::channel::Receiver::receive(&#input_channel_rx_name),
            ) => {
                let ret = self.#method_name(#(#in_names),*).await;

                embassy_sync::channel::Sender::send(&#output_channel_tx_name, ret).await;
            }
        };

        (args_channels_rx_tx, select_arm)
    }

    // Also generate the input and output channel declarations, and initializations.
    fn generate_client_method(
        &self,
        input_channel_name: &Ident,
        output_channel_name: &Ident,
    ) -> (TokenStream, TokenStream, TokenStream) {
        let method_name = &self.method.sig.ident;
        let in_names = &self.in_args.names;
        let in_names = if in_names.is_empty() {
            quote! { () }
        } else {
            quote! { (#(#in_names),*) }
        };
        let method_name_str = method_name.to_string();
        let input_channel_tx_name =
            Ident::new(&format!("{method_name_str}_tx"), self.method.span());
        let output_channel_rx_name =
            Ident::new(&format!("{method_name_str}_rx"), self.method.span());
        let mut method = self.method.clone();

        method.block = parse_quote!({
            // Method call.
            embassy_sync::channel::Sender::send(&self.#input_channel_tx_name, #in_names).await;

            // Method return.
            embassy_sync::channel::Receiver::receive(&self.#output_channel_rx_name).await
        });

        let in_types = &self.in_args.types;
        let out_type = &self.out_type;
        let capacity = super::ALL_CHANNEL_CAPACITY;
        let tx_rx_declarations = quote! {
            #input_channel_tx_name: embassy_sync::channel::Sender<
                'static,
                embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
                (#(#in_types),*),
                #capacity,
            >,
            #output_channel_rx_name: embassy_sync::channel::Receiver<
                'static,
                embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
                #out_type,
                #capacity,
            >,
        };

        let tx_rx_initializations = quote! {
            #input_channel_tx_name: embassy_sync::channel::Channel::sender(&#input_channel_name),
            #output_channel_rx_name: embassy_sync::channel::Channel::receiver(&#output_channel_name),
        };

        (
            quote! { #method },
            tx_rx_declarations,
            tx_rx_initializations,
        )
    }
}

#[derive(Debug)]
struct Signal {
    /// The input arguments' channel and client-side struct declarations.
    declarations: TokenStream,
    /// Name of the receive method (e.g., receive_power_error).
    receive_method_name: Ident,
    /// Name of the subscriber struct (e.g., ControllerPowerError).
    subscriber_struct_name: Ident,
}

impl Signal {
    fn parse(method: &mut ImplItemFn, struct_name: &Ident) -> Result<Self> {
        remove_signal_attr(method)?;

        let MethodInputArgs { types, names } = MethodInputArgs::parse(method)?;

        let method_name = &method.sig.ident;
        let method_name_str = method_name.to_string();
        let struct_name_caps = struct_name.to_string().to_uppercase();
        let method_name_caps = method_name_str.to_uppercase();
        let method_name_pascal = snake_to_pascal_case(&method_name_str);
        let signal_channel_name = Ident::new(
            &format!("{struct_name_caps}_{method_name_caps}_CHANNEL"),
            method.span(),
        );
        let signal_publisher_name = Ident::new(
            &format!("{struct_name_caps}_{method_name_caps}_PUBLISHER"),
            method.span(),
        );
        let subscriber_struct_name =
            Ident::new(&format!("{struct_name}{method_name_pascal}"), method.span());
        let args_struct_name = Ident::new(
            &format!("{struct_name}{method_name_pascal}Args"),
            method.span(),
        );

        let capacity = super::SIGNAL_CHANNEL_CAPACITY;
        let max_subscribers = super::BROADCAST_MAX_SUBSCRIBERS;
        let max_publishers = super::BROADCAST_MAX_PUBLISHERS;

        let declarations = quote! {
            static #signal_channel_name:
                embassy_sync::pubsub::PubSubChannel<
                    embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
                    #args_struct_name,
                    #capacity,
                    #max_subscribers,
                    #max_publishers,
                > = embassy_sync::pubsub::PubSubChannel::new();

            static #signal_publisher_name: embassy_sync::once_lock::OnceLock<embassy_sync::pubsub::publisher::Publisher<
                'static,
                embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
                #args_struct_name,
                #capacity,
                #max_subscribers,
                #max_publishers,
                // Safety: The publisher is only initialized once.
            >> = embassy_sync::once_lock::OnceLock::new();

            #[derive(Debug, Clone)]
            pub struct #args_struct_name {
                #(pub #names: #types),*
            }

            pub struct #subscriber_struct_name {
                subscriber: embassy_sync::pubsub::subscriber::Subscriber<
                    'static,
                    embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
                    #args_struct_name,
                    #capacity,
                    #max_subscribers,
                    #max_publishers,
                >,
            }

            impl #subscriber_struct_name {
                pub fn new() -> Option<Self> {
                    embassy_sync::pubsub::PubSubChannel::subscriber(&#signal_channel_name)
                        .ok()
                        .map(|subscriber| Self { subscriber })
                }
            }

            impl futures::Stream for #subscriber_struct_name {
                type Item = #args_struct_name;

                fn poll_next(
                    self: core::pin::Pin<&mut Self>,
                    cx: &mut core::task::Context<'_>,
                ) -> core::task::Poll<Option<Self::Item>> {
                    let subscriber = core::pin::Pin::new(&mut *self.get_mut().subscriber);
                    futures::Stream::poll_next(subscriber, cx)
                }
            }
        };

        method.block = parse_quote!({
            let publisher = embassy_sync::once_lock::OnceLock::get_or_init(
                &#signal_publisher_name,
                // Safety: The publisher is only initialized once.
                || embassy_sync::pubsub::PubSubChannel::publisher(&#signal_channel_name).unwrap());
            embassy_sync::pubsub::publisher::Pub::publish(
                publisher,
                #args_struct_name { #(#names),* },
            ).await;
        });

        let receive_method_name =
            Ident::new(&format!("receive_{}", method_name_str), method.span());

        Ok(Self {
            declarations,
            receive_method_name,
            subscriber_struct_name,
        })
    }
}

/// Duration for poll methods. Used as a key for grouping methods with the same timeout.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum PollDuration {
    Seconds(u64),
    Millis(u64),
    Micros(u64),
}

impl PollDuration {
    fn to_duration_expr(&self) -> TokenStream {
        match self {
            PollDuration::Seconds(n) => quote! { embassy_time::Duration::from_secs(#n) },
            PollDuration::Millis(n) => quote! { embassy_time::Duration::from_millis(#n) },
            PollDuration::Micros(n) => quote! { embassy_time::Duration::from_micros(#n) },
        }
    }
}

/// A method that will be called periodically with a timeout.
#[derive(Debug)]
struct PollMethod {
    method_name: Ident,
    duration: PollDuration,
}

impl PollMethod {
    fn parse(method: &mut ImplItemFn) -> Result<Option<Self>> {
        let mut duration: Option<(PollDuration, proc_macro2::Span)> = None;

        // Check if method has a poll attribute.
        for attr in &method.attrs {
            if !attr.path().is_ident("controller") {
                continue;
            }

            attr.parse_nested_meta(|meta| {
                let new_duration = if meta.path.is_ident("poll_seconds") {
                    meta.input.parse::<Token![=]>()?;
                    let lit: LitInt = meta.input.parse()?;
                    let value: u64 = lit.base10_parse()?;
                    if value == 0 {
                        return Err(syn::Error::new_spanned(
                            lit,
                            "poll duration must be greater than zero",
                        ));
                    }
                    Some((PollDuration::Seconds(value), meta.path.span()))
                } else if meta.path.is_ident("poll_millis") {
                    meta.input.parse::<Token![=]>()?;
                    let lit: LitInt = meta.input.parse()?;
                    let value: u64 = lit.base10_parse()?;
                    if value == 0 {
                        return Err(syn::Error::new_spanned(
                            lit,
                            "poll duration must be greater than zero",
                        ));
                    }
                    Some((PollDuration::Millis(value), meta.path.span()))
                } else if meta.path.is_ident("poll_micros") {
                    meta.input.parse::<Token![=]>()?;
                    let lit: LitInt = meta.input.parse()?;
                    let value: u64 = lit.base10_parse()?;
                    if value == 0 {
                        return Err(syn::Error::new_spanned(
                            lit,
                            "poll duration must be greater than zero",
                        ));
                    }
                    Some((PollDuration::Micros(value), meta.path.span()))
                } else {
                    None
                };

                if let Some((new_dur, span)) = new_duration {
                    if duration.is_some() {
                        return Err(syn::Error::new(
                            span,
                            "only one poll attribute is allowed per method",
                        ));
                    }
                    duration = Some((new_dur, span));
                }

                Ok(())
            })?;
        }

        let Some((duration, _)) = duration else {
            return Ok(None);
        };

        // Validate that poll methods have no parameters besides receiver.
        let has_non_receiver_params = method
            .sig
            .inputs
            .iter()
            .any(|arg| matches!(arg, syn::FnArg::Typed(_)));
        if has_non_receiver_params {
            return Err(syn::Error::new_spanned(
                &method.sig.inputs,
                "poll methods cannot have parameters (only `&self` or `&mut self` is allowed)",
            ));
        }

        // Remove the poll attribute from the method.
        remove_poll_attr(method)?;

        let method_name = method.sig.ident.clone();
        Ok(Some(Self {
            method_name,
            duration,
        }))
    }
}

fn remove_poll_attr(method: &mut ImplItemFn) -> syn::Result<()> {
    method.attrs = method
        .attrs
        .iter()
        .cloned()
        .filter_map(|attr| {
            if !attr.path().is_ident("controller") {
                return Some(Ok(attr));
            }

            let res = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("poll_seconds")
                    || meta.path.is_ident("poll_millis")
                    || meta.path.is_ident("poll_micros")
                {
                    // Consume the `= N` part.
                    meta.input.parse::<Token![=]>()?;
                    let _: LitInt = meta.input.parse()?;
                    Ok(())
                } else {
                    let path = &meta.path;
                    let found = path
                        .get_ident()
                        .map(|ident| ident.to_string())
                        .unwrap_or_else(|| quote!(#path).to_string());
                    let e = format!(
                        "poll methods cannot have other `controller` attributes (found `{}`); \
                         remove attributes like `getter`, `setter`, `publish`, or `signal`",
                        found
                    );
                    Err(syn::Error::new_spanned(meta.path, e))
                }
            });
            match res {
                Err(e) => Some(Err(e)),
                Ok(()) => None,
            }
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(())
}

// Like ImplItemFn, but with a semicolon at the end instead of a body block
struct ImplItemSignal {
    attrs: Vec<Attribute>,
    vis: Visibility,
    sig: Signature,
}

impl Parse for ImplItemSignal {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let attrs = input.call(Attribute::parse_outer)?;
        let vis = input.parse()?;
        let sig = input.parse()?;
        let _: Token![;] = input.parse()?;

        Ok(ImplItemSignal { attrs, vis, sig })
    }
}

fn remove_signal_attr(method: &mut ImplItemFn) -> syn::Result<()> {
    method.attrs = method
        .attrs
        .iter()
        .cloned()
        .filter_map(|attr| {
            if !attr.path().is_ident("controller") {
                return Some(Ok(attr));
            }

            let res = attr.parse_nested_meta(|meta| {
                if !meta.path.is_ident("signal") {
                    let e = format!(
                        "expected `signal` attribute, found `{}`",
                        meta.path.get_ident().unwrap()
                    );

                    return Err(syn::Error::new_spanned(meta.path, e));
                }

                Ok(())
            });
            match res {
                Err(e) => Some(Err(e)),
                Ok(()) => None,
            }
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(())
}

#[derive(Debug)]
struct MethodInputArgs {
    types: Vec<TokenStream>,
    names: Vec<TokenStream>,
}

impl MethodInputArgs {
    fn parse(method: &ImplItemFn) -> Result<Self> {
        let (in_types, in_names): (Vec<_>, Vec<_>) = method
            .sig
            .inputs
            .iter()
            .filter_map(|arg| match arg {
                syn::FnArg::Typed(arg) => {
                    let arg_type = &arg.ty;
                    let arg_name = match &*arg.pat {
                        syn::Pat::Ident(pat) => &pat.ident,
                        _ => {
                            return Some(Err(syn::Error::new(
                                arg.span(),
                                "Expected identifier as argument name",
                            )))
                        }
                    };

                    Some(Ok((quote! { #arg_type }, quote! { #arg_name })))
                }
                // Filter `self`.
                syn::FnArg::Receiver(_) => None,
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .unzip();

        Ok(Self {
            types: in_types,
            names: in_names,
        })
    }
}

#[derive(Debug)]
struct PubSetter {
    channel_declarations: TokenStream,
    rx_tx: TokenStream,
    select_arm: TokenStream,
    client_method: TokenStream,
    client_tx_rx_declarations: TokenStream,
    client_tx_rx_initializations: TokenStream,
}

#[derive(Debug)]
struct PubGetter {
    channel_declarations: TokenStream,
    rx_tx: TokenStream,
    select_arm: TokenStream,
    client_method: TokenStream,
    client_tx_rx_declarations: TokenStream,
    client_tx_rx_initializations: TokenStream,
}

fn generate_pub_setter(field: &SetterFieldInfo, struct_name: &Ident) -> PubSetter {
    let field_name = &field.field_name;
    let field_type = &field.field_type;
    let setter_method_name = &field.setter_name;
    let field_name_str = field_name.to_string();

    let struct_name_caps = struct_name.to_string().to_uppercase();
    let field_name_caps = field_name_str.to_uppercase();
    let input_channel_name = Ident::new(
        &format!("{}_SET_{}_INPUT_CHANNEL", struct_name_caps, field_name_caps),
        field_name.span(),
    );
    let output_channel_name = Ident::new(
        &format!(
            "{}_SET_{}_OUTPUT_CHANNEL",
            struct_name_caps, field_name_caps
        ),
        field_name.span(),
    );
    let capacity = super::ALL_CHANNEL_CAPACITY;

    let channel_declarations = quote! {
        static #input_channel_name:
            embassy_sync::channel::Channel<
                embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
                #field_type,
                #capacity,
            > = embassy_sync::channel::Channel::new();
        static #output_channel_name:
            embassy_sync::channel::Channel<
                embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
                (),
                #capacity,
            > = embassy_sync::channel::Channel::new();
    };

    let input_channel_rx_name =
        Ident::new(&format!("{}_value_rx", field_name_str), field_name.span());
    let output_channel_tx_name =
        Ident::new(&format!("{}_ack_tx", field_name_str), field_name.span());
    let rx_tx = quote! {
        let #input_channel_rx_name = embassy_sync::channel::Channel::receiver(&#input_channel_name);
        let #output_channel_tx_name = embassy_sync::channel::Channel::sender(&#output_channel_name);
    };

    let select_arm = if let Some(internal_setter) = &field.internal_setter_name {
        // Published field: call the internal setter which broadcasts changes.
        quote! {
            value = futures::FutureExt::fuse(
                embassy_sync::channel::Receiver::receive(&#input_channel_rx_name),
            ) => {
                self.#internal_setter(value).await;

                embassy_sync::channel::Sender::send(&#output_channel_tx_name, ()).await;
            }
        }
    } else {
        // Non-published field: set the field directly.
        quote! {
            value = futures::FutureExt::fuse(
                embassy_sync::channel::Receiver::receive(&#input_channel_rx_name),
            ) => {
                self.#field_name = value;

                embassy_sync::channel::Sender::send(&#output_channel_tx_name, ()).await;
            }
        }
    };

    let input_channel_tx_name =
        Ident::new(&format!("{}_value_tx", field_name_str), field_name.span());
    let output_channel_rx_name =
        Ident::new(&format!("{}_ack_rx", field_name_str), field_name.span());
    let client_method = quote! {
        pub async fn #setter_method_name(&self, value: #field_type) {
            embassy_sync::channel::Sender::send(&self.#input_channel_tx_name, value).await;
            embassy_sync::channel::Receiver::receive(&self.#output_channel_rx_name).await
        }
    };

    let client_tx_rx_declarations = quote! {
        #input_channel_tx_name: embassy_sync::channel::Sender<
            'static,
            embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
            #field_type,
            #capacity,
        >,
        #output_channel_rx_name: embassy_sync::channel::Receiver<
            'static,
            embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
            (),
            #capacity,
        >,
    };

    let client_tx_rx_initializations = quote! {
        #input_channel_tx_name: embassy_sync::channel::Channel::sender(&#input_channel_name),
        #output_channel_rx_name: embassy_sync::channel::Channel::receiver(&#output_channel_name),
    };

    PubSetter {
        channel_declarations,
        rx_tx,
        select_arm,
        client_method,
        client_tx_rx_declarations,
        client_tx_rx_initializations,
    }
}

fn generate_pub_getter(field: &GetterFieldInfo, struct_name: &Ident) -> PubGetter {
    let field_name = &field.field_name;
    let field_type = &field.field_type;
    let getter_name = &field.getter_name;
    let field_name_str = field_name.to_string();

    let struct_name_caps = struct_name.to_string().to_uppercase();
    let field_name_caps = field_name_str.to_uppercase();
    let input_channel_name = Ident::new(
        &format!("{}_GET_{}_INPUT_CHANNEL", struct_name_caps, field_name_caps),
        field_name.span(),
    );
    let output_channel_name = Ident::new(
        &format!(
            "{}_GET_{}_OUTPUT_CHANNEL",
            struct_name_caps, field_name_caps
        ),
        field_name.span(),
    );
    let capacity = super::ALL_CHANNEL_CAPACITY;

    let channel_declarations = quote! {
        static #input_channel_name:
            embassy_sync::channel::Channel<
                embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
                (),
                #capacity,
            > = embassy_sync::channel::Channel::new();
        static #output_channel_name:
            embassy_sync::channel::Channel<
                embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
                #field_type,
                #capacity,
            > = embassy_sync::channel::Channel::new();
    };

    let input_channel_rx_name = Ident::new(
        &format!("{}_get_request_rx", field_name_str),
        field_name.span(),
    );
    let output_channel_tx_name = Ident::new(
        &format!("{}_get_response_tx", field_name_str),
        field_name.span(),
    );
    let rx_tx = quote! {
        let #input_channel_rx_name = embassy_sync::channel::Channel::receiver(&#input_channel_name);
        let #output_channel_tx_name = embassy_sync::channel::Channel::sender(&#output_channel_name);
    };

    let select_arm = quote! {
        _ = futures::FutureExt::fuse(
            embassy_sync::channel::Receiver::receive(&#input_channel_rx_name),
        ) => {
            let value = core::clone::Clone::clone(&self.#field_name);

            embassy_sync::channel::Sender::send(&#output_channel_tx_name, value).await;
        }
    };

    let input_channel_tx_name = Ident::new(
        &format!("{}_get_request_tx", field_name_str),
        field_name.span(),
    );
    let output_channel_rx_name = Ident::new(
        &format!("{}_get_response_rx", field_name_str),
        field_name.span(),
    );
    let client_method = quote! {
        pub async fn #getter_name(&self) -> #field_type {
            embassy_sync::channel::Sender::send(&self.#input_channel_tx_name, ()).await;
            embassy_sync::channel::Receiver::receive(&self.#output_channel_rx_name).await
        }
    };

    let client_tx_rx_declarations = quote! {
        #input_channel_tx_name: embassy_sync::channel::Sender<
            'static,
            embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
            (),
            #capacity,
        >,
        #output_channel_rx_name: embassy_sync::channel::Receiver<
            'static,
            embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
            #field_type,
            #capacity,
        >,
    };

    let client_tx_rx_initializations = quote! {
        #input_channel_tx_name: embassy_sync::channel::Channel::sender(&#input_channel_name),
        #output_channel_rx_name: embassy_sync::channel::Channel::receiver(&#output_channel_name),
    };

    PubGetter {
        channel_declarations,
        rx_tx,
        select_arm,
        client_method,
        client_tx_rx_declarations,
        client_tx_rx_initializations,
    }
}

/// Generate ticker declarations and select arms for poll methods, grouped by duration.
///
/// Returns (ticker_declarations, select_arms) where:
/// - ticker_declarations: Code to create Tickers before the loop.
/// - select_arms: Select arms that wait on ticker.next().
fn generate_poll_code(poll_methods: &[&PollMethod]) -> (Vec<TokenStream>, Vec<TokenStream>) {
    // Group poll methods by duration.
    let mut groups: BTreeMap<PollDuration, Vec<&Ident>> = BTreeMap::new();
    for poll in poll_methods {
        groups
            .entry(poll.duration.clone())
            .or_default()
            .push(&poll.method_name);
    }

    let mut ticker_declarations = Vec::new();
    let mut select_arms = Vec::new();

    for (index, (duration, method_names)) in groups.into_iter().enumerate() {
        let duration_expr = duration.to_duration_expr();
        let ticker_name = Ident::new(
            &format!("__poll_ticker_{index}"),
            proc_macro2::Span::call_site(),
        );

        ticker_declarations.push(quote! {
            let mut #ticker_name = embassy_time::Ticker::every(#duration_expr);
        });

        select_arms.push(quote! {
            _ = futures::FutureExt::fuse(#ticker_name.next()) => {
                #(self.#method_names().await;)*
            }
        });
    }

    (ticker_declarations, select_arms)
}
