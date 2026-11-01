# Nupur

Nupur is the Ruby binding layer under Makiri's extension. It gives the
extension a safe API over Ruby's C: Ruby values and their class-checked
handles, exceptions, method trampolines, TypedData, the GVL, and a wrapper
for every C call that can raise. Every `unsafe` that talks to Ruby is in
Nupur or in the crate under it, but one: the extension's `Init_`, which hands
its definitions to `run_init`.

The layers, from the top:

```
makiri_rs   the extension: Makiri's Ruby methods
nupur       the safe API, built on nupur_sys
nupur_sys   (sys/) Ruby's C, each call taking a proof; the one crate on rb-sys
rb-sys      Ruby's C API, as generated bindings
```

Nupur does not depend on the extension and knows nothing of it. What it needs
from the extension (the exception class for a broken internal invariant, the
`rake oom` hooks) the extension registers at `Init_`. The extension reaches
Nupur as `crate::nupur`, and its `bridge` re-exports `rb`, `typed`, `gvl`,
`build`, `hash` and `args`.

## What it guarantees

Most of Nupur's rules are its types and its crates' dependencies, so code
that breaks one does not compile. `src/lib.rs` states each in full.

- **No raise crosses Rust.** A raise is a `longjmp` over Rust frames, which
  must not happen. Every C call that can raise is a wrapper in
  `nupur_sys::raising` that takes a `&Protected`. Only `rb_protect`'s thunk
  makes one, and lends it to the closure that `raw::call` runs, so the raise
  comes back as an `Err`; a method's `Err` is raised by its trampoline once
  every Rust frame of the method has returned. Nupur has no other way to
  call a C function: it does not depend on rb-sys, and `nupur_sys`
  re-exports no C function.
- **No panic reaches C.** A trampoline turns a panic into the internal
  error; a callback C calls catches one (`caught::PanicLatch`) for its
  caller to resume. Code that must not unwind is a `no_unwind` site, and a
  release build with `--cfg nupur_no_unwind` proves it cannot
  (`rake no_unwind`).
