# The Ruby boundary: panics, slots, raises, guards

This file explains why the extension is shaped as it is where Rust meets
Ruby: `CLAUDE.md` keeps the rules, and this is their reasoning and history.
`ext/makiri/rust/nupur/src/lib.rs` states what the binding layer guarantees.

## Fail-closed, and the export gate

**Security first, fail closed.** Enforce per-evaluate XPath budgets and
node-set caps, validate inputs, and never return a truncated or wrong result:
raise instead.

**Export only `Init_makiri`**, plus `ruby_abi_version`, which Ruby reads at
require time. The extension links no Lexbor any more, but it did, and a
bundle that re-exported Lexbor's ~1700 `lxb_*`/`lexbor_*` symbols let
another Lexbor-based gem in the same process (e.g. `nokolexbor`) bind its
`lxb_*` calls to our different Lexbor version, which segfaulted. Any name of
ours in the dynamic table is the same hazard.

**Verify with `bundle exec rake symbols`**, which asserts the whole claim:
nothing but those two names is exported, function or data. Do not weaken that gate to "no
`lxb_` exported". That check passed while ~220 `mkr_*` names leaked into the
dynamic table.

## Panics and unwinding

**A panic must not kill the host.** The crate builds with
`panic = "unwind"`, so every method's trampoline
(`bridge::rb::method::finish`) catches a panic and raises
`Makiri::InternalError` in its place, running every `Drop` of the method's
frames on the way out. `abort` did none of that: no destructor, no cleanup,
and SIGABRT for the whole process. So **do not set it back**.
`spec/panic_spec.rb` is what would catch that, through the
`Makiri.__panic(kind)` hook, which exists for no other purpose.

Two rules follow, and anything new has to keep both.

### Release in a `Drop`

**Whatever must be released across a panic is a `Drop`**, not a statement
after the work: a cleanup call that follows the work is exactly what
unwinding skips. The two Lexbor-era cases, `lexbor::selectors::Session` and
`post_parse::DocOwner`, are reference-only now.

### Latch a panic in a callback

**A callback must not panic into C.** Rust turns an unwind at an
`extern "C"` boundary into an abort, because unwinding through frames built
without unwind tables is undefined behaviour. So the callback catches the
panic, latches it and returns the stop status, and the caller re-raises once
C has unwound. `caught::PanicLatch` does this, and it deliberately has the
same shape the callbacks already used for the node cap and OOM.

The build that ships installs the latch in two callbacks:

- `bridge::gvl`'s trampoline, which carries the whole parser;
- `nupur::raw::hash_foreach`'s `rb_hash_foreach` callback, which answers
  "stop" for a latched panic or an error and never raises from inside the
  walk.

For the same reason, a Hash is walked only through
`nupur::hash::hash_foreach`, which is built over that callback. `RHash` has
no walk of its own, so there is no latch-less one to call, as magnus's
`foreach` was. The reference-only Lexbor callbacks carry the latch too
(`lexbor::selectors`' traversal, `lexbor::chunks::chunk_cb`,
`tree_guard::hook_token_cb`).

Two kinds of `extern "C"` function have no latch, on purpose:

- The GC callbacks in `nupur/src/typed/callbacks.rs` run where nothing can be
  raised, and a half-freed object is worse than a stop, so they keep
  aborting. Keep their bodies trivial.
- The `rb_protect` thunk of `nupur::raw::call` holds one C call and the
  `Copy` values it reads (`F: Copy`, `T: Copy`), so there is no Rust there to
  panic and no `catch_unwind` for a raise to cross. Keep it that way.

The one C call that calls back into Rust is `rb_hash_foreach`, and its
callback is latched (above) and raises nothing.

### Proving unwind-freedom

**Nothing that must not unwind can, and the compiler proves it**
(`engine/src/no_unwind.rs`, `rake no_unwind`). An unwind out of an
`extern "C"` function aborts, and so does a panic in a `Drop` that the GC
runs or that runs while another panic unwinds. Each such body is a `Site`
run through `must_not_unwind`.

