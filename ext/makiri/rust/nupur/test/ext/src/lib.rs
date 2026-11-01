//! `NupurTest`: a few methods over nupur, each one a guarantee of the layer
//! put where a spec can reach it (`../spec/nupur_spec.rb`). Nothing of
//! Makiri's: this is what an extension written on the layer alone sees.

use core::cell::Cell;

use nupur::args::{integer_arg, string_arg, Kwargs, Shape};
use nupur::encoding::Unconvertible;
use nupur::rb::{
    prelude::*, Builtin, DataClass, Error, ExceptionClass, Float, RArray, RHash, RbConst, Ruby,
    Value,
};
use nupur::string::{ReadError, Transcode};
use nupur::typed::{Birth, Payload, TypedType};
use nupur::{function, method};

/* ---- a wrapped object with a slot ---- */

/// A payload with a counter, and one slot: the value it was made with.
struct Holder {
    bumps: Cell<i64>,
}

impl Payload for Holder {}

static HOLDER: TypedType<Holder, 1> = TypedType::base(c"NupurTest::Holder");
static HOLDER_CLASS: RbConst<DataClass> = RbConst::new();

/// `Holder.new(v)`: an object keeping `v` in its slot.
fn holder_new(ruby: &Ruby, v: Value) -> Result<Value, Error> {
    let class = HOLDER_CLASS.defined(ruby)?;
    let made = HOLDER.wrap(
        ruby,
        Birth {
            klass: class,
            data: Holder {
                bumps: Cell::new(0),
            },
            roots: [v],
        },
    )?;
    Ok(made.value())
}

/// `#held`: the value in the slot.
fn holder_held(_: &Ruby, this: Value) -> Result<Value, Error> {
    Ok(HOLDER.check(this)?.root::<0>())
}

/// `#bump`: one more on the counter, read through the handle; the count.
fn holder_bump(_: &Ruby, this: Value) -> Result<i64, Error> {
    let h = HOLDER.check(this)?;
    let d = h.data();
    d.bumps.set(d.bumps.get() + 1);
    Ok(d.bumps.get())
}

/* ---- errors, panics, exits ---- */

fn echo(_: &Ruby, v: Value) -> Value {
    v
}

fn arg_error(_: &Ruby) -> Result<(), Error> {
    Err(Error::new(
        Builtin::ArgumentError,
        "nupur: a refused argument",
    ))
}

/// A panic in the method's body (0), in a GVL-released body (1), or in a
/// Hash walk's callback (2, over `h`).
#[allow(clippy::panic)]
fn panic_in(ruby: &Ruby, kind: Value, h: Value) -> Result<(), Error> {
    match integer_arg(kind)?.to_i64()? {
        0 => panic!("nupur_test: panic in a method"),
        1 => nupur::gvl::without_gvl(ruby, || panic!("nupur_test: panic without the GVL")),
        _ => {
            let h = RHash::from_value(h).ok_or_else(|| nupur::rb::error::type_error("a Hash"))?;
            nupur::hash::hash_foreach(h, |_, _| panic!("nupur_test: panic in a Hash walk"))
        }
    }
}

/// Yield `0..n` to the block; `n`.
fn yield_each(ruby: &Ruby, n: Value) -> Result<i64, Error> {
    let n = integer_arg(n)?.to_i64()?;
    for i in 0..n {
        nupur::rb::method::yield_value(ruby, nupur::build::int(ruby, i)?.as_value())?;
    }
    Ok(n)
}

/// Yield once and drop what came back: the trampoline must still resume an
/// exit.
fn drop_exit(ruby: &Ruby) {
    nupur::rb::method::yield_dropping_error_for_test(ruby);
}

/* ---- Strings ---- */

fn read_error<P>(e: ReadError<P>) -> Error {
    match e {
        ReadError::Refused(_) => Error::new(Builtin::ArgumentError, "nupur: holds a NUL"),
        ReadError::TooLong => Error::new(Builtin::RangeError, "nupur: too long"),
        ReadError::NotUtf8 => Error::new(Builtin::ArgumentError, "nupur: not UTF-8"),
        ReadError::NoUtf8Form => Error::new(Builtin::ArgumentError, "nupur: no UTF-8 form"),
        ReadError::Oom => Error::new(Builtin::NoMemoryError, "nupur: out of memory"),
        ReadError::Raised(e) => e,
    }
}