- **The GVL is a type.** A call of Ruby's C that needs the GVL takes a proof
  that it is held: the Ruby value it is about (`nupur_sys::value::Value`,
  which Nupur's `Value` holds), or, where there is none, the token `Ruby`.
  Neither is `Send`, so neither can enter a body run with the GVL released.
  A value is made only inside `nupur_sys`; a token only by `rb::method`'s
  private `entry_token`, which only the entries Ruby calls use.
- **No Ruby value is held as an integer.** `nupur_sys` re-exports no
  `VALUE`, and keeps the conversions between a value and its word to itself.
- **A wrapped object's Ruby values are its slots.** A payload holds no Ruby
  value. The slots (`nupur_sys::value::Root`) are written once, when the
  object is made, and Nupur's mark callback marks them. A payload is reached
  only through a `typed::Typed` handle, which keeps the object alive while
  its data is borrowed.
- **A String is read by copying.** No slice of one is handed out. Text is a
  copy checked to be UTF-8 by std's validator, never trusted from the
  String's cached coderange.
- **A C integer is converted in one place** (`nupur_sys::int`). A C `long`
  is 32 bits on Windows, so the conversions are written once for either
  width, tested at both and proved at both (`verus/int.rs`).
- **Two Ractors may run the layer at once**, once the extension declares
  itself Ractor-safe (`rb::method::declare_ractor_safe`). Nothing of
  Nupur's is shared between them but what any Ractor may read.

### What the types do not hold

Some rules cannot be types. Makiri holds these with checks of the code
(`rake dylint`'s lints in `lints/makiri_lints`, `rake no_unwind`,
`rake unsafe:boundaries`), so a copy of Nupur used elsewhere needs checks of
its own:

- **That a token is made only where Ruby calls in.** That Ruby called the
  code, holding the GVL, is a fact about the caller, which no type states.
  `gvl_token_outside_entry` keeps the trampolines and `run_init` to where
  Ruby calls them, and `nupur_sys`'s maker (`Ruby::assume_gvl`) to
  `entry_token`.
- **That a Ruby value stays where the GC's stack scan sees it.** A value is
  `Copy`, and no type can refuse one in a `Vec` or a `Box`, whose contents
  the scan does not see. `ruby_value_on_heap` reads the types of bindings,
  fields, statics and expressions to find one.
- **That `nupur_sys`'s own code keeps the layer's rules.** It is the one
  crate that can name rb-sys, so inside it the raise, GVL and integer rules
  are lints (`raising_call_outside_protect`, `ffi_without_gvl_proof`,
  `c_integer_outside_int`). Which C functions can raise is a table held to
  a survey of Ruby's C API.

An extension must keep a few rules of its own: a body run with the GVL
released runs no Ruby, nothing runs the caller's Ruby unasked, and a
Ractor-safe extension's own state is safe across Ractors. `src/lib.rs` lists
them and what checks each. Read it before using the layer.

## Writing an extension

A method is a Rust function that takes the GVL's token (`&Ruby`) and returns
its value, or a `Result` of it; the `method!` / `function!` macros wrap it in
a trampoline. `Init_` hands two functions to `run_init`: a check of the
running Ruby, whose refusal is a `LoadError`, and the definitions. This is a
trimmed copy of the test extension in `test/ext/src/lib.rs`:

```rust
use nupur::rb::{prelude::*, Error, ExceptionClass, RbConst, Ruby, Value};
use nupur::function;

static INTERNAL: RbConst<ExceptionClass> = RbConst::new();

fn echo(_: &Ruby, v: Value) -> Value {
    v
}

fn check(ruby: &Ruby) -> Result<(), Error> {
    nupur::abi::check_running_api("nupur_test", "rake nupur:spec")?;
    nupur::abi::check_raise_state(ruby)
}

fn init(ruby: &Ruby) -> Result<(), Error> {
    let m = ruby.define_module("NupurTest")?;
    let internal = m.define_error("InternalError", ruby.exception_exception())?;
    INTERNAL.set(internal)?;
    nupur::rb::error::INTERNAL_ERROR.set(internal)?;
    m.define_module_function("echo", function!(echo, 1))?;
    Ok(())
}

#[no_mangle]
#[allow(non_snake_case)]
pub extern "C" fn Init_nupur_test() {
    // SAFETY: Ruby calls `Init_nupur_test` on its thread, with the GVL held.
    unsafe { nupur::rb::method::run_init(check, init) };
}
```

The test extension also wraps an object with a slot (`TypedType`, `Birth`),
reads arguments and keywords (`args`), walks a Hash, reads Strings, and runs
a body with the GVL released.

## Compared with Magnus

Nupur replaced Magnus in Makiri, and keeps many of its names (`Value`,
`RString`, `Error`, `method!`). The comparison is with Magnus 0.8.2, the
version Makiri used until the replacement in October 2026.

Magnus is a general binding that covers most of Ruby's C API; Nupur covers
only the C functions and types Makiri uses. Using Magnus meant trusting all
of its internals and their `unsafe` for the thirty or so things Makiri
needed from it. With Nupur, what is trusted is the C functions Makiri chose
and wrote a contract for.

Where the two differ on safety:

| | Magnus | Nupur |
| --- | --- | --- |
| C calls that can raise | Many run under `protect`; which ones can raise is the implementers' judgement, which a user cannot check | Each is a wrapper that takes the protection's token, so it cannot be called outside `rb_protect`; which functions can raise is a table held to a survey of Ruby's C API |
| A panic in a method | Raised as `fatal` (Magnus #118), which cannot be rescued where it happens | Raised as the extension's internal error, an `Exception` that a host can rescue by name |
| Arguments | Converted with `to_str` / `to_int`, Ruby code the caller can redefine | Not converted: a method takes `Value` and checks the type; a method taking `RString` or `i64` does not compile |
| Ruby values in a wrapped object | The user writes the mark function; a value can sit in a `Vec` or a field (`BoxValue` exists for the heap) | Fixed slots, written once and marked by Nupur; a payload holds no Ruby value, and a lint refuses one on the heap |
| Borrowing a wrapped object's data | A borrowed `&T` could outlive its `Value`, which the optimizer had dropped, and point at freed memory (Magnus #131, #101) | Borrowed through a `Typed` handle that keeps the object visible to the GC until the last borrow ends |
| A String's bytes | `RString::as_slice` lends them | Only copied out, never lent |
| The GVL | `Ruby::get()`, checked at run time | A Ruby value proves it, or a token where there is none; the token is made only where Ruby calls in, and neither is `Send`, so neither enters a GVL-released body |
| Raw `VALUE`s | Available with its `rb-sys` feature (`AsRawValue`, `FromRawValue`) | Not reachable from Nupur: `nupur_sys` re-exports no `VALUE` |

Magnus is easier to write against: it converts arguments and return values
for you, its handles are `Copy`, it has far more of Ruby's API, and it is a
published crate. Nupur gives these up on purpose: an error-prone way of
writing something is either impossible or the longest way to write it.
Nupur is not published, and is used only inside Makiri's workspace.

## Layout

| Nupur | What it holds |
| --- | --- |
| `rb` | `Value` and the class-checked handles, `Error`, the `method!` / `function!` trampolines, class and module definitions |
| `typed` | TypedData: wrapping a Rust payload as a Ruby object, its slots and GC callbacks, memory reported to the GC |
| `gvl` | Running a body with the GVL released |
| `build` | Building Arrays, Hashes, numbers, Symbols and UTF-8 Strings |
| `string`, `encoding`, `utf8_copy` | Reading and writing Strings by copying, and their encodings |
| `args`, `hash` | Reading a method's arguments and keywords; walking a Hash |
| `describe`, `msg` | Writing Ruby values into an exception message without a call or an allocation |
| `abi` | What the binary assumes of the running Ruby, and the load-time checks of it |
| `caught`, `no_unwind` | Catching a panic at a C boundary; the bodies that must not unwind |
| `stack`, `table`, `inject` | The native-stack check, a private integer-keyed table, the `rake oom` hooks |

`raw` (the protected calls), `int` (`nupur_sys::int`, re-exported) and
`cname` are private to the crate.

| `nupur_sys` | What it holds |
| --- | --- |
| `protect` | `rb_protect`'s thunk, the `Protected` token, `$!`, a pending non-local exit |
| `raising` | The C calls that can raise, each taking `&Protected` |
| `non_raising` | The C calls that raise nothing, each taking a value or the token (some read, some write or release the GVL) |
| `value` | `Value`, the token `Ruby`, a slot's `Root` |
| `data_type` | A TypedData type's `rb_data_type_t` and its flags (`DataFlags`) |
| `int` | The conversions between C's integers and Rust's widths |
| `no_unwind` | The guard of each body that must not unwind (Nupur re-exports it) |

## Features and dependencies

Nupur depends on `nupur_sys` alone, and `nupur_sys` on rb-sys alone. The one
feature, `alloc-inject`, compiles in the `rake oom` hooks; the extension's
own `alloc-inject` turns it on, and it turns on `nupur_sys`'s. rb-sys is used
without `link-ruby`, since the extension is loaded into a Ruby that already
provides the `rb_*` symbols.

## Tests

```bash
cargo test -p nupur -p nupur_sys
bundle exec rake nupur:spec
```

`rake nupur:spec` builds the test extension (`test/ext`, a few methods over
the layer and nothing of Makiri's) and runs the specs beside it
(`test/spec`): a panic in a method, below the GVL release and in a Hash
walk; a block's exits; a slot under GC and compaction; checked String reads.
CI runs it on Linux and macOS.

`nupur_sys`'s compile-fail doctests show that its tokens and values cannot
be made outside it. Only nightly's rustdoc checks the error each names, so
`rake rust:compile_fail` runs them there.
