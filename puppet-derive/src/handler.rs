use crate::generics_processor::{create_generic_builder, extract_message, Extract};
use crate::{GenericBuilder, ProcessedGenerics};
use std::mem;
use syn::token::SelfValue;
use syn::{Error, FnArg, ImplItem, ImplItemMethod, ReturnType};

pub struct ExtractedHandlers {
    pub handlers: Vec<(ImplItemMethod, Extract)>,
    pub generics: GenericBuilder,
}

/// Walks through all methods marked as message handlers.
///
/// This extracts the used generics, types, and type of handler.
pub fn create_message_handlers(
    items: &mut Vec<ImplItem>,
    generics: &ProcessedGenerics,
) -> Result<ExtractedHandlers, Error> {
    let handlers = extract_puppet_handlers(items)?;

    let mut generic_builder = create_generic_builder(generics);
    let mut message_callbacks = Vec::new();
    for (handler, kind) in handlers {
        if !is_ref_self(&handler) {
            return Err(Error::new_spanned(
                handler,
                "Message handler must be either `&self` or `&mut self`",
            ));
        }

        let msg = extract_message(&mut generic_builder, &handler, kind)?;
        message_callbacks.push((handler, msg));
    }

    Ok(ExtractedHandlers {
        handlers: message_callbacks,
        generics: generic_builder,
    })
}

/// Extracts the actor handlers marked with the `#[puppet]` attribute.
fn extract_puppet_handlers(
    items: &mut Vec<ImplItem>,
) -> Result<Vec<(ImplItemMethod, HandlerKind)>, Error> {
    let mut message_handlers = Vec::new();
    for tokens in mem::take(items) {
        if let Some(kind) = is_puppet_handler(&tokens) {
            if let ImplItem::Method(method) = tokens.clone() {
                if kind == HandlerKind::WithReply && method.sig.output != ReturnType::Default {
                    return Err(Error::new_spanned(
                        method.sig.output,
                        "Message handlers taking a reply parameter cannot return a value.",
                    ));
                }

                if method.sig.asyncness.is_none() {
                    return Err(Error::new_spanned(
                        method.sig,
                        "Message handlers must be async.",
                    ));
                }

                message_handlers.push((method, kind));
            }
            items.push(remove_puppet_handler(tokens));
        } else {
            // Add the method back.
            items.push(tokens);
        }
    }

    Ok(message_handlers)
}

#[derive(Copy, Clone, Eq, PartialEq)]
pub enum HandlerKind {
    Direct,
    WithReply,
}

impl HandlerKind {
    pub fn expected_argument_count(&self) -> usize {
        match self {
            Self::Direct => 2,
            Self::WithReply => 3,
        }
    }
}

fn is_puppet_handler(tokens: &ImplItem) -> Option<HandlerKind> {
    if let ImplItem::Method(ref method) = tokens {
        for attr in method.attrs.iter() {
            if attr.path.is_ident("puppet") {
                return Some(HandlerKind::Direct);
            } else if attr.path.is_ident("puppet_with_reply") {
                return Some(HandlerKind::WithReply);
            }
        }
    }

    None
}

fn remove_puppet_handler(mut tokens: ImplItem) -> ImplItem {
    if let ImplItem::Method(method) = &mut tokens {
        method.attrs.retain(|attr| {
            !attr.path.is_ident("puppet") && !attr.path.is_ident("puppet_with_reply")
        });
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
