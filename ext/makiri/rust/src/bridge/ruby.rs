//! The Ruby helpers that name Makiri: its error classes and its load check.
//! Everything generic is `nupur`'s - the argument checks (`args`), the
//! messages' value text (`describe`), the common errors and values (`rb`) -
//! re-exported here so the bridge and the glue keep naming it `bridge::ruby`.

#![forbid(unsafe_code)]

pub use crate::nupur::args::{index_arg, integer_arg, name_arg, range_beg_len, string_arg};
pub use crate::nupur::describe::{AsText, Describe, ExceptionText};
pub use crate::nupur::rb::error::{arg_error, internal_error, type_error};
pub use crate::nupur::rb::global::is_kind_of;
pub use crate::nupur::rb::value::{boolean, check_frozen, identity_word, nil, same_value};

use crate::bridge::rb::Error;

/// A `Makiri::Error` carrying `msg` - the one way the extension raises its own
/// error class, so a caller never spells the class (or finds a stale copy of
/// the lookup) itself.
pub fn makiri_error(msg: impl crate::bridge::rb::Message) -> Error {
    Error::new(&crate::init::EXC_ERROR, msg)
}
