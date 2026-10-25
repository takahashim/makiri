//! The Ruby helpers that name Makiri: its error classes and its load check.
//! Everything generic - the argument checks, the common exceptions, the
//! raising C API - is `nupur::helpers`, re-exported here so the bridge and
//! the glue keep naming it `bridge::ruby`.

#![forbid(unsafe_code)]

pub use crate::nupur::helpers::*;

use crate::bridge::rb::{Error, Ruby};

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
pub fn check_running_ruby_api(ruby: &Ruby) -> Result<(), Error> {
    let running = crate::nupur::helpers::running_ruby_api();
    let built = crate::nupur::helpers::built_ruby_api();
    if running == built {
        return Ok(());
    }
    Err(Error::new(
        ruby.exception_load_error(),
        format_args!(
            "makiri's native extension was built for Ruby {}.{}, but this is Ruby {}.{}; \
             rebuild it (`bundle exec rake clean compile` in a checkout, or reinstall the gem)",
            built.0, built.1, running.0, running.1
        ),
    ))
}

/// `Makiri::Error` - the one definition; the modules that each kept a private
/// copy of this now import it.
pub fn error_class() -> crate::bridge::rb::ExceptionClass {
    crate::init::EXC_ERROR.exception()
}

/// A `Makiri::Error` carrying `msg` - the one way the extension raises its own
/// error class, so a caller never spells the class (or finds a stale copy of
/// the lookup) itself.
pub fn makiri_error(msg: impl crate::bridge::rb::Message) -> Error {
    Error::new(error_class(), msg)
}
