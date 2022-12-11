extern crate proc_macro;

use proc_macro::TokenStream;
use proc_macro2::Ident;
use quote::{format_ident, quote, ToTokens, TokenStreamExt};
use std::collections::{HashMap, HashSet};
use std::mem;
use syn::punctuated::Punctuated;
use syn::token::SelfValue;
use syn::{
    parse_macro_input, Error, FnArg, GenericArgument, GenericParam, Generics, ImplItem,
    ImplItemMethod, ItemImpl, Lifetime, Pat, Path, PathArguments, PathSegment, PredicateType,
    ReturnType, Type, TypeParam, TypePath, WhereClause, WherePredicate,
};

macro_rules! bail {
    ($msg:expr) => {{
        return Err($msg.to_string());
    }};
}

#[proc_macro_attribute]
/// Create an actor for the given struct Impl.
///
/// This allows you to mark methods as message handlers with the `#[puppet]` attribute.
/// Once marked, the first parameter after one of `&self` or `&mut self` will be the message to
/// receive.
///
/// Once the actor has been derived, you can spawn it with the `spawn_actor`
/// or `spawn_actor_with_queue_size(n)` method. By default the queue size is `100` messages.
///
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
    let mut actor_generics = info.generics.clone();
    let mut actor_where = info.generics.make_where_clause().clone();
    if let Err(e) = remove_bounds_from_generics(&mut actor_generics, &mut actor_where) {
        return Error::new_spanned(info, e).into_compile_error().into();
    }

    let mut message_handlers = Vec::new();
    for tokens in mem::take(&mut info.items) {
        if is_puppet_handler(&tokens) {
            if let ImplItem::Method(method) = tokens.clone() {
                if method.sig.asyncness.is_none() {
                    return Error::new_spanned(method, "Message handlers must be async.")
                        .into_compile_error()
                        .into();
                }

                message_handlers.push(method);
            }
            info.items.push(remove_puppet_handler(tokens));
        } else {
            // Add the method back.
            info.items.push(tokens);
        }
    }

    let mut generic_builder = create_generic_builder(&actor_generics, &actor_where);
    let mut message_callbacks = Vec::new();
    for handler in message_handlers {
        if !is_ref_self(&handler) {
            return Error::new_spanned(
                handler,
                "Message handler must be either `&self` or `&mut self`",
            )
            .into_compile_error()
            .into();
        }

        let (msg, types) = match extract_message(&mut generic_builder, &handler) {
            Ok(message) => message,
            Err(e) => return Error::new_spanned(handler, e).into_compile_error().into(),
        };

        message_callbacks.push((handler, msg, types));
    }

    let types = message_callbacks
        .iter()
        .cloned()
        .map(|(_, msg, ty)| {
            quote! {
                #msg {
                    msg: #ty,
                    tx: puppet::__private::futures::channel::oneshot::Sender<<#ty as puppet::Message>::Output>,
                }
            }
        });

    let enum_name = format_ident!("{}Op", actor_name);
    let enum_traits = message_callbacks
        .iter()
        .cloned()
        .map(|(_handler, msg, ty)| EnumFrom {
            name: msg,
            message: ty,
            parent: enum_name.clone(),
            generics: generic_builder.clone(),
        });
    let enum_variants = message_callbacks.iter().map(|v| v.1.clone());
    let enum_caller = message_callbacks.iter().map(|v| v.0.sig.ident.clone());

    let remaining_generics = generic_builder.remaining_generics();
    let remaining_where = generic_builder.remaining_where();
    let enum_where = generic_builder.enum_where();
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
                    #(
                        Self::#enum_variants { msg, tx } => {
                            let res = actor.#enum_caller(msg).await;
                            let _ = tx.send(res);
                        }
                    ),*
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

            #helper_methods
        }

        #enum_tokens
    };

    tokens.into()
}

