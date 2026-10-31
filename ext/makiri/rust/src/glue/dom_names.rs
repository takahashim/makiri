//! The DOM's naming rules as both representations' `*_ns` factories and
//! setters raise them: one wording, so `Makiri::HTML` and `Makiri::XML` refuse
//! a name or a namespace alike.

#![forbid(unsafe_code)]

use crate::bridge::rb::Error;

use crate::bridge::ruby::makiri_error;
use crate::xml::dom_name;

/// The refusal of a namespace that does not fit the qualified name it came with.
const NS_MISFIT: &str =
    "the namespace does not fit the qualified name (a prefix needs a namespace; \
xml and xmlns take only their own)";

/// [`dom_name::validate_and_extract`] with its refusals as the `*_ns` methods
/// of both representations raise them: `ArgumentError` for a name ("invalid
/// `what` name"), `Makiri::Error` for a namespace. Returns the name split,
/// its prefix empty when there is none.
pub fn dom_extract<'q>(
    ns: &[u8],
    qname: &'q [u8],
    local_ok: fn(&[u8]) -> bool,
    what: &str,
) -> Result<dom_name::QName<'q>, Error> {
    dom_name::validate_and_extract(ns, qname, local_ok).map_err(|e| match e {
        dom_name::ExtractError::Name => Error::new(
            crate::bridge::rb::Builtin::ArgumentError,
            format_args!("invalid {what} name"),
        ),
        dom_name::ExtractError::Namespace => makiri_error(NS_MISFIT),
    })
}
