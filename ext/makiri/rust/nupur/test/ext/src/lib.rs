//! `NupurTest`: a few methods over nupur, each one a guarantee of the layer
//! put where a spec can reach it (`../spec/nupur_spec.rb`). Nothing of
//! Makiri's: this is what an extension written on the layer alone sees.

use core::cell::Cell;

use nupur::args::{integer_arg, string_arg, Kwargs, Shape};
use nupur::encoding::Unconvertible;
use nupur::rb::{
    prelude::*, Builtin, DataClass, Error, ExceptionClass, Float, RArray, RHash, RbConst, Ruby,
    Symbol, Value,
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

/// Raises Ruby's own class `n` of the builtins past the first ten.
fn raise_builtin(_: &Ruby, n: Value) -> Result<Value, Error> {
    let class = match integer_arg(n)?.to_u64()? {
        0 => Builtin::KeyError,
        1 => Builtin::IndexError,
        2 => Builtin::FrozenError,
        3 => Builtin::ThreadError,
        4 => Builtin::StopIteration,
        _ => Builtin::ZeroDivisionError,
    };
    Err(Error::new(class, "nupur: raised by name"))
}

/// `s` read as checked UTF-8 with no NUL, and made a new String.
fn read_text(ruby: &Ruby, s: Value) -> Result<Value, Error> {
    let s = string_arg(s)?;
    let copy =
        nupur::string::read_utf8(ruby, s, |b| b.contains(&0).then_some(())).map_err(read_error)?;
    Ok(nupur::string::utf8_str(ruby, copy.as_str())?.as_value())
}

/// `s`'s bytes, whatever they are, made a UTF-8 String unchecked.
fn trusting_str(ruby: &Ruby, s: Value) -> Result<Value, Error> {
    let bytes = nupur::string::read_bytes(string_arg(s)?)?;
    Ok(nupur::string::utf8_str_trusting(ruby, &bytes)?.as_value())
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

/// `s`'s bytes as they are, in a new ASCII-8BIT String: `read_bytes` into
/// `binary_str`, whatever `s`'s encoding.
fn binary_copy(ruby: &Ruby, s: Value) -> Result<Value, Error> {
    let bytes = nupur::string::read_bytes(string_arg(s)?)?;
    Ok(nupur::string::binary_str(ruby, &bytes)?.as_value())
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
        .only(ruby, &["a"])?
        .value(ruby, "a")?
        .unwrap_or_else(|| ruby.qnil()))
}

/// `kw_only(a:, b:)`: refuses any other keyword, then answers `a`.
fn kw_only(ruby: &Ruby, args: &[Value]) -> Result<Value, Error> {
    let kw = Kwargs::scan(ruby, args)?;
    let kw = kw.only(ruby, &["a", "b"])?;
    Ok(kw.value(ruby, "a")?.unwrap_or_else(|| ruby.qnil()))
}

/// `kw_undeclared(a:)`: declares `a`, then reads `b` - the method's own
/// mistake, which Nupur answers with an internal error.
fn kw_undeclared(ruby: &Ruby, args: &[Value]) -> Result<Value, Error> {
    let kw = Kwargs::scan(ruby, args)?.only(ruby, &["a"])?;
    Ok(kw.value(ruby, "b")?.unwrap_or_else(|| ruby.qnil()))
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
/// Spins with the GVL released for up to `ms` milliseconds, asking its
/// `Cancel` as it goes: `true` when it ran the whole time, `false` when it
/// was asked to stop and Ruby then raised nothing.
fn spin_cancellable(ruby: &Ruby, ms: Value) -> Result<bool, Error> {
    let ms = integer_arg(ms)?.to_u64()?;
    nupur::gvl::without_gvl_cancellable(ruby, move |cancel| {
        let until = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        while std::time::Instant::now() < until {
            if cancel.requested() {
                return false;
            }
            std::hint::spin_loop();
        }
        true
    })
}

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

/* ---- classes, Integers, Symbols ---- */

/// The name of the builtin class `v` is an instance of exactly - its header's
/// class, so a subclass's instance and one with a singleton class are
/// neither - or nil.
fn exact_class(ruby: &Ruby, v: Value) -> Result<Value, Error> {
    let classes = [
        ("Array", ruby.class_array()),
        ("Float", ruby.class_float()),
        ("Hash", ruby.class_hash()),
        ("Integer", ruby.class_integer()),
        ("String", ruby.class_string()),
        ("Symbol", ruby.class_symbol()),
    ];
    for (name, class) in classes {
        if v.has_class(class) {
            return Ok(nupur::build::symbol(ruby, name)?.as_value());
        }
    }
    Ok(ruby.qnil())
}

/// `n`'s decimal digits, through `Integer::to_decimal`.
fn decimal_of(ruby: &Ruby, n: Value) -> Result<Value, Error> {
    Ok(integer_arg(n)?.to_decimal(ruby)?.as_value())
}

/// The Integer the bytes of `s` spell, through `build::int_from_decimal`.
fn int_from_decimal(ruby: &Ruby, s: Value) -> Result<Value, Error> {
    let bytes = nupur::string::read_bytes(string_arg(s)?)?;
    Ok(nupur::build::int_from_decimal(ruby, &bytes)?.as_value())
}

/// A Symbol's name, through `Symbol::name`.
fn symbol_name(_: &Ruby, s: Value) -> Result<Value, Error> {
    let sym = Symbol::from_value(s).ok_or_else(|| nupur::rb::error::type_error("a Symbol"))?;
    Ok(sym.name()?.as_value())
}

/* ---- calling a method ---- */

static TO_S: nupur::rb::call::IdConst = nupur::rb::call::IdConst::new(c"to_s");

/// `recv.public_send(name, *rest)`, through `rb::call::public_method`.
fn send_public(ruby: &Ruby, args: &[Value]) -> Result<Value, Error> {
    let a = nupur::args::scan(
        ruby,
        args,
        Shape {
            rest: true,
            ..Shape::positional(2, 0)
        },
    )?;
    let name = nupur::string::read_utf8(ruby, string_arg(a.required(1))?, |_| None::<()>)
        .map_err(read_error)?;
    let cname = std::ffi::CString::new(name.as_str())
        .map_err(|_| nupur::rb::error::arg_error("a name with a NUL"))?;
    let id = nupur::rb::Id::intern(ruby, &cname)?;
    nupur::rb::call::public_method(a.required(0), id, a.rest())
}

/// Whether `recv` responds to `to_s` publicly, through `IdConst` and
/// `rb::call::responds_to`.
fn responds_to_s(ruby: &Ruby, recv: Value) -> Result<bool, Error> {
    let id = TO_S.get(ruby)?;
    nupur::rb::call::responds_to(recv, id)
}

/// Whether `Object` holds the constant `name`, through `Module::const_defined`.
fn object_const_defined(ruby: &Ruby, name: Value) -> Result<bool, Error> {
    let name =
        nupur::string::read_utf8(ruby, string_arg(name)?, |_| None::<()>).map_err(read_error)?;
    ruby.class_object().const_defined(ruby, name.as_str())
}

/// A wrapped objects' class `name` under `Object`, through
/// `Module::define_data_class`: refused for a name already defined.
fn object_data_class(ruby: &Ruby, name: Value) -> Result<Value, Error> {
    let name =
        nupur::string::read_utf8(ruby, string_arg(name)?, |_| None::<()>).map_err(read_error)?;
    let class = ruby
        .class_object()
        .define_data_class(ruby, name.as_str(), None)?;
    Ok(class.as_value())
}

/// `abi::check_undefined` over the path `outer::name`.
fn check_undefined(ruby: &Ruby, outer: Value, name: Value) -> Result<Value, Error> {
    let outer =
        nupur::string::read_utf8(ruby, string_arg(outer)?, |_| None::<()>).map_err(read_error)?;
    let name =
        nupur::string::read_utf8(ruby, string_arg(name)?, |_| None::<()>).map_err(read_error)?;
    nupur::abi::check_undefined(ruby, &[outer.as_str(), name.as_str()])?;
    Ok(ruby.qnil())
}

/* ---- numbers and collections built without a call each ---- */

/// The Float of the bits `bits`, through `build::float` - a flonum built
/// from its bits, a heap Float otherwise.
fn float_of_bits(ruby: &Ruby, bits: Value) -> Result<Value, Error> {
    let bits = integer_arg(bits)?.to_u64()?;
    Ok(nupur::build::float(ruby, f64::from_bits(bits))?.as_value())
}

/// `[a, b, c]` through `build::ary_from`, then `d, e` through `ary_cat`.
fn ary_bulk(ruby: &Ruby, args: &[Value]) -> Result<Value, Error> {
    let a = nupur::build::ary_from(ruby, args.get(..3).unwrap_or_default())?;
    nupur::build::ary_cat(a, args.get(3..).unwrap_or_default())?;
    Ok(a.as_value())
}

/// A Hash of `pairs` (key, value, ...) through `build::hash_insert_pairs`.
fn hash_bulk(ruby: &Ruby, args: &[Value]) -> Result<Value, Error> {
    let h = nupur::build::hash(ruby)?;
    nupur::build::hash_insert_pairs(h, args)?;
    Ok(h.as_value())
}

/* ---- reading a String in place, and a value's kind ---- */

/// The byte sum of `s`, read through `with_bytes`.
fn lent_sum(_: &Ruby, s: Value) -> Result<u64, Error> {
    let s = string_arg(s)?;
    Ok(s.with_bytes(|b| b.iter().map(|&x| u64::from(x)).sum()))
}

/// `[length, appended]`: the length read under `with_locked_bytes`, and
/// whether `block`, run with the lock held, could append to `s` - it must
/// not, so the spec's block raises and this answers the error.
fn locked_len(ruby: &Ruby, s: Value) -> Result<Value, Error> {
    let s = string_arg(s)?;
    let n = s.with_locked_bytes(|b| b.len())?;
    let inner = s.with_locked_bytes(|_| nupur::rb::method::yield_value(ruby, s.as_value()))?;
    let out = nupur::build::ary(ruby, 2)?;
    nupur::build::push(out, nupur::build::int(ruby, i64::try_from(n).unwrap_or(0))?)?;
    nupur::build::push(out, inner?)?;
    Ok(out.as_value())
}

/// Whether Ruby holds `s` to be UTF-8, through `scanned_utf8`.
fn knows_utf8(_: &Ruby, s: Value) -> Result<bool, Error> {
    string_arg(s)?.scanned_utf8()
}

/// `v`'s kind, through `Value::kind`, as a Symbol.
fn kind_of(ruby: &Ruby, v: Value) -> Result<Value, Error> {
    use nupur::rb::Kind;
    let k = match v.kind() {
        Kind::Nil => "nil",
        Kind::True => "true",
        Kind::False => "false",
        Kind::Fixnum(_) => "fixnum",
        Kind::Bignum(_) => "bignum",
        Kind::Float(_) => "float",
        Kind::String(_) => "string",
        Kind::Symbol(_) => "symbol",
        Kind::Array(_) => "array",
        Kind::Hash(_) => "hash",
        Kind::Other => "other",
    };
    Ok(nupur::build::symbol(ruby, k)?.as_value())
}

/* ---- a wrapped object shared between Ractors ---- */

/// A payload read from several Ractors at once: plain data, `Sync`.
struct Shared {
    n: i64,
}

impl Payload for Shared {}

static SHARED: TypedType<Shared, 1> = TypedType::shareable(c"NupurTest::Shared");
static SHARED_CLASS: RbConst<DataClass> = RbConst::new();

/// `Shared.new(n, v)`: an object holding `n`, keeping `v` in its slot, made
/// shareable as it is made.
fn shared_new(ruby: &Ruby, n: Value, v: Value) -> Result<Value, Error> {
    shared_made(ruby, &SHARED, &SHARED_CLASS, n, v)
}

/* A type deriving from a shareable one is shareable too. */
static SHARED_DERIVED: TypedType<Shared, 1> =
    TypedType::derived(c"NupurTest::SharedDerived", &SHARED);
static SHARED_DERIVED_CLASS: RbConst<DataClass> = RbConst::new();

fn shared_derived_new(ruby: &Ruby, n: Value, v: Value) -> Result<Value, Error> {
    shared_made(ruby, &SHARED_DERIVED, &SHARED_DERIVED_CLASS, n, v)
}

fn shared_made(
    ruby: &Ruby,
    ty: &'static TypedType<Shared, 1>,
    class: &'static RbConst<DataClass>,
    n: Value,
    v: Value,
) -> Result<Value, Error> {
    let made = ty.wrap(
        ruby,
        Birth {
            klass: class.defined(ruby)?,
            data: Shared {
                n: integer_arg(n)?.to_i64()?,
            },
            roots: [v],
        },
    )?;
    Ok(made.value())
}

fn shared_n(_: &Ruby, this: Value) -> Result<i64, Error> {
    Ok(SHARED.check(this)?.data().n)
}

fn shared_held(_: &Ruby, this: Value) -> Result<Value, Error> {
    Ok(SHARED.check(this)?.root::<0>())
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

    let shared = m.define_data_class(ruby, "Shared", None)?;
    SHARED_CLASS.set(shared)?;
    shared.define_singleton_method("new", function!(shared_new, 2))?;
    shared.define_method("n", method!(shared_n, 0))?;
    shared.define_method("held", method!(shared_held, 0))?;
    let shared_derived = m.define_data_class(ruby, "SharedDerived", Some(shared))?;
    SHARED_DERIVED_CLASS.set(shared_derived)?;
    shared_derived.define_singleton_method("new", function!(shared_derived_new, 2))?;

    let holder = m.define_data_class(ruby, "Holder", None)?;
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
    m.define_module_function("raise_builtin", function!(raise_builtin, 1))?;
    m.define_module_function("trusting_str", function!(trusting_str, 1))?;
    m.define_module_function("read_capped", function!(read_capped, 2))?;
    m.define_module_function("transcode_kind", function!(transcode_kind, 1))?;
    m.define_module_function("filled", function!(filled, 1))?;
    m.define_module_function("slices", function!(slices, 1))?;
    m.define_module_function("binary_copy", function!(binary_copy, 1))?;
    m.define_module_function("kw", function!(kw, -1))?;
    m.define_module_function("kw_only", function!(kw_only, -1))?;
    m.define_module_function("kw_undeclared", function!(kw_undeclared, -1))?;
    m.define_module_function("shape", function!(shape, -1))?;
    m.define_module_function("pairs", function!(pairs, 1))?;
    m.define_module_function("sum_without_gvl", function!(sum_without_gvl, 1))?;
    m.define_module_function("spin_cancellable", function!(spin_cancellable, 1))?;
    m.define_module_function("frozen_check", function!(frozen_check, 1))?;
    m.define_module_function("booleans", function!(booleans, 0))?;
    m.define_module_function("usize_of", function!(usize_of, 1))?;
    m.define_module_function("f64_bits_of", function!(f64_bits_of, 1))?;
    m.define_module_function("exact_class", function!(exact_class, 1))?;
    m.define_module_function("decimal_of", function!(decimal_of, 1))?;
    m.define_module_function("int_from_decimal", function!(int_from_decimal, 1))?;
    m.define_module_function("symbol_name", function!(symbol_name, 1))?;
    m.define_module_function("send_public", function!(send_public, -1))?;
    m.define_module_function("float_of_bits", function!(float_of_bits, 1))?;
    m.define_module_function("lent_sum", function!(lent_sum, 1))?;
    m.define_module_function("locked_len", function!(locked_len, 1))?;
    m.define_module_function("knows_utf8", function!(knows_utf8, 1))?;
    m.define_module_function("kind_of", function!(kind_of, 1))?;
    m.define_module_function("ary_bulk", function!(ary_bulk, -1))?;
    m.define_module_function("hash_bulk", function!(hash_bulk, -1))?;
    m.define_module_function("responds_to_s", function!(responds_to_s, 1))?;
    m.define_module_function("object_const_defined", function!(object_const_defined, 1))?;
    m.define_module_function("object_data_class", function!(object_data_class, 1))?;
    m.define_module_function("check_undefined", function!(check_undefined, 2))?;
    Ok(())
}

#[no_mangle]
#[allow(non_snake_case)]
pub extern "C" fn Init_nupur_test() {
    // SAFETY: Ruby calls `Init_nupur_test` on its thread, with the GVL held.
    unsafe { nupur::rb::method::run_init(check, init) };
}