fn remove_bounds_from_generics(
    generics: &mut Generics,
    where_clause: &mut WhereClause,
) -> Result<(), String> {
    for param in generics.params.iter_mut() {
        match param {
            GenericParam::Type(ty) => {
                let mut segments = Punctuated::new();
                segments.push(PathSegment {
                    ident: ty.ident.clone(),
                    arguments: PathArguments::None,
                });

                let mut bounds = Punctuated::new();
                for bound in mem::take(&mut ty.bounds) {
                    bounds.push(bound);
                }

                let predicate = WherePredicate::Type(PredicateType {
                    lifetimes: None,
                    bounded_ty: Type::Path(TypePath {
                        qself: None,
                        path: Path {
                            leading_colon: None,
                            segments,
                        },
                    }),
                    colon_token: Default::default(),
                    bounds,
                });
                where_clause.predicates.push(predicate);
            }
            GenericParam::Lifetime(_) => {
                bail!("Puppet actors cannot support lifetime generics.");
            }
            GenericParam::Const(_) => {
                bail!("Puppet actors cannot support const generics.");
            }
        }
    }
    Ok(())
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

fn is_puppet_handler(tokens: &ImplItem) -> bool {
    if let ImplItem::Method(ref method) = tokens {
        for attr in method.attrs.iter() {
            if attr.path.is_ident("puppet") {
                return true;
            }
        }
    }

    false
}

fn remove_puppet_handler(mut tokens: ImplItem) -> ImplItem {
    if let ImplItem::Method(method) = &mut tokens {
        method.attrs.retain(|attr| !attr.path.is_ident("puppet"));
    }
    tokens
}

fn is_ref_self(method: &ImplItemMethod) -> bool {
    if let Some(FnArg::Receiver(receiver)) = method.sig.inputs.first() {
        if receiver.self_token != SelfValue::default() {
            return false;
        }

        return receiver.reference.is_some();
    }

    false
}

fn extract_message(
    builder: &mut GenericBuilder,
    method: &ImplItemMethod,
) -> Result<(Ident, Type), String> {
    if method.sig.inputs.len() >= 3 {
        bail!(format!(
            "Message handler can only support accepting `self` and a single message parameter. Got: {}",
            format_inputs(method.sig.inputs.iter().skip(2))
        ));
    }

    if let Some(FnArg::Typed(arg)) = method.sig.inputs.iter().nth(1) {
        if let Type::Path(path) = &*arg.ty {
            let v = check_message_type(builder, path)?;
            return Ok((v, *arg.ty.clone()));
        } else {
            bail!(
                "Only concrete types or generics are supported by the system. \
                impl trait blocks and others are not."
            );
        }
    }

    bail!("Expected message argument to be present.")
}

fn check_message_type(builder: &mut GenericBuilder, path: &TypePath) -> Result<Ident, String> {
    let segment = get_segment(path)?;

    if let PathArguments::AngleBracketed(arguments) = segment.arguments {
        for arg in arguments.args.iter() {
            check_valid_generic(builder, arg)?;
        }
    }

    Ok(segment.ident)
}

fn check_valid_generic(
    builder: &mut GenericBuilder,
    argument: &GenericArgument,
) -> Result<(), String> {
    match argument {
        GenericArgument::Type(ty) => {
            walk_through_generics(builder, ty)?;
        }
        _ => bail!("Unsupported generics in message handler signature."),
    }

    Ok(())
}

/// Walks through all of the generics for a given base type and ensures that they
/// are added to the generic builder if applicable.
fn walk_through_generics(builder: &mut GenericBuilder, ty: &Type) -> Result<(), String> {
    let types_queue = crossbeam_queue::SegQueue::new();
    types_queue.push(ty.clone());

    while let Some(ty) = types_queue.pop() {
        match ty {
            Type::Array(t) => {
                types_queue.push(*t.elem);
            }
            Type::BareFn(t) => {
                for lt in t.lifetimes.unwrap_or_default().lifetimes {
                    check_lifetime_static(Some(&lt.lifetime))?;
                }

                for arg in t.inputs {
                    types_queue.push(arg.ty);
                }

                match t.output {
                    ReturnType::Default => {}
                    ReturnType::Type(_, ty) => {
                        types_queue.push(*ty);
                    }
                }
            }
            Type::Group(t) => {
                types_queue.push(*t.elem);
            }
            Type::ImplTrait(_) => bail!("Message handlers do not support impl trait types."),
            Type::Infer(_) => bail!("Message handlers do not support inferred types."),
            Type::Macro(_) => bail!("Message handlers do not support macros in the arguments."),
            Type::Never(_) => {}
            Type::Paren(t) => {
                types_queue.push(*t.elem);
            }
            Type::Path(t) => {
                for segment in t.path.segments {
                    builder.check_and_insert_type(&segment.ident);

                    if let PathArguments::AngleBracketed(generics) = segment.arguments {
                        for arg in generics.args {
                            if let GenericArgument::Type(ty) = arg {
                                types_queue.push(ty);
                            }
                        }
                    }
                }
            }
            Type::Ptr(t) => {
                types_queue.push(*t.elem);
            }
            Type::Reference(t) => {
                check_lifetime_static(t.lifetime.as_ref())?;
                types_queue.push(*t.elem);
            }
            Type::Slice(_) => bail!("Slices are not supported by puppet in message handlers."),
            Type::TraitObject(_) => {}
            Type::Tuple(tuple) => {
                for ty in tuple.elems.iter() {
                    types_queue.push(ty.clone());
                }
            }
            Type::Verbatim(_) => {}
            _ => {}
        };
    }

    Ok(())
}

fn check_lifetime_static(lifetime: Option<&Lifetime>) -> Result<(), String> {
    if let Some(lt) = lifetime {
        if lt.ident != "static" {
            bail!("Only `&'static` lifetimes are supported in generic message payloads.")
        }
    } else {
        bail!("Only `&'static` lifetimes are supported in generic message payloads.")
    }
    Ok(())
}

#[derive(Clone)]
struct GenericBuilder {
    types_lookup: HashMap<Ident, TypeParam>,
    built_types: HashMap<Ident, TypeParam>,
    remaining_generics: HashSet<Ident>,
    parent_where: WhereClause,
}

impl GenericBuilder {
    fn check_and_insert_type(&mut self, i: &Ident) {
        let ty = match self.types_lookup.get(i) {
            None => return,
            Some(ty) => ty,
        };

        self.remaining_generics.remove(i);
        self.built_types.insert(i.clone(), ty.clone());
    }

    fn remaining_generics(&self) -> RemainingGenerics {
        RemainingGenerics(self.clone())
    }

    fn remaining_where(&self) -> WhereClause {
        let where_ = self.parent_where.clone();
        let mut clause = WhereClause {
            where_token: Default::default(),
            predicates: Punctuated::new(),
        };
        for pred in where_.predicates.iter() {
            if let WherePredicate::Type(ty) = &pred {
                if let Type::Path(path) = &ty.bounded_ty {
                    if let Some(generic) = path.path.segments.first() {
                        if self.remaining_generics.contains(&generic.ident) {
                            clause.predicates.push(pred.clone());
                        }
                    }
                }
            }
        }
        clause
    }

    fn enum_where(&self) -> WhereClause {
        let where_ = self.parent_where.clone();
        let mut clause = WhereClause {
            where_token: Default::default(),
            predicates: Punctuated::new(),
        };
        for pred in where_.predicates.iter() {
            if let WherePredicate::Type(ty) = &pred {
                if let Type::Path(path) = &ty.bounded_ty {
                    if let Some(generic) = path.path.segments.first() {
                        if self.built_types.contains_key(&generic.ident) {
                            clause.predicates.push(pred.clone());
                        }
                    }
                }
            }
        }
        clause
    }
}

impl ToTokens for GenericBuilder {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        let generics = self.built_types.keys().cloned();
        let stream = quote! {
            <#(#generics),*>
        };
        tokens.append_all(stream);
    }
}

