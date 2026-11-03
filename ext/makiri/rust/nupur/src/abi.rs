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
    nupur_sys::non_raising::running_api_version()
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

/// `Err(LoadError)` when the constant `path` names (`["Makiri", "Node"]`,
/// read from `Object`) is already defined - an autoload counting, and
/// nothing loaded: the extension's classes are there before its `Init_` has
/// made them. A `require` an exit cut short leaves them (Ruby marks the
/// feature loaded only when `Init_` returns, so the next `require` runs it
/// again), and so does code that defined one first. Asked in `check`, before
/// anything is defined, it is a `LoadError` the caller may rescue; met in
/// `init`, where `Module::define_data_class` refuses such a name, it would
/// be `fatal`. Name the class the extension's `init` defines first.
pub fn check_undefined(ruby: &crate::rb::Ruby, path: &[&str]) -> Result<(), crate::rb::Error> {
    use crate::rb::{RClass, RModule, ReprValue};
    let Some((last, outer)) = path.split_last() else {
        return Ok(());
    };
    let defined = |at: crate::rb::Value, name: &str| -> Result<bool, crate::rb::Error> {
        let id = crate::rb::Id::intern(ruby, &crate::rb::class::c_name(name)?)?;
        crate::raw::const_defined_at(at, id)
    };
    let mut at = ruby.class_object().as_value();
    for name in outer {
        if !defined(at, name)? {
            return Ok(());
        }
        let v = crate::raw::const_get(ruby, at, &crate::rb::class::c_name(name)?)?;
        if RModule::from_value(v).is_none() && RClass::from_value(v).is_none() {
            /* Not a namespace: `init`'s definition under it refuses it. */
            return Ok(());
        }
        at = v;
    }
    if !defined(at, last)? {
        return Ok(());
    }
    Err(crate::rb::Error::new(
        crate::rb::Builtin::LoadError,
        format_args!(
            "{} is already defined: the extension was loaded before, or other code defined its classes",
            Path(path)
        ),
    ))
}

/// A constant's path, written `A::B`.
struct Path<'a>(&'a [&'a str]);

impl core::fmt::Display for Path<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for (i, name) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str("::")?;
            }
            f.write_str(name)?;
        }
        Ok(())
    }
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