The proof rests on a symbol nobody defines. A release build with
`--cfg makiri_no_unwind` (and `--cfg nupur_no_unwind` for nupur's own sites,
`nupur_unwinds_<site>`) gives each site's guard a `Drop` that calls
`makiri_unwinds_<site>`. Only an unwind reaches that call, so the optimizer
removes it exactly where it proves the body unwind-free, and `nm -u` lists
the sites that can unwind. The `self_test` control site can unwind by design
and must be the one left, or the check is not live. CI runs the proof in one
cell.

`rake dylint`'s `unguarded_no_unwind_site` fails on an `extern "C"` fn or a
`Drop` in the compiled crate, macro-generated ones included, whose body does
not run under `must_not_unwind`.

What the proof needed:

- Post-catch code free of `fmt`: `Msg::copy`, and `&'static str` errors in
  `raw::typed`/`c_len`. A `format_args!` there brought the unwind back.
- No abort call left at all. An `abort(3)` declared `extern "C"` stood in
  for `std::process::abort` until nothing needed one.
- `caught::quietly` (catch, leak the payload) around what the compiler cannot
  see through: a payload's drop and `downcast`, a type's drop glue in the
  free callback (the XPath AST's recursive glue), and the memsize callback,
  whose answer is advisory (0 on a panic).

mark and compact are not caught, because a value left unmarked is freed
while referenced; they are proven unwind-free as they are.

## Slots, hidden ivars and handles

### Slots

**A VALUE a wrapped object keeps alive is one of its slots, which `nupur`
owns** (`bridge::typed`, i.e. `nupur::typed`). An object's data is an
envelope: the payload `T` (a `Payload`, which holds no Ruby value at all)
and `N` slots (`TypedType<T, N>`, `EmbeddedType<T, N>`).
`wrap(ruby, Birth { klass, data, roots })` writes both in the same step,
right after the object's allocation with nothing run between, and the slots
are never written again. Through that step the roots were on the caller's
stack, where the stack scan pins them.

The mark callback marks every slot pinned in a fixed loop, so there is no
per-type trace to forget. The ordering rule behind the `DocData.errors`
crash, "store a VALUE into a struct only after its wrapper exists", has no
step left to happen in.

A slot is read back by `root_known::<I>()`, or with an embedded payload by
`read`. `root_known` checks `I < N` when it compiles and asserts the
object's type at run time with one `rb_typeddata_is_kind_of`; a mismatch is
a panic. Nothing re-points a slot and none is movable, so there is no
compaction callback.

**Three objects have a slot, one each**, for the reads too hot for an ivar:

- a node wrapper: its Document (`NodeWrapperType`);
- a Document: its node table (`DocType`);
- a NodeSet: its Document (`NodeSetType`).

### Hidden ivars

Every other value a wrapper keeps alive is a hidden ivar of its object: an
XPathContext's Document and node (`#node=` is an ivar write), and a
Document's errors. They are written and read with `rb::object::ivar_set` /
`rb::object::ivar_get` under names without `@`, so Ruby code cannot see
them.

### The node table

The node table keeps one wrapper per node, so a node navigated to twice is
the same object. It is a Ruby Hash from the node's index in its document to
its wrapper. The index is a Fixnum on every target, Windows' 31-bit ones
included; the document's stamp is checked against the node's instead of
riding in the key. The GC marks and moves the Hash itself, and the Hash
replaced a Rust map of movable values. Its entries are written only once the
wrapper exists, and a wrapper is handed out only once it is in the table.

Measured against the Rust map (`notes/magnus_replacement_plan.ja.md`): the
light readers unchanged, a cached traverse +4-8%, wrapping fresh nodes about
+12%. Moving the node's and the NodeSet's Document to ivars too cost
+15-65% and was not taken.

A `DocData`'s changing fields are cells, so the GC callbacks and every
method read a wrapper's data through `&T`: no `&mut` of it is made at all.

### Memory reported to the GC

**Memory reported to the GC** (an arena, a NodeSet's heap, an XPathContext's
AST cache) is reported through a `typed::External` the payload keeps. That
is the only way a report is made, and each report gives the GC the change
since the last one. `nupur`'s free callback gives the memory back: it reads
`Payload::reported_external` (the `External`'s `reported()`) before the
drop, so no `Drop` of Makiri's calls Ruby.

### Objects completed after they are made

An object whose data is not complete when it is made is made by
`wrap_hidden`: a Document before its content (`new_html_document` /
`new_xml_document`), or an XPathContext before its ivars. `wrap_hidden`:

1. writes the object's hidden ivars while it still has its class (generic
   ivar storage may read it);
2. hides it from `ObjectSpace`;
3. runs the caller's `fill` over its data;
4. only then shows it and returns it.

A hidden object has no class, and Ruby code given one would crash on it, so
none is handed out before step 4. A Document's node table is a
`table::IntTable`, made hidden and never shown. There is no bare `hide` /
`reveal`.

### Handles

**A payload is borrowed through a `typed::Typed` handle, which holds its
object.** These make one:

- `TypedType::check`, for a value Ruby gave;
- `known`, for one whose type is established (asserted; a mismatch is a
  panic);
- `wrap`, which returns one for an object just made;
- `wrap_hidden`, which lends one's data to its `fill`.

The handle is the only way to a payload: there is no `&Value -> &T`
(`get` / `get_known` are gone). A type check that borrows nothing is
`ensure`, and a forgotten handle is `rake dylint`'s `forgotten_handle`.

The handle holds the object because a `&T` keeps its object alive to the
type checker only: a non-embedded payload is its own malloc'd block, which
the stack scan does not take for the object. So the handle holds the VALUE
in an `rb::kept::Kept`, whose `Drop`, after the last borrow, hands it to
`keep_alive`. That is an empty `asm!` with the VALUE as an input operand, or,
on the targets without a stable `asm!`, a `read_volatile` of it, a weaker
contract and unchecked there. The premises that make either enough are
stated there. `black_box` (the old `gc_guard`) is best-effort and is not
used.

The Document's guards take a handle too. `html_read` / `xml_read` and the
writes borrow a `DocHandle`: `doc_known(v)` for a node's slot or a receiver,
`check` for a value to be checked, or what `new_html_document` /
`new_xml_document` return. `HtmlSelf` / `XmlSelf` own their Document's
handle, so a receiver view borrows it from the receiver.

### No Ruby value off the stack

The other half of the slot rule is that no Ruby value is kept where the
GC's stack scan cannot see it: a `Vec`, a `Box`, a static or
`thread_local!`, or a `Payload` type's own field. `rake dylint`'s
`ruby_value_on_heap` checks this by reading the types. A lint sees a `VALUE`
only as an integer, which is why raw `VALUE`s stay in `nupur/src/raw/` and
`nupur/src/typed/`.

The lint reads every expression's type too, so it reports a temporary
handed straight to a call (`f(vec![v])`) or a closure boxed with a value it
captured. What it cannot see is a value behind `dyn Trait` once coerced, and
a generic body instantiated with a Ruby value.

### The data type table

`rb_data_type_t` is built from a zeroed value with only `dmark`, `dfree`,
`dsize` and the flags set (`DataType::new`). A size assertion stops the
build on a callback table nobody has read: it is 5 words up to Ruby 4.0 and
12 from 4.1 (`handle_weak_references`, `reserved[7]`), a change that broke
every extension that named the fields (magnus #163, #189). CI's Ruby head
cell is where the next change shows.

### Embedded node data

A node wrapper's `NodeData` is embedded in its object's slot
(`bridge::typed::EmbeddedType`, `RUBY_TYPED_EMBEDDABLE`, Ruby 3.3+): one
allocation per wrapper, and the free callback drops the data without
freeing memory. Compaction moves that data with the object, so a
`&NodeData` held across anything that can allocate would point at where the
data was, and `EmbeddedType` has no `get` to hand one out. A reader takes
`read`'s copy instead (the payload is `Copy`; `read_node` makes it a
`NodeRef`, the handle and the Document), and only the GC callbacks see the
data in place. A new embedded type goes through `EmbeddedType` too.

The NodeSet was not embedded for this reason: its fill holds `&NodeSet`
across its own growth. That reference is borrowed from the `FreshSet` that
owns the new set, never with a lifetime of its own.

## C calls that can raise

### Every raising call is protected

**Every C call that can raise is a wrapper in `nupur/src/raw/`, and every
wrapper runs it under `rb_protect` and returns `Result`.** `nupur::raw` is
private to nupur; an extension reaches each wrapper through a value-level
API.

A raise is a `longjmp` over every Rust frame up to the nearest
`rb_protect`, and crossing a frame with a `Drop` or a `catch_unwind` is
undefined behaviour. So each wrapper hands its one C call to `raw::call`,
whose `extern "C"` thunk is the only frame a raise crosses, and the raise
comes back as `Err`. A caller may then own copies, hold document guards and
build Ruby objects in any order.

### Classifying which functions raise

Which C functions can raise is decided in `lints/makiri_lints`, in two lists
in `raising.rs`:

- `NON_RAISING`: the functions the C API raise survey finds no path to a
  longjmp for;
- `REVIEWED`: those it does find a path for and a person has read, each with
  the reason.

Every other rb-sys function is taken to raise, so an unclassified one is
refused until someone classifies it. The list of raising functions this
replaced let `rb_undef_alloc_func` and `rb_obj_freeze` through.

The survey (<https://takahashim.github.io/ruby-c-apis/>,
github.com/takahashim/ruby-c-apis) builds every public C function to LLVM IR
and asks which reach a longjmp. It covers the first and the latest patch
release of each Ruby series since 3.3 (3.3.0, 3.3.13, 3.4.0, 3.4.12, 4.0.0,
4.0.9) and HEAD, on Linux and macOS. Its `any` reading takes a call through
a function pointer to reach any address-taken function. That is its most
conservative reading, so a path may be a false positive and a missing one
cannot be. Its `typed` reading, a callee whose type matches the call's,
drops 60 of those paths and leaves direct ones, which is what `REVIEWED`
reasons cite.

The survey's result is vendored in `lints/makiri_lints/raise_survey.json`
(`script/update_raise_survey.rb`), and `rake dylint` runs
`script/check_non_raising.rb`. It fails on:

- a `NON_RAISING` entry the survey finds a path for;
- a `REVIEWED` entry it no longer finds one for, or one without a reason;
- an entry that is not in the survey.

It also holds both lists to Ruby's headers, as rb-sys's bindings carry their
doc comments: an entry documented to raise (`@exception`) or that never
returns fails.

The survey took `rb_obj_reveal` out: its write barrier can allocate, so it is
a protected `raw::obj_reveal`, and `wrap_hidden` / `new_*_document` return
`Result`. It also took out `rb_enc_get` / `rb_enc_get_index`: a File or
Symbol raises, so each is an `allow` at the String-only call sites. The
headers say nothing of any of them.

`REVIEWED` holds what the survey keeps a path for and the source shows is
never taken, whoever calls: `rb_thread_call_without_gvl2` (the path needs a
flag's value, which the `2` variant fixes).

### Raises ruled out at one call site

A function that can raise, but not where the extension calls it, is an
`allow` at that call with its reason, never a crate-wide entry:

- `rb_gc_mark` in the mark callback: it raises only when memory runs out,
  and that longjmp loses nothing there.
- `rb_attr_get` in `ivar_get`: it raises only for an `@` name of a class, a
  module or a shareable object read from another Ractor, and `ivar_get`
  takes a `HiddenId`, which is never an `@` name.

### The lints that hold `raw` to the table

`raising_call_outside_protect` fails on such a call outside the closure
handed to `raw::call`. A function that raises only for what its caller rules
out, such as `rb_obj_is_kind_of` with a non-class, is an `allow` at its site
saying so.

`unraising_call_in_raw` holds `raw` to what it is. A call of Ruby's C in
`raw` outside `call`, or a `call` that protects nothing, means the table and
the code disagree. A read that raises nothing belongs to `rb::value`, to
`rb::object` for an object's ivars and class, or to `rb::method` for the
current frame.

`call` is `raw::protect`'s, together with the rest of the protection (`$!`
taken and kept, a `break` / `throw` pending until the trampoline resumes it)
and the calls that raise on purpose; this is the one part of `raw` outside
the rule. A new raising call is a new wrapper with its raise stated.

### Building values, and the cost

The glue builds every Ruby value over those wrappers: Arrays, Hashes,
numbers and Symbols through `bridge::build`, Strings through
`bridge::string::utf8_str`, and every wrapped object, a NodeSet included,
through `typed::TypedType::wrap`.

Each protected call costs a `setjmp`, ~8 ns. A light reader that allocates
one String (`#name`, `#[]`, `#text`) measured ~15-20 ns slower than the
unprotected allocation it replaced; dropping the lifting (see "Document
guards") took back about half of that
(`notes/ruby_boundary_simplification_plan.ja.md`).

### The value-level wrappers

The value-level wrappers are safe and typed. They take and return
`bridge::rb`'s values (`RString`, `RArray`, `RHash`, `Value`), slices and
`bridge::string::Encoding`, and check what the C function would not: a Hash
key's kind, a substring's bounds, `to_encoding`'s argument. One with no
Ruby value takes `&Ruby` as the GVL's proof.

Only those over raw memory stay `unsafe`, and they are `nupur`'s alone:
`typed_object_zalloc` and `check_typeddata`, both `pub(crate)`. So are the
three that longjmp - `exc_raise`, `jump_tag`, `exc_fatal` - which only a
trampoline's raise may call. A String made in place is `utf8_str_filled`,
which is safe; its `fill` writes front to back through an `Unwritten` and
must reach the end.

### Where a raw `VALUE` may appear

A raw `VALUE` is an integer to Rust, which no lint can tell from a `u64` in
a `Vec`. It is spelled, or read from a C global of Ruby's (`rb_cObject`),
only in `raw/`, `int.rs`, `typed/` and the `bridge::rb` files that are the
typed values. `rake dylint`'s `raw_ruby_outside_bridge` enforces this, and
also fails on any `rb_sys::` outside `bridge/`.

Everything else therefore holds typed values, and `TypedType::wrap` is safe
for that reason: it takes `&Ruby`, and a payload keeps no Ruby value, since
its roots are slots. `bridge/` itself is `#![forbid(unsafe_code)]` from its
root down: every `unsafe` that reads Ruby's memory or calls its C is
`nupur`'s.

## Document guards

**A Document's content is reached only through a guard** (`bridge::dom`,
over `crate::doc_cell`). `doc_cell` is Ruby-free, so its `compile_fail`
doctests pin that a guard's loan cannot outlive it and that a guard stays on
its thread.

The `DocData` owns the content in a `DocCell`, and each access is a counted
borrow:

- a read: `HtmlRead` / `XmlRead`, or the receiver views `HtmlSelf::read` /
  `XmlSelf::read`;
- an edit: `HtmlWrite` / `XmlWrite`, taken only inside
  `HtmlEdit::with_tree`, `new_node`, `Editing::with_arena` and their kin.

No raw pointer to the content is kept or handed out, and none of its old
`unsafe` contract ("nothing edits it while you hold this") is left to a call
site. An edit that begins while any read is held - a copy of the receiver -
is refused as `Makiri::Error`, and a read during an edit is an
`InternalError`. An edit that changes two documents (an adoption: the copy
placed, the original released) takes both writes before it changes either,
so it is refused whole or done whole.

A guard is a `Drop`, and a raise past one would leave the document borrowed
for good. No raise crosses one, since every raising call returns `Err` (see
"C calls that can raise"). `rake oom`'s raise sweep makes every allocating
call raise in turn and checks that no document is left borrowed.

So a reader builds its result with the read held (`str_from_read`,
`node_set_from_read`). The copy-out-then-build it used to need ("lifting")
went with stage 4 of `notes/ruby_boundary_simplification_plan.ja.md`, and
the light readers got faster for it (`#text` ~124 -> ~112 ns).

## InternalError, not fatal

**A Ruby method raises `Makiri::InternalError`, not `fatal`.** Every
`method!`/`function!` expands to a trampoline whose body runs inside
`bridge::rb::method::finish`, which catches a panic and raises that
exception, in the test hooks too. So no method needs a wrapper of its own;
the per-method `bridge::ruby::entry` and its gate went with magnus.

`InternalError` descends from `Exception`, *not* `StandardError`, and that
is the point. A bare `rescue => e` keeps passing it through, because a
broken invariant is not a bad selector. A host that wants to turn one
request into a 500 can still catch it, without a thread boundary to re-raise
at; a `fatal` cannot be rescued in its own frame at all.

Outside a registered method a panic does not become `InternalError`:

- The GC callbacks abort (see "Panics and unwinding").
- In `Init_makiri`, a panic - or an `Err` - once the extension has begun to
  define itself is `fatal`, since `InternalError` may not exist yet and a
  half-defined extension must not be used. Only the check that comes first,
  the Ruby API one, raises an ordinary `LoadError`
  (`bridge::rb::method::run_init(check, init)`).

## C integers

**A C integer is converted in one place, `nupur::int`.** A C `long` is 32
bits on Windows and `i64` itself everywhere the code is built and linted, so
`n as c_long` truncates where no compiler warning can show it. Ruby's ranges
follow the `long` too: a Fixnum is 2^30 - 1 at most on Windows. Each Windows
bug used to be found by review.

`int` writes the rules once over a `Long` of either width, tested at both on
any machine and held to rb-sys's `FIXNUM_MAX` / `fixable`. Outside it,
`rake dylint`'s `c_integer_outside_int` refuses an integer `as` cast and any
naming of `c_long` / `c_ulong` in nupur and in `bridge` / `glue` / `init`.
Convert with `From` / `TryFrom`; a length is a `usize`, an index an `i64`.
Clippy's cast lints, on in nupur, see the real types on CI's Windows cell.

## Panic lints and the panic hook

`clippy::unwrap_used`, `clippy::expect_used` and `clippy::panic` (in
`Cargo.toml`) keep a new panic from arriving by accident; a site that wants
one carries an `#[allow]` with a reason.

`spec/panic_spec.rb` drives `Makiri.__panic(kind)`:

- kind 4 panics below the GVL-release frame. This is the case that proves the
  latch, since without it that panic aborts.
- kind 5 panics in a method body.
- kind 6 panics in the token-done path of a real document parse
  (`html::tree`'s `parse_with_panicking_hook`, its own sink, so no real parse
  carries it).

## Hardening flags and exports

The C-era hardening flags (`-D_FORTIFY_SOURCE=2`,
`-fstack-protector-strong`, `-fvisibility=hidden`, `-Wformat-security`) are
**gone rather than relaxed**: they hardened C sources, and there are none.

The job of `-fvisibility=hidden` is the one that survives, and it is
enforced at the source: nothing but `Init_makiri` is `#[no_mangle]`, so
rustc emits no other exported name. That is not a stylistic choice; on ELF it
is the only thing that works:

- rustc owns the cdylib link and passes its own export list;
- a second `--version-script` is merged rather than applied, so it cannot
  narrow;
- `objcopy`/`strip` cannot remove an entry from a linked `.so`'s `.dynsym`
  at all.

The post-link trim in `extconf.rb` survives as belt-and-braces on macOS,
where `strip -u -r -s` does work; do not mistake it for the mechanism.

UBSan is gone for the same reason: rustc's `-Zsanitizer` has no `undefined`,
and there is no C left for `-fsanitize=undefined` to instrument.
