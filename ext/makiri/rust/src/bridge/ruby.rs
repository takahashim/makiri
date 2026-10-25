//! The Ruby helpers that name Makiri: its error classes and its load check.
//! Everything generic is `nupur`'s - the argument checks (`args`), the
//! messages' value text (`describe`), the common errors and values (`rb`) -
//! re-exported here so the bridge and the glue keep naming it `bridge::ruby`.

#![forbid(unsafe_code)]

pub use crate::nupur::args::{integer_arg, integer_range_arg, long_arg, name_arg, string_arg};
pub use crate::nupur::describe::{AsText, Describe, ExceptionText};
pub use crate::nupur::raw::{check_frozen, range_beg_len};
pub use crate::nupur::rb::error::{arg_error, internal_error, type_error};
pub use crate::nupur::rb::global::is_kind_of;
pub use crate::nupur::rb::value::{boolean, identity_word, nil, same_value};

use crate::bridge::rb::Error;

/// `Err(LoadError)` when the running Ruby's API `MAJOR.MINOR` is not the one
/// this extension was compiled for.
///
/// The FIRST thing `Init_makiri` does. A release Ruby does not check
/// `ruby_abi_version` itself, and under macOS's `-undefined dynamic_lookup`
/// a binary built for another Ruby loads and then misreads the interpreter's
/// structs - defining one class could already corrupt it, so the check
/// cannot wait for `lib/makiri.rb`. It reads the running Ruby's exported
/// `ruby_api_version` (three ints, whose layout no Ruby changes) and nothing
/// else; only a mismatch goes on to build the exception.
pub fn check_running_ruby_api() -> Result<(), Error> {
    let running = crate::nupur::abi::running_ruby_api();
    let built = crate::nupur::abi::built_ruby_api();
    if running == built {
        return Ok(());
    }
    Err(Error::new(
        crate::bridge::rb::Builtin::LoadError,
        format_args!(
            "makiri's native extension was built for Ruby {}.{}, but this is Ruby {}.{}; \
             rebuild it (`bundle exec rake clean compile` in a checkout, or reinstall the gem)",
            built.0, built.1, running.0, running.1
        ),
    ))
}

/// A `Makiri::Error` carrying `msg` - the one way the extension raises its own
/// error class, so a caller never spells the class (or finds a stale copy of
/// the lookup) itself.
pub fn makiri_error(msg: impl crate::bridge::rb::Message) -> Error {
    Error::new(&crate::init::EXC_ERROR, msg)
}
