//! `nupur_sys`: Ruby's C API as Nupur sees it - the one crate of the layer
//! that depends on rb-sys, so Nupur itself cannot name rb-sys.
//!
//! It holds the protection every C call that can raise runs under
//! ([`protect`]: `rb_protect`, `$!`, a pending non-local exit, and the
//! [`protect::Protected`] token the protected closure is lent), and the
//! mechanism that proves what must not unwind cannot ([`no_unwind`]), whose
//! guard the protect thunk runs under.
//!
//! Ruby's C is reached only through wrappers here, each taking its proof: a
//! raising call the protection's token ([`raising`], [`protect::Protected`]),
//! any other a value or the GVL's token ([`non_raising`], [`value::Value`],
//! [`value::Ruby`]). Of rb-sys itself only a few types and constants Nupur
//! names are re-exported, and no C function and no `VALUE`, so those rules
//! are types rather than lints.
//!
//! The crate denies `unsafe_code`, and the files that need it carry an
//! `allow` each, as Nupur does.
#![deny(unsafe_code)]
/* The casts that lose a value, as in Nupur: where the code is built a C
 * `long` is `i64` itself, so these see nothing there; on Windows, whose
 * `long` is 32 bits, clippy sees the real types - CI runs it on that cell
 * too. `int` is the one module that casts. */
#![warn(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
/* `i64::from(a_c_long)` is a no-op here and a widening on Windows: the
 * conversions are written for every target, which this lint cannot see. */
#![allow(
    clippy::useless_conversion,
    reason = "C's integer types are aliases whose width is the target's"
)]
/* Built as a dependency under `rake dylint`, the crate sees `dylint_lib` but
 * not the lints, so their `allow`s name lints it does not know. */
#![cfg_attr(dylint_lib = "makiri_lints", allow(unknown_lints))]

/* The types and constants Nupur names, and nothing else of rb-sys's: no C
 * function is re-exported, so Nupur calls Ruby's C only through the
 * wrappers here, each taking its proof (`raising`, `non_raising`, `protect`); and
 * no `VALUE`, so Nupur holds a Ruby value only as a `value::Value` (or a
 * slot's `value::Root`), never as an integer. */
pub use rb_sys::{
    rb_data_type_t, rb_encoding, rb_enumerator_size_func, ruby_coderange_type,
    ruby_econv_flag_type, ruby_encoding_consts, ruby_value_type, size_t, ssize_t, st_retval, ID,
    RUBY_API_VERSION_MAJOR, RUBY_API_VERSION_MINOR, RUBY_DATA_FUNC,
};

pub mod data_type;
pub mod int;
pub mod no_unwind;
pub mod non_raising;
pub mod protect;
pub mod raising;
pub mod value;

/* `rake dylint`'s self-test: what the lints must report here. */
#[cfg(nupur_sys_lint_probe)]
mod lint_probe;
