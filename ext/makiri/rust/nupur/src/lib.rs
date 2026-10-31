//! `nupur`: a Ruby binding layer for an extension - values, exceptions,
//! method trampolines, TypedData, the GVL, and every C call that can raise,
//! over rb-sys alone.
//!
//! It knows nothing of the extension it serves - a crate of its own, which
//! does not depend on the extension - and what it needs of the extension
//! (the exception a broken invariant raises, the OOM sweep's hooks) is
//! registered at `Init_` ([`rb::error::INTERNAL_ERROR`],
//! `inject::install`). Every `unsafe` the extension needs to talk to
//! Ruby is here, each with its contract stated. The crate denies
//! `unsafe_code`, and the files that need it carry an `allow` each.
//!
//! # What the layer guarantees
//!
//! - **No raise crosses Rust.** Every C call that can raise runs under
//!   `rb_protect` (`raw`) and comes back as `Err`; a method's `Err` is
//!   raised by its trampoline ([`rb::method::finish`]) once every Rust frame
//!   of the method has returned.
//! - **No panic reaches C.** A trampoline catches a panic and raises the
//!   internal error in its place; a callback C runs latches one
//!   ([`caught::PanicLatch`]) for its caller to resume. What must not unwind
//!   (an `extern "C"` function, a `Drop` the GC runs) is a [`no_unwind`]
//!   site, which a release build with `--cfg nupur_no_unwind` proves.
//! - **The GVL is a token.** A [`rb::Ruby`] is made only by a trampoline and
//!   by [`rb::method::run_init`], both `unsafe fn`s only the macros' expansion
//!   (and the extension's `Init_`) call; everything that calls Ruby takes one,
//!   or a Ruby value, which exists only where one did. Either is proof
//!   enough, so a function takes `&Ruby` only when it holds no Ruby value
//!   (or handle made from one, `self` included) to show. The exceptions say
//!   more than the GVL: the reads of the running method's own frame
//!   (`rb::method::block_given`, `keyword_given`, `yield_value`), whose
//!   token is the method's, and `Opaque::get`, whose value lives in a
//!   static.
//! - **A wrapped object's values are its slots** ([`typed`]), marked by the
//!   layer; a payload holds none. Memory reported to the GC is given back by
//!   the free callback. One is made only of a [`rb::DataClass`], a class
//!   defined without an allocator, so Ruby cannot make one with no payload.
//! - **A String is read by copying** ([`string`]): no slice of one is handed
//!   out. Read as text ([`string::read_utf8`]), it is a copy checked to be
//!   UTF-8 by std's validator over the copy itself ([`utf8_copy`]), never by
//!   the String's cached coderange, which another C extension could leave
//!   wrong.
//! - **A C integer is converted in one place** (`int`): a `long` is 32
//!   bits on Windows, and the conversions are written once over either
//!   width and tested at both; everything else converts with `From` /
//!   `TryFrom` and names no `long`.
//! - **A non-local exit is resumed as it was.** While a `break` or `throw`
//!   a protected call caught is on its way to the trampoline, no protected
//!   call runs C, so the thread's error info Ruby resumes it from is left
//!   alone.
//! - **An object is held while its data is borrowed through a handle.**
//!   A [`typed::Typed`] - from `TypedType::check` for a value Ruby gave,
//!   `TypedType::known` for one whose type is established, `TypedType::wrap`
//!   for an object just made; there is no other way to a payload - lends the data and
//!   keeps the object where the GC sees it until the last borrow is over,
//!   wherever the object came from; `wrap_hidden` lends it to its `fill` the
//!   same way, and hands the object out only once it is complete and shown.
//! - **Two Ractors may run the layer at once.** Each runs its methods under
//!   its own GVL, so nothing of the layer's is shared but what any Ractor
//!   may read: a static keeps only a [`rb::Shareable`] value (a class or a
//!   module), a pending exit is the native thread's own, and a wrapped
//!   object never leaves its Ractor - its type is never one Ruby may make
//!   shareable, and its class, having no allocator, cannot be copied or
//!   moved to another. An extension opts in at `Init_`
//!   ([`rb::method::declare_ractor_safe`]).
//!
//! Several of these rest on checks of this crate's own code, not on its
//! types alone. In Makiri they are `rake dylint` (`lints/makiri_lints`,
//! whose first group is the layer's and names only `nupur::` paths, so a
//! copy changes its `paths::EXTENSION` alone), `rake no_unwind` and
//! `rake unsafe:boundaries`; a copy of this layer elsewhere needs its own:
//!
//! - every C call that can raise is inside `raw::call` -
//!   `raising_call_outside_protect` and `unraising_call_in_raw`, over a
//!   table of the functions that cannot raise held to a survey of Ruby's C
//!   API (`script/check_non_raising.rb`);
//! - every `extern "C"` function and GC-run `Drop` is a [`no_unwind`] site -
//!   `unguarded_no_unwind_site`, and `rake no_unwind`'s proof of each;
//! - a safe function that calls Ruby's C holds a proof of the GVL -
//!   `ffi_without_gvl_proof`;
//! - no Ruby error is dropped - `discarded_ruby_error`;
//! - no C `long` is cast outside `int` - `c_integer_outside_int`;
//! - each file's count of `unsafe` is pinned - `rake unsafe:boundaries`.
//!
//! Its behaviour is checked without Makiri too: `test/ext` is an extension
//! of a few methods over the layer alone, and `test/spec` the specs that load
//! it (`rake nupur:spec`) - a panic in a method, below the GVL release and in
//! a Hash walk, a block's exits, a slot under collection and compaction, a
//! checked String read and its `$!`.
//!
//! # What the extension must uphold
//!
//! The types do not carry these; a crate using the layer keeps them, by
//! review or by lints. Each says what checks it in Makiri, which a crate
//! that copies this layer has to supply in its own way:
//!
//! - **A Ruby value stays where the GC's stack scan sees it.** A [`rb::Value`]
//!   (and every handle over one, [`typed::Typed`] included) is `!Send`, and
//!   a `Value` is `Copy` (a `Typed`, which holds its object, is not): on the
//!   stack it is kept alive and in place, but in a `Vec`, a `Box`, a
//!   `static` or a payload's field it is not, and is freed or moved while
//!   still held. Keep such values on the stack, in a Ruby Array, in a
//!   wrapped object's slots, or in an [`rb::Opaque`] / [`rb::RbConst`].
//!   Checked by `ruby_value_on_heap`, which reads the types of bindings,
//!   parameters, results, fields, statics and every expression - so a
//!   temporary handed straight to a call (`f(vec![v])`) too. Beyond it: a
//!   value behind `dyn Trait` once coerced, and a generic body for the
//!   type it is called with.
//! - **A payload's handle is not `mem::forget`ed.** A payload is borrowed
//!   only through a [`typed::Typed`] (`check`, `known`, `wrap`), whose drop
//!   is what holds the object past the last borrow; there is no way from a
//!   bare `Value` to a `&T`. Checked by `forgotten_handle`; a type check
//!   that borrows nothing is `TypedType::ensure`.
//! - **A GVL-released body runs no Ruby.** [`gvl::without_gvl`]'s `Send`
//!   bound keeps a token and a value out of it, but not a call of a layer
//!   function that needs neither (making an [`rb::Error`], say) - harmless
//!   today, yet not a guarantee. Checked by `ruby_in_gvl_released_body`.
//! - **Nothing runs the caller's Ruby unasked** - a `to_s`, a block, a
//!   method call. Checked by `runs_callers_ruby`, with each sanctioned call
//!   an `allow` stating why.
//! - **Declared Ractor-safe, the extension's own state is too.** What the
//!   layer keeps is safe across Ractors; what the extension keeps - its
//!   statics, whatever its methods take the GVL to serialise (now only one
//!   Ractor's threads) - is its own to show, and a class of its wrapped
//!   objects defines no Marshal hook, through which Ruby would copy one to
//!   another Ractor. Checked by review alone, and by specs that run its
//!   methods in several Ractors at once.
//! - **The GVL's token is made only where Ruby calls in.** [`rb::method::run_init`]
//!   is called from the extension's `extern "C" fn Init_*` alone, and
//!   [`rb::method::finish`] from the `method!` / `function!` expansions alone;
//!   the token's own maker is crate-private. Checked by
//!   `gvl_token_outside_entry`.