struct RemainingGenerics(GenericBuilder);
impl ToTokens for RemainingGenerics {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        let mut generics = Vec::new();
        for remaining in self.0.remaining_generics.iter() {
            if let Some(ty) = self.0.types_lookup.get(remaining) {
                generics.push(ty.clone());
            }
        }
        let stream = quote! {
            <#(#generics),*>
        };
        tokens.append_all(stream);
    }
}

fn create_generic_builder(valid_generics: &Generics, where_clause: &WhereClause) -> GenericBuilder {
    let mut builder = GenericBuilder {
        types_lookup: Default::default(),
        built_types: Default::default(),
        remaining_generics: Default::default(),
        parent_where: where_clause.clone(),
    };

    for generic in valid_generics.params.iter() {
        if let GenericParam::Type(ty) = generic {
            builder.remaining_generics.insert(ty.ident.clone());
            builder.types_lookup.insert(ty.ident.clone(), ty.clone());
        }
    }

    builder
}

fn get_segment(path: &TypePath) -> Result<PathSegment, String> {
    path.path
        .segments
        .first()
        .cloned()
        .ok_or_else(|| "Expected message type to be part of function signature.".to_string())
}

fn format_inputs<'a>(inputs: impl Iterator<Item = &'a FnArg>) -> String {
    let mut args = Vec::new();

    for input in inputs {
        if let FnArg::Typed(arg) = &input {
            if let Pat::Ident(ident) = &*arg.pat {
                args.push(ident.ident.to_string());
            }
        }
    }

    args.join(", ")
}

#[cfg(test)]
mod tests {
    use syn::{parse_str, ItemImpl};

    #[test]
    fn test_parse() {
        let input = r#"
        impl<T: Clone + Bar> Foo<T>
        {
        }
        "#;

        let parsed: ItemImpl = parse_str(input).unwrap();
        dbg!(parsed);
    }
}
