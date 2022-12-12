use syn::{Error, FnArg, Pat, PathSegment, TypePath};

pub fn get_segment(path: &TypePath) -> Result<PathSegment, Error> {
    path.path.segments.first().cloned().ok_or_else(|| {
        Error::new_spanned(
            path,
            "Expected message type to be part of function signature.",
        )
    })
}

pub fn format_inputs<'a>(inputs: impl Iterator<Item = &'a FnArg>) -> String {
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