#![deny(unsafe_code)]
/* The casts that lose a value. Where the code is built a C `long` is `i64`
 * itself, so these see nothing there that `c_integer_outside_int` does not;
 * on Windows, whose `long` is 32 bits, clippy sees the real types - CI runs
 * it on that cell too. `int` is the one module that casts. */
#![warn(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]
/* `i64::from(a_c_long)` is a no-op here and a widening on Windows: the
 * conversions are written for every target, which this lint cannot see. */
#![allow(
    clippy::useless_conversion,
    reason = "C's integer types are aliases whose width is the target's"
)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
/* `rake dylint` sets `dylint_lib` for the whole build, the extension's run
 * compiling this crate as a plain dependency: there the extension's lints,
 * which the `allow`s here name, are unknown. */
#![cfg_attr(dylint_lib = "makiri_lints", allow(unknown_lints))]
/* An `Err` of a Ruby error is never dropped: `Error::rescue` handles the
 * classes it names and hands every exit back. `deny` here, not `forbid` as
 * in the extension: the layer's one drop is `describe::for_message`, a
 * message's lookup, allowed there with its reason. */
#![cfg_attr(dylint_lib = "makiri_lints", deny(discarded_ruby_error))]

// The lints' self-test for this crate (`rake dylint`).
#[cfg(nupur_lint_probe)]
mod lint_probe;

