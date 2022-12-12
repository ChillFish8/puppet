use crate::handler::HandlerKind;
use crate::utils::{format_inputs, get_segment};
use proc_macro2::Ident;
use quote::{quote, ToTokens, TokenStreamExt};
use std::collections::{HashMap, HashSet};
use std::mem;
use syn::punctuated::Punctuated;
use syn::{
    Error, FnArg, GenericArgument, GenericParam, Generics, ImplItemMethod, Lifetime, Path,
    PathArguments, PathSegment, PredicateType, ReturnType, Type, TypeParam, TypePath, WhereClause,
    WherePredicate,
};

static REPLY_TYPE: &str = "Reply";

pub struct ProcessedGenerics {
    pub types: Generics,
    pub where_clause: WhereClause,
}

/// Extracts the generics from the impl of a struct and moves any constraints
/// to the where clause.
pub fn process_impl_generics(mut generics: Generics) -> Result<ProcessedGenerics, Error> {
    let mut actor_generics = generics.clone();
    let mut actor_where = generics.make_where_clause().clone();

    remove_bounds_from_generics(&mut actor_generics, &mut actor_where)?;

    Ok(ProcessedGenerics {
        types: actor_generics,
        where_clause: actor_where,
    })
}

fn remove_bounds_from_generics(
    generics: &mut Generics,
    where_clause: &mut WhereClause,
) -> Result<(), Error> {
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
                return Err(Error::new_spanned(
                    param,
                    "Puppet actors cannot support lifetime generics.",
                ))
            }
            GenericParam::Const(_) => {
                return Err(Error::new_spanned(
                    param,
                    "Puppet actors cannot support const generics.",
                ))
            }
        }
    }
    Ok(())
}

#[derive(Clone)]
pub struct GenericBuilder {
    types_lookup: HashMap<Ident, TypeParam>,
    built_types: HashMap<Ident, TypeParam>,
    remaining_generics: HashSet<Ident>,
    parent_where: WhereClause,
}

impl GenericBuilder {
    pub fn check_and_insert_type(&mut self, i: &Ident) {
        let ty = match self.types_lookup.get(i) {
            None => return,
            Some(ty) => ty,
        };

        self.remaining_generics.remove(i);
        self.built_types.insert(i.clone(), ty.clone());
    }

    pub fn remaining_generics(&self) -> RemainingGenerics {
        RemainingGenerics(self.clone())
    }

