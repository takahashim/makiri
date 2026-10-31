//! A `{prefix => uri}` Hash, as every query that takes one reads it: XPath's
//! (a query's Hash and keywords, `XPathContext.new`'s, `#register_namespace`)
//! and CSS's. One reading, so all of them convert, check, cap and word a
//! refusal the same way; what a query does with a binding is the caller's
//! `register`.

#![forbid(unsafe_code)]

use crate::bridge::rb::{Error, RHash, Ruby, Value};

use crate::bridge::ruby::makiri_error;
use crate::bridge::string::ruby_try_verified_text_pair;

/// Bind every `{prefix => uri}` pair of `h` through `register`, each prefix
/// and URI held to `cap` bytes.
///
/// On any bad entry the error is returned and nothing further is registered -
/// the caller's owner then frees what it registered into, so a partial
/// registration is never handed out.
pub fn bind_each(
    ruby: &Ruby,
    h: RHash,
    cap: usize,
    mut register: impl FnMut(&[u8], &[u8]) -> Result<(), Error>,
) -> Result<(), Error> {
    /* More pairs than a context may hold is refused before any is converted
     * or copied, rather than after every one of them has been. */
    let max = crate::xpath::ctx::MAX_NAMESPACES;
    if h.len() > max {
        return Err(makiri_error(format_args!(
            "invalid namespace mapping: {} bindings (max {max})",
            h.len()
        )));
    }
    /* The pairs are copied out first and bound after (`hash::each_pair`). */
    crate::bridge::hash::each_pair(ruby, h, |prefix, uri| {
        bind_pair(ruby, prefix, uri, cap, &mut register)
    })
}

/// Bind one `prefix => uri` pair through `register`.
///
/// The prefix is a String or a Symbol (a keyword binding's key), the URI a
/// String; nothing else converts.
pub fn bind_pair(
    ruby: &Ruby,
    prefix: Value,
    uri: Value,
    cap: usize,
    mut register: impl FnMut(&[u8], &[u8]) -> Result<(), Error>,
) -> Result<(), Error> {
    let ks = crate::bridge::ruby::name_arg(prefix)?;
    let vs = crate::bridge::ruby::string_arg(uri)?;
    let (pv, uv) = ruby_try_verified_text_pair(ruby, ks, vs, cap).map_err(|r| {
        r.into_error(|reason| makiri_error(format_args!("invalid namespace mapping: {reason}")))
    })?;
    register(pv.as_bytes(), uv.as_bytes())
}

/// `(prefix, uri)` pairs, owned.
pub type Bindings = Vec<(Vec<u8>, Vec<u8>)>;

/// `h`'s bindings, read as [`bind_each`] reads them and copied out - for a
/// matcher that takes them as bytes after the Hash is done with (CSS).
pub fn owned(ruby: &Ruby, h: RHash, cap: usize) -> Result<Bindings, Error> {
    use crate::falloc::{try_to_vec, VecGrow};
    let oom = || makiri_error("out of memory reading namespace bindings");
    let mut out = Vec::new();
    bind_each(ruby, h, cap, |prefix, uri| {
        let pair = (
            try_to_vec(prefix).ok_or_else(oom)?,
            try_to_vec(uri).ok_or_else(oom)?,
        );
        out.falloc_push(pair).map_err(|()| oom())
    })?;
    Ok(out)
}