// The value and method layer: `Value` and the class-checked handles,
// `Error`, the `method!` / `function!` trampolines, class definitions.
pub mod rb;

// Every Ruby C call that can raise, each under `rb_protect`.
pub(crate) mod raw;

// A short name as a C string on the stack, as Ruby's C API takes one.
pub(crate) mod cname;

// Every conversion between Rust's integer widths and C's `long` - the
// layer's own: what it hands an extension is in Rust's widths.
pub(crate) mod int;

// TypedData: wrapping a Rust value as a Ruby object, and its GC hooks.
pub mod typed;

// Running a body with the GVL released.
pub mod gvl;

// Building Ruby values: Arrays, Hashes, numbers, Symbols, UTF-8 Strings.
pub mod build;

// A Ruby encoding, as Strings are read and converted by it.
pub mod encoding;

// A Ruby String's bytes, read and written by copying.
pub mod string;

// Owned bytes checked to be UTF-8 (`Utf8Copy`), std alone.
pub mod utf8_copy;

// The OOM sweep's hooks, installed together (`rake oom`).
#[cfg(feature = "alloc-inject")]
pub mod inject;

// A private table from small integers to values ([`table::IntTable`]).
pub mod table;

// Reading a method's arguments: a type check each, no conversion; the
// separation of positional arguments and keywords; keywords by name.
pub mod args;

// Walking a Hash's pairs.
pub mod hash;

// Ruby values written into a message without a call.
pub mod describe;

// What the binary assumes of the running Ruby, and the load checks of it.
pub mod abi;

// The native-stack check, for a token.
pub mod stack;

// `Msg`: an exception message written into a fixed buffer on the stack,
// so an error path allocates nothing.
pub mod msg;

// Catching a panic at a C boundary and re-raising it past one.
pub mod caught;

// The layer's bodies that must not unwind, and the build that checks they
// cannot (`rake no_unwind`).
pub mod no_unwind;