    pub fn remaining_where(&self) -> WhereClause {
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

    pub fn enum_where(&self) -> WhereClause {
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

pub struct RemainingGenerics(GenericBuilder);
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

/// Creates a new generic builder.
///
/// The builder is designed to track what generics are used by messages
/// and return type and what are just part of the impl itself.
///
/// This lets us keep the generics not used for messages and inter-mix them.
pub fn create_generic_builder(generics: &ProcessedGenerics) -> GenericBuilder {
    let mut builder = GenericBuilder {
        types_lookup: Default::default(),
        built_types: Default::default(),
        remaining_generics: Default::default(),
        parent_where: generics.where_clause.clone(),
    };

    for generic in generics.types.params.iter() {
        if let GenericParam::Type(ty) = generic {
            builder.remaining_generics.insert(ty.ident.clone());
            builder.types_lookup.insert(ty.ident.clone(), ty.clone());
        }
    }

    builder
}

pub struct ExtractedMessage {
    pub ident: Ident,
    pub ty: Type,
}

pub enum ArgOrder {
    First = 0,
    Second = 1,
}

pub enum Extract {
    Message(ExtractedMessage),
    MessageWithReply(ExtractedMessage, ArgOrder),
}

impl Extract {
    pub fn ident(&self) -> Ident {
        match self {
            Self::Message(m) => m.ident.clone(),
            Self::MessageWithReply(m, _) => m.ident.clone(),
        }
    }

    pub fn ty(&self) -> Type {
        match self {
            Self::Message(m) => m.ty.clone(),
            Self::MessageWithReply(m, _) => m.ty.clone(),
        }
    }
}

/// Extracts the message type from the handler.
pub fn extract_message(
    builder: &mut GenericBuilder,
    method: &ImplItemMethod,
    kind: HandlerKind,
) -> Result<Extract, Error> {
    if method.sig.inputs.len() > kind.expected_argument_count() {
        return Err(Error::new_spanned(
            method,
            format!(
                "Message handler can only support accepting `self` and a single message parameter. Got: {}",
                format_inputs(method.sig.inputs.iter().skip(2))
            )
        ));
    }

    if kind == HandlerKind::Direct {
        if let Some(FnArg::Typed(arg)) = method.sig.inputs.iter().nth(1) {
            return if let Type::Path(path) = &*arg.ty {
                let v = try_get_message_ident(builder, path)?;
                Ok(Extract::Message(ExtractedMessage {
                    ident: v,
                    ty: *arg.ty.clone(),
                }))
            } else {
                Err(Error::new_spanned(
                    method,
                    "Only concrete types or generics are supported by the system. \
                    impl trait blocks and others are not.",
                ))
            };
        }

        return Err(Error::new_spanned(
            method,
            "Expected message argument to be present.",
        ));
    }

    let mut iter = method.sig.inputs.iter().skip(1).take(2).enumerate();
    let mut message_argument = None;
    let mut reply_argument = None;

    while let Some((pos, FnArg::Typed(arg))) = iter.next() {
        if let Type::Path(path) = &*arg.ty {
            if is_reply_type(path)? {
                reply_argument = if pos == 0 {
                    Some(ArgOrder::First)
                } else {
                    Some(ArgOrder::Second)
                };
                continue;
            }

            let v = try_get_message_ident(builder, path)?;
            message_argument = Some(ExtractedMessage {
                ident: v,
                ty: *arg.ty.clone(),
            })
        } else {
            return Err(Error::new_spanned(
                method,
                "Only concrete types or generics are supported by the system. \
                impl trait blocks and others are not.",
            ));
        }
    }

    if reply_argument.is_none() || message_argument.is_none() {
        return Err(Error::new_spanned(
            method,
            "Expected message argument to be present and a reply argument, \
             hint: reply parameter must take `puppet::Reply<T>` type.",
        ));
    }

    return Ok(Extract::MessageWithReply(
        message_argument.unwrap(),
        reply_argument.unwrap(),
    ));
}

fn is_reply_type(path: &TypePath) -> Result<bool, Error> {
    Ok(get_segment(path)?.ident == REPLY_TYPE)
}

fn try_get_message_ident(builder: &mut GenericBuilder, path: &TypePath) -> Result<Ident, Error> {
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
) -> Result<(), Error> {
    match argument {
        GenericArgument::Type(ty) => {
            walk_through_generics(builder, ty)?;
        }
        _ => {
            return Err(Error::new_spanned(
                argument,
                "Unsupported generics in message handler signature.",
            ))
        }
    }

    Ok(())
}

/// Walks through all of the generics for a given base type and ensures that they
/// are added to the generic builder if applicable.
fn walk_through_generics(builder: &mut GenericBuilder, ty: &Type) -> Result<(), Error> {
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
            Type::TraitObject(_) => {}
            Type::Tuple(tuple) => {
                for ty in tuple.elems.iter() {
                    types_queue.push(ty.clone());
                }
            }
            Type::Verbatim(_) => {}
            Type::ImplTrait(_) => {
                return Err(Error::new_spanned(
                    ty,
                    "Message handlers do not support impl trait types.",
                ))
            }
            Type::Infer(_) => {
                return Err(Error::new_spanned(
                    ty,
                    "Message handlers do not support inferred types.",
                ))
            }
            Type::Macro(_) => {
                return Err(Error::new_spanned(
                    ty,
                    "Message handlers do not support macros in the arguments.",
                ))
            }
            Type::Slice(_) => {
                return Err(Error::new_spanned(
                    ty,
                    "Slices are not supported by puppet in message handlers.",
                ))
            }
            _ => {}
        };
    }

    Ok(())
}

fn check_lifetime_static(lifetime: Option<&Lifetime>) -> Result<(), Error> {
    if let Some(lt) = lifetime {
        if lt.ident != "static" {
            return Err(Error::new_spanned(
                lt,
                "Only `&'static` lifetimes are supported in generic message payloads.",
            ));
        }
    }
    Ok(())
}