/// `s` read as checked UTF-8 with no NUL, and made a new String.
fn read_text(ruby: &Ruby, s: Value) -> Result<Value, Error> {
    let s = string_arg(s)?;
    let copy =
        nupur::string::read_utf8(ruby, s, |b| b.contains(&0).then_some(())).map_err(read_error)?;
    Ok(nupur::string::utf8_str(ruby, copy.as_str().as_bytes())?.as_value())
}

/// `s` read under a cap of `max` bytes of UTF-8.
fn read_capped(ruby: &Ruby, s: Value, max: Value) -> Result<i64, Error> {
    let s = string_arg(s)?;
    let max = integer_arg(max)?.to_u64()?;
    let max = usize::try_from(max).unwrap_or(usize::MAX);
    let copy = nupur::string::read_utf8_within(ruby, s, max, |_| None::<()>).map_err(read_error)?;
    Ok(i64::try_from(copy.as_str().len()).unwrap_or(i64::MAX))
}

/// What `to_utf8` made of `s`, strictly: `:as_is`, `:transcoded` or
/// `:fault`.
fn transcode_kind(ruby: &Ruby, s: Value) -> Result<Value, Error> {
    let s = string_arg(s)?;
    let kind = match nupur::string::to_utf8(ruby, s, None, Unconvertible::Raise)? {
        Transcode::AsIs(_) => "as_is",
        Transcode::Transcoded(_) => "transcoded",
        Transcode::Fault(_) => "fault",
    };
    Ok(nupur::build::symbol(ruby, kind)?.as_value())
}

/// A String of `n` bytes written in place.
fn filled(ruby: &Ruby, n: Value) -> Result<Value, Error> {
    let n = usize::try_from(integer_arg(n)?.to_u64()?).unwrap_or(0);
    let s = nupur::string::utf8_str_filled(ruby, n, |out| {
        for _ in 0..n {
            out.push(b"a");
        }
        Ok(())
    })?;
    Ok(s.as_value())
}

/// `"abcd"` from two slices, or (`short`) a total the slices fall short of,
/// which is an internal error.
fn slices(ruby: &Ruby, short: bool) -> Result<Value, Error> {
    let parts: [&[u8]; 2] = [b"ab", b"cd"];
    let total = if short { 5 } else { 4 };
    Ok(nupur::string::utf8_str_from_slices(ruby, parts.into_iter(), total)?.as_value())
}

/* ---- arguments, Hashes, the GVL ---- */

/// `kw(a: x)`: keyword `a`, or nil.
fn kw(ruby: &Ruby, args: &[Value]) -> Result<Value, Error> {
    Ok(Kwargs::scan(ruby, args)?
        .value(ruby, "a")?
        .unwrap_or_else(|| ruby.qnil()))
}

/// One required argument and one optional: how many were given.
fn shape(ruby: &Ruby, args: &[Value]) -> Result<i64, Error> {
    let a = nupur::args::scan(ruby, args, Shape::positional(1, 1))?;
    Ok(if a.optional(0).is_some() { 2 } else { 1 })
}

/// `h`'s pairs, as an Array of `[k, v]`.
fn pairs(ruby: &Ruby, h: Value) -> Result<Value, Error> {
    let h = RHash::from_value(h).ok_or_else(|| nupur::rb::error::type_error("a Hash"))?;
    let out: RArray = nupur::build::ary(ruby, h.len())?;
    nupur::hash::each_pair(ruby, h, |k, v| {
        let pair = nupur::build::ary(ruby, 2)?;
        nupur::build::push(pair, k)?;
        nupur::build::push(pair, v)?;
        nupur::build::push(out, pair)
    })?;
    Ok(out.as_value())
}

