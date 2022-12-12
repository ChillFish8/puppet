mod generics_processor;
mod handler;
mod utils;

extern crate proc_macro;

use crate::generics_processor::{ArgOrder, Extract, GenericBuilder, ProcessedGenerics};
use crate::utils::get_segment;
use proc_macro::TokenStream;
use proc_macro2::Ident;
use quote::{format_ident, quote, ToTokens, TokenStreamExt};
use syn::{parse_macro_input, Error, ItemImpl, Type};

macro_rules! maybe_compiler_error {
    ($res:expr) => {{
        match $res {
            Err(e) => return e.into_compile_error().into(),
            Ok(r) => r,
        }
    }};
}

#[proc_macro_attribute]
/// Create an actor for the given struct Impl.
///
/// This allows you to mark methods as message handlers with the `#[puppet]` attribute.
/// Once marked, the first parameter after one of `&self` or `&mut self` will be the message to
/// receive.
///
/// Once the actor has been derived, you can spawn it any of the following:
///
/// ### Helper Methods (requires `helper-methods` feature)
/// - `spawn_actor()`
/// - `spawn_actor_with_name(name)`
/// - `spawn_actor_with_queue_size(size)`
/// - `spawn_actor_with_name_and_size(name, size)`
///
/// ### Custom Executor (requires `custom-executor` feature)
/// - `spawn_actor_with(name, size, <T as puppet::Executor>)`
///
/// ### Base Methods
/// - `run_actor(puppet::Reply<<Msg as Message>::Output>)`
///
/// ### Example
/// ```ignore
/// use puppet::puppet_actor;
///
/// pub struct MyActor;
///
/// #[puppet_actor]
/// impl MyActor {
///     #[puppet]
///     async fn on_say_hello(&self, msg: SayHello) -> String {
///         format!("Hello, {}!", msg.name)
///     }
/// }
///
/// let actor = MyActor {};
/// let mailbox = actor.spawn_actor().await;
/// ```
pub fn puppet_actor(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let info = parse_macro_input!(item as ItemImpl);
    generate(info)
}

