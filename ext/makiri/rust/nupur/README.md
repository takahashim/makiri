# Nupur

Nupur is the Ruby binding layer under Makiri's extension, built on rb-sys
alone. It holds every `unsafe` the extension needs to talk to Ruby, and gives
the extension a safe API over it: Ruby values and their class-checked
handles, exceptions, method trampolines, TypedData, the GVL, and a wrapper
for every C call that can raise.

Nupur does not depend on the extension and knows nothing of it. What it needs
from the extension (the exception class for a broken internal invariant, the
`rake oom` hooks) the extension registers at `Init_`. The extension reaches
Nupur as `crate::nupur`, and its `bridge` re-exports `rb`, `typed`, `gvl`,
`build`, `hash` and `args`.

## What it guarantees

`src/lib.rs` states each guarantee in full, with what checks it. In short:

- **No raise crosses Rust.** Every C call that can raise runs under
  `rb_protect` (`raw`) and comes back as an `Err`. A method's `Err` is raised
  by its trampoline once every Rust frame of the method has returned.
- **No panic reaches C.** A trampoline turns a panic into the internal error;
  a callback C calls catches one (`caught::PanicLatch`) for its caller to
  resume. Code that must not unwind is a `no_unwind` site, and a release
  build with `--cfg nupur_no_unwind` proves it cannot (`rake no_unwind`).
- **The GVL is a token.** A `Ruby` is made only where Ruby calls in (a
  method trampoline and `rb::method::run_init`), and everything that calls
  Ruby takes one, or a Ruby value, which can exist only where a token did.
- **A wrapped object's Ruby values are its slots.** A payload holds no Ruby
  value; the slots are written once, when the object is made, and Nupur's
  mark callback marks them. A payload is reached only through a
  `typed::Typed` handle, which keeps the object alive while its data is
  borrowed.
- **A String is read by copying.** No slice of one is handed out. Text is a
  copy checked to be UTF-8 by std's validator, never trusted from the
  String's cached coderange.
- **A C integer is converted in one place** (`int`): a C `long` is 32 bits on
  Windows, so the conversions are written once for either width.
- **Two Ractors may run the layer at once**, once the extension declares
  itself Ractor-safe (`rb::method::declare_ractor_safe`). Nothing of Nupur's
  is shared between them but what any Ractor may read.

The same file also lists the rules the types cannot enforce, which an
extension must keep: Ruby values stay where the GC's stack scan sees them, a
GVL-released body runs no Ruby, nothing runs the caller's Ruby unasked, and
the rest. Read it before using the layer.

## An extension on Nupur

A method is a Rust function that takes the GVL's token (`&Ruby`) and returns
its value, or a `Result` of it; the `method!` / `function!` macros wrap it in
a trampoline.
`Init_` hands two functions to `run_init`: a check of the running Ruby, whose
refusal is a `LoadError`, and the definitions. This is a trimmed copy of the
test extension in `test/ext/src/lib.rs`:

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

## Modules

| Module | What it holds |
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

`raw` (the C calls that can raise), `int` and `cname` are private to the
crate.

## Features and dependencies

The one feature, `alloc-inject`, compiles in the `rake oom` hooks; the
extension's own `alloc-inject` turns it on. rb-sys is used without
`link-ruby`, since the extension is loaded into a Ruby that already provides
the `rb_*` symbols.

## Tests

```bash
cargo test -p nupur
bundle exec rake nupur:spec
```

`rake nupur:spec` builds the test extension (`test/ext`, a few methods over
the layer and nothing of Makiri's) and runs the specs beside it
(`test/spec`): a panic in a method, below the GVL release and in a Hash walk;
a block's exits; a slot under GC and compaction; checked String reads.
CI runs it on Linux and macOS.

Makiri's `rake dylint` (`lints/makiri_lints`), `rake no_unwind` and
`rake unsafe:boundaries` hold the layer's own code to its rules; a copy of
the layer elsewhere needs checks of its own (`src/lib.rs` lists them).