/// `0 + 1 + ... + (n - 1)`, summed with the GVL released.
fn sum_without_gvl(ruby: &Ruby, n: Value) -> Result<u64, Error> {
    let n = integer_arg(n)?.to_u64()?;
    nupur::gvl::without_gvl(ruby, move || (0..n).sum::<u64>())
}

/// `Ok` unfrozen, Ruby's `FrozenError` frozen.
fn frozen_check(_: &Ruby, v: Value) -> Result<(), Error> {
    v.check_frozen()
}

/* ---- immediates and numbers ---- */

/// `[true, false]`, from `Ruby::qtrue` and `Ruby::qfalse`.
fn booleans(ruby: &Ruby) -> Result<Value, Error> {
    let out: RArray = nupur::build::ary(ruby, 2)?;
    nupur::build::push(out, ruby.qtrue())?;
    nupur::build::push(out, ruby.qfalse())?;
    Ok(out.as_value())
}

/// `n` through `Integer::to_usize`: a negative `n`, or one past a `usize`,
/// is `RangeError`.
fn usize_of(_: &Ruby, n: Value) -> Result<usize, Error> {
    integer_arg(n)?.to_usize()
}

/// The bits of `x` through `Float::to_f64`, so the spec compares them
/// exactly (a NaN, a -0.0).
fn f64_bits_of(_: &Ruby, x: Value) -> Result<u64, Error> {
    let x = Float::from_value(x).ok_or_else(|| nupur::rb::error::type_error("a Float"))?;
    Ok(x.to_f64().to_bits())
}

/* ---- Init ---- */

static INTERNAL: RbConst<ExceptionClass> = RbConst::new();

fn check(ruby: &Ruby) -> Result<(), Error> {
    nupur::abi::check_running_api("nupur_test", "rake nupur:spec")?;
    nupur::abi::check_raise_state(ruby)
}

fn init(ruby: &Ruby) -> Result<(), Error> {
    /* Every method below may be called from any Ractor: the specs run some
     * in several at once. */
    nupur::rb::method::declare_ractor_safe(ruby);
    let m = ruby.define_module("NupurTest")?;
    let internal = m.define_error("InternalError", ruby.exception_exception())?;
    INTERNAL.set(internal)?;
    nupur::rb::error::INTERNAL_ERROR.set(internal)?;

    let holder = m.define_data_class("Holder", None)?;
    HOLDER_CLASS.set(holder)?;
    holder.define_singleton_method("new", function!(holder_new, 1))?;
    holder.define_method("held", method!(holder_held, 0))?;
    holder.define_method("bump", method!(holder_bump, 0))?;

    m.define_module_function("echo", function!(echo, 1))?;
    m.define_module_function("arg_error", function!(arg_error, 0))?;
    m.define_module_function("panic_in", function!(panic_in, 2))?;
    m.define_module_function("yield_each", function!(yield_each, 1))?;
    m.define_module_function("drop_exit", function!(drop_exit, 0))?;
    m.define_module_function("read_text", function!(read_text, 1))?;
    m.define_module_function("read_capped", function!(read_capped, 2))?;
    m.define_module_function("transcode_kind", function!(transcode_kind, 1))?;
    m.define_module_function("filled", function!(filled, 1))?;
    m.define_module_function("slices", function!(slices, 1))?;
    m.define_module_function("kw", function!(kw, -1))?;
    m.define_module_function("shape", function!(shape, -1))?;
    m.define_module_function("pairs", function!(pairs, 1))?;
    m.define_module_function("sum_without_gvl", function!(sum_without_gvl, 1))?;
    m.define_module_function("frozen_check", function!(frozen_check, 1))?;
    m.define_module_function("booleans", function!(booleans, 0))?;
    m.define_module_function("usize_of", function!(usize_of, 1))?;
    m.define_module_function("f64_bits_of", function!(f64_bits_of, 1))?;
    Ok(())
}

#[no_mangle]
#[allow(non_snake_case)]
pub extern "C" fn Init_nupur_test() {
    // SAFETY: Ruby calls `Init_nupur_test` on its thread, with the GVL held.
    unsafe { nupur::rb::method::run_init(check, init) };
}