fn generate(mut info: ItemImpl) -> TokenStream {
    let actor_name = if let Type::Path(path) = &*info.self_ty {
        get_segment(path).expect("Get struct name").ident
    } else {
        return Error::new_spanned(info, "Expected macro to be placed on a struct impl.")
            .into_compile_error()
            .into();
    };

    let extracted_generics = maybe_compiler_error!(generics_processor::process_impl_generics(
        info.generics.clone()
    ));
    let extract_handlers = maybe_compiler_error!(handler::create_message_handlers(
        &mut info.items,
        &extracted_generics
    ));

    let types = extract_handlers.handlers
        .iter()
        .map(|(_, msg)| {
            let ty = msg.ty();
            let ident = msg.ident();
            quote! {
                #ident {
                    msg: #ty,
                    tx: puppet::__private::futures::channel::oneshot::Sender<<#ty as puppet::Message>::Output>,
                }
            }
        });

    let enum_name = format_ident!("{}Op", actor_name);
    let enum_traits = extract_handlers
        .handlers
        .iter()
        .map(|(_handler, msg)| EnumFrom {
            name: msg.ident(),
            message: msg.ty(),
            parent: enum_name.clone(),
            generics: extract_handlers.generics.clone(),
        });
    let mut enum_match_statements = Vec::new();
    for (method, message) in extract_handlers.handlers.iter() {
        let callback_name = &method.sig.ident;
        let tokens = match message {
            Extract::Message(msg) => {
                let ident = &msg.ident;
                quote! {
                    Self::#ident { msg, tx } => {
                        let res = actor.#callback_name(msg).await;
                        let _ = tx.send(res);
                    }
                }
            }
            Extract::MessageWithReply(msg, ArgOrder::First) => {
                let ident = &msg.ident;
                quote! {
                    Self::#ident { msg, tx } => {
                        let reply = puppet::Reply::from(tx);
                        // Do not let methods return a type other than ().
                        let () = actor.#callback_name(reply, msg).await;
                    }
                }
            }
            Extract::MessageWithReply(msg, ArgOrder::Second) => {
                let ident = &msg.ident;
                quote! {
                    Self::#ident { msg, tx } => {
                        let reply = puppet::Reply::from(tx);
                        // Do not let methods return a type other than ().
                        let () = actor.#callback_name(msg, reply).await;
                    }
                }
            }
        };

        enum_match_statements.push(tokens);
    }

    let remaining_generics = extract_handlers.generics.remaining_generics();
    let remaining_where = extract_handlers.generics.remaining_where();
    let enum_where = extract_handlers.generics.enum_where();
    let generic_builder = extract_handlers.generics.clone();
    let actor_generics = extracted_generics.types;
    let actor_where = extracted_generics.where_clause;

    let enum_tokens = quote! {
        pub enum #enum_name #generic_builder
        #enum_where
        {
            #(#types),*
        }

        impl #generic_builder #enum_name #generic_builder
        #enum_where
        {
            async fn __run #remaining_generics (self, actor: &mut #actor_name #actor_generics)
            #remaining_where
            {
                match self {
                    #(#enum_match_statements),*
                }
            }
        }

        #(#enum_traits)*
    };

    #[cfg(not(feature = "helper-methods"))]
    let helper_methods = quote! {};

    #[cfg(feature = "helper-methods")]
    let helper_methods = quote! {
        pub async fn spawn_actor(mut self) -> puppet::ActorMailbox<#actor_name #actor_generics> {
            self.spawn_actor_with_queue_size(100).await
        }

        pub async fn spawn_actor_with_name(mut self, name: impl AsRef<str>) -> puppet::ActorMailbox<#actor_name #actor_generics> {
            self.spawn_actor_with_name_and_size(name, 100).await
        }

        pub async fn spawn_actor_with_queue_size(mut self, n: usize) -> puppet::ActorMailbox<#actor_name #actor_generics> {
            self.spawn_actor_with_name_and_size(stringify!(#actor_name), 100).await
        }

        pub async fn spawn_actor_with_name_and_size(mut self, name: impl AsRef<str>, n: usize) -> puppet::ActorMailbox<#actor_name #actor_generics> {
            let (tx, rx) = puppet::__private::flume::bounded::<#enum_name #generic_builder>(n);

            puppet::__private::tokio::spawn(async move {
                while let Ok(op) = rx.recv_async().await {
                    op.__run(&mut self).await;
                }
            });

            let name = std::borrow::Cow::Owned(name.as_ref().to_string());
            puppet::ActorMailbox::new(tx, name)
        }
    };

    #[cfg(not(feature = "custom-executor"))]
    let custom_executor = quote! {};

    #[cfg(feature = "custom-executor")]
    let custom_executor = quote! {
        pub async fn spawn_actor_with(mut self, name: impl AsRef<str>, n: usize, executor: impl puppet::Executor) -> puppet::ActorMailbox<#actor_name #actor_generics> {
            use puppet::Executor;

            let (tx, rx) = puppet::__private::flume::bounded::<#enum_name #generic_builder>(n);

            executor.spawn(async move {
                while let Ok(op) = rx.recv_async().await {
                    op.__run(&mut self).await;
                }
            });

            let name = std::borrow::Cow::Owned(name.as_ref().to_string());
            puppet::ActorMailbox::new(tx, name)
        }
    };

    let tokens = quote! {
        #info

        impl #actor_generics puppet::Actor for #actor_name #actor_generics #actor_where {
            type Messages = #enum_name #generic_builder;
        }

        impl #actor_generics #actor_name #actor_generics #actor_where {
            pub async fn run_actor(mut self, messages: puppet::__private::flume::Receiver<#enum_name #generic_builder>) {
                while let Ok(op) = messages.recv_async().await {
                    op.__run(&mut self).await;
                }
            }

            #custom_executor

            #helper_methods
        }

        #enum_tokens
    };

    tokens.into()
}

struct EnumFrom {
    parent: Ident,
    name: Ident,
    message: Type,
    generics: GenericBuilder,
}

impl ToTokens for EnumFrom {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        let parent = self.parent.clone();
        let name = self.name.clone();
        let message = self.message.clone();
        let generics = self.generics.clone();
        let where_clause = self.generics.enum_where();
        let variant = quote! {
            impl #generics puppet::MessageHandler<#message> for #parent #generics
            #where_clause
            {
                fn create(msg: #message) -> (Self, puppet::__private::futures::channel::oneshot::Receiver<<#message as puppet::Message>::Output>) {
                    let (tx, rx) = puppet::__private::futures::channel::oneshot::channel();

                    let slf = Self::#name { msg, tx };

                    (slf, rx)
                }
            }
        };
        tokens.append_all(variant);
    }
}
