//! What this binary assumes of the Ruby that loads it, and the load checks
//! that refuse a Ruby where an assumption fails, before anything is defined:
//! the Ruby API version it was built against ([`check_running_api`]), and
//! the `rb_protect` state a raise comes back with, which Ruby does not
//! document and the layer tells an exception from a `throw` by
//! ([`check_raise_state`]).

#![forbid(unsafe_code)]

/// The `MAJOR.MINOR` of the Ruby whose headers this extension was compiled
/// against - its ABI, since a teeny release keeps it. Published as a
/// constant by the extension.
pub fn ruby_api_version() -> crate::msg::Msg {
    crate::msg::Msg::format(format_args!(
        "{}.{}",
        nupur_sys::RUBY_API_VERSION_MAJOR,
        nupur_sys::RUBY_API_VERSION_MINOR
    ))
}

/// The Ruby API `MAJOR.MINOR` this extension was compiled against.
pub(crate) fn built_ruby_api() -> (i64, i64) {
    (
        i64::from(nupur_sys::RUBY_API_VERSION_MAJOR),
        i64::from(nupur_sys::RUBY_API_VERSION_MINOR),
    )
}

/// The running Ruby's API `MAJOR.MINOR`, read from the `ruby_api_version`
/// it exports (three ints, whose layout no Ruby changes).
pub(crate) fn running_ruby_api() -> (i64, i64) {
    nupur_sys::read::running_api_version()
}

/// `Err(LoadError)` unless the running Ruby's API `MAJOR.MINOR` is the one
/// this extension was compiled for: the first thing an extension's `Init_`
/// does. A release Ruby does not check `ruby_abi_version` itself, and under
/// macOS's `-undefined dynamic_lookup` a binary built for another Ruby loads
/// and then misreads the interpreter's structs - defining one class could
/// already corrupt it. It reads the running Ruby's exported
/// `ruby_api_version` and nothing else; only a mismatch goes on to build the
/// exception. `name` names the extension in the message, and `rebuild` says
/// how to rebuild it.
pub fn check_running_api(name: &str, rebuild: &str) -> Result<(), crate::rb::Error> {
    let running = running_ruby_api();
    let built = built_ruby_api();
    if running == built {
        return Ok(());
    }
    Err(crate::rb::Error::new(
        crate::rb::Builtin::LoadError,
        format_args!(
            "{name}'s native extension was built for Ruby {}.{}, but this is Ruby {}.{}; \
             rebuild it ({rebuild})",
            built.0, built.1, running.0, running.1
        ),
    ))
}

/// After [`check_running_api`]: `Err(LoadError)` unless a raise made under
/// the layer's protection comes back as an exception, not as some other exit
/// (`raw::raise_is_recognised`, which says why that is not a given) - the
/// layer could not tell an exception from a `throw`. Nothing is defined yet
/// when it refuses.
pub fn check_raise_state(ruby: &crate::rb::Ruby) -> Result<(), crate::rb::Error> {
    if crate::raw::raise_is_recognised(ruby)? {
        return Ok(());
    }
    Err(crate::rb::Error::new(
        crate::rb::Builtin::LoadError,
        "this Ruby reports a raise by a state the extension does not know \
         (`rb_protect`'s TAG_RAISE changed); it cannot tell an exception from a throw",
    ))
}
