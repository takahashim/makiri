#![feature(rustc_private)]
#![warn(unused_extern_crates)]

//! The lints over the binding layer and the extension built on it, in two
//! groups. Paths are named as [`paths`] spells them: the layer's items by
//! its own crate name (`nupur::...`), the extension's by its path inside
//! [`paths::EXTENSION`].
//!
//! # The layer's guarantees (nupur)
//!
//! What `nupur/src/lib.rs` promises an extension, and what a copy of the
//! layer under another extension takes with it - these name only `nupur::`
//! and rb-sys paths:
//!
//! - `raising_call_outside_protect` and `unraising_call_in_raw`
//!   ([`raising`]): one table of the C functions that cannot raise, every
//!   other taken to, and what `nupur::raw` holds checked against it.
//! - `unguarded_no_unwind_site` ([`no_unwind_site`]): every `extern "C"`
//!   function and `Drop` runs under `no_unwind::must_not_unwind`.
//! - `ffi_without_gvl_proof` ([`gvl_proof`]): a call into Ruby's C API from a
//!   layer function with no proof of the GVL in its signature.
//! - `gvl_token_outside_entry` ([`entry`]): the GVL's token made anywhere but
//!   the entries Ruby calls.
//! - `forgotten_handle` ([`handles`]): a handle that holds a wrapped object
//!   forgotten.
//! - `discarded_ruby_error` ([`discarded`]): an `Err` of a Ruby error thrown
//!   away rather than handled with `Error::rescue`.
//! - `ruby_value_on_heap` ([`value_on_heap`]): a Ruby value kept where Ruby's
//!   GC cannot see it.
//! - `c_integer_outside_int` ([`c_integer`]): an integer `as` cast, or a C
//!   `long` named, at the Ruby boundary outside `nupur::int`.
//! - From [`boundaries`]: `raw_ruby_outside_bridge`'s half that holds a raw
//!   `VALUE` to the layer's files, `runs_callers_ruby` and
//!   `ruby_in_gvl_released_body`.
//!
//! # The extension's own layout (Makiri)
//!
//! Where Makiri draws its own lines - its engine, its glue and bridge, its
//! allocation policy - in [`boundaries`]: `ruby_layer_in_engine`,
//! `infallible_allocation`, `raw_ruby_outside_bridge`'s half that holds
//! rb-sys to `bridge/`, `raw_node_crossing`, `ruby_surface_in_bridge`. The
//! engine is a crate of its own (`makiri_engine`, [`paths::ENGINE`]) that
//! depends on no Ruby, so `ruby_layer_in_engine` has nothing to find there;
//! it holds nupur's one Ruby-free module, `utf8_copy`, which shares a crate
//! with the Ruby layer.
//!
//! (A second lint, `converted_argument`, checked that no registered method
//! took a type magnus would convert with the caller's Ruby. Since
//! `bridge::rb` replaced magnus, its `TryConvert` exists only for `Value`,
//! `bool` and Makiri's receiver types, so such a method does not compile.)
//!
//! `rake dylint` runs them all, then their self-test (`src/lint_probe.rs`
//! in the extension crate).

extern crate rustc_hir;
extern crate rustc_lint;
extern crate rustc_middle;
extern crate rustc_session;
extern crate rustc_span;

mod boundaries;
mod c_integer;
mod discarded;
mod entry;
mod gvl_proof;
mod handles;
mod no_unwind_site;
mod paths;
mod raising;
mod value_on_heap;

use rustc_hir::def_id::{DefId, LocalDefId};
use rustc_hir::intravisit::FnKind;
use rustc_hir::{Body, Expr, FnDecl, HirId, Item, LetStmt, Path};
use rustc_lint::{LateContext, LateLintPass, LintStore};
use rustc_session::{Session, declare_lint, declare_lint_pass};
use rustc_span::Span;
use std::cell::OnceCell;

dylint_linting::dylint_library!();

#[allow(clippy::no_mangle_with_rust_abi)]
#[unsafe(no_mangle)]
pub fn register_lints(sess: &Session, lint_store: &mut LintStore) {
    dylint_linting::init_config(sess);
    lint_store.register_lints(&[
        RUBY_VALUE_ON_HEAP,
        RUBY_LAYER_IN_ENGINE,
        UNGUARDED_NO_UNWIND_SITE,
        RUBY_IN_GVL_RELEASED_BODY,
        INFALLIBLE_ALLOCATION,
        RAISING_CALL_OUTSIDE_PROTECT,
        RAW_RUBY_OUTSIDE_BRIDGE,
        RUNS_CALLERS_RUBY,
        RAW_NODE_CROSSING,
        RUBY_SURFACE_IN_BRIDGE,
        DISCARDED_RUBY_ERROR,
        FFI_WITHOUT_GVL_PROOF,
        C_INTEGER_OUTSIDE_INT,
        UNRAISING_CALL_IN_RAW,
        GVL_TOKEN_OUTSIDE_ENTRY,
        FORGOTTEN_HANDLE,
    ]);
    lint_store.register_late_lint_pass(Box::new(|_| Box::new(MakiriLints)));
}

declare_lint! {
    /// ### What it does
    /// Reports a Ruby value (a type implementing Makiri's `ReprValue`) inside
    /// a heap container (`Box`, `Vec`, `VecDeque`, `HashMap`, `HashSet`,
    /// `BTreeMap`, `BTreeSet`, `Rc`, `Arc`, ...) - kept in a binding, a
    /// parameter, a return value, a field or a temporary no binding names
    /// (`f(vec![v])`) - in a `static` or `thread_local!`, or directly in the
    /// data of a wrapped Ruby object (a Makiri `Payload` type). A closure
    /// holds what it captures by value.
    ///
    /// ### Why is this bad?
    /// Ruby's GC finds values by scanning the machine stack. A value kept on
    /// the heap is not seen: it can be collected, or moved by compaction,
    /// while Rust still uses it.
    ///
    /// ### Example
    /// ```rust,ignore
    /// let kept: Vec<RString> = strings.collect();
    /// ```
    /// Use instead: keep the values on the stack, in a Ruby Array, or - in a
    /// wrapped object - in one of its slots (`TypedType`'s `roots`).
    pub RUBY_VALUE_ON_HEAP,
    Deny,
    "a Ruby value kept where Ruby's GC cannot see it"
}

declare_lint! {
    /// ### What it does
    /// Reports the engine (`html`, `xml`, `xpath`, ... - Ruby-free modules)
    /// naming anything of the Ruby layer: `bridge`, `glue`, `init`, `rb_sys`.
    ///
    /// ### Why is this bad?
    /// An edit's closure and a GVL-released body run engine code, and neither
    /// may reach Ruby: the one leaves the document's indexes dropped, the
    /// other has no GVL.
    pub RUBY_LAYER_IN_ENGINE,
    Deny,
    "the engine names something of the Ruby layer"
}

declare_lint! {
    /// ### What it does
    /// Reports an `extern "C"` function or a `Drop` whose body does not run
    /// under `no_unwind::must_not_unwind` (or a helper that does).
    ///
    /// ### Why is this bad?
    /// An unwind out of an `extern "C"` function aborts the process, as does
    /// a panic in a `Drop` the GC runs; `rake no_unwind` proves only the
    /// bodies that are sites.
    pub UNGUARDED_NO_UNWIND_SITE,
    Deny,
    "an extern \"C\" function or Drop not under must_not_unwind"
}

declare_lint! {
    /// ### What it does
    /// Reports a call, inside the closure given to `bridge::gvl::without_gvl`,
    /// to anything but the engine or a library.
    ///
    /// ### Why is this bad?
    /// With the GVL released no Ruby may run, and only the engine is known
    /// Ruby-free.
    pub RUBY_IN_GVL_RELEASED_BODY,
    Deny,
    "a GVL-released body calls what may reach Ruby"
}

declare_lint! {
    /// ### What it does
    /// Reports an allocation that aborts on OOM: `to_owned`, `to_vec`,
    /// `to_string`, `format!`, `from_utf8_lossy`, `into_owned`, `vec!`,
    /// `CString::new`.
    ///
    /// ### Why is this bad?
    /// An allocation failure must fail closed - an error, not the host
    /// process gone - so every allocation goes through `falloc`, and an
    /// exception message through `crate::msg::Msg`.
    pub INFALLIBLE_ALLOCATION,
    Deny,
    "an allocation that aborts on OOM"
}

declare_lint! {
    /// ### What it does
    /// Reports a call to a C function of rb-sys outside the closure given to
    /// `nupur::raw`'s `call`, unless the function is in the table of those
    /// that cannot raise ([`raising`]): an unclassified one is taken to raise.
    ///
    /// ### Why is this bad?
    /// A raise is a longjmp over every Rust frame up to the nearest
    /// `rb_protect`, which is undefined behaviour across a `Drop`.
    pub RAISING_CALL_OUTSIDE_PROTECT,
    Deny,
    "a C call not known to raise nothing, outside raw::call"
}

declare_lint! {
    /// ### What it does
    /// Reports, in `nupur::raw`, a call of Ruby's C outside `call`, and
    /// anywhere a `call` whose closure calls nothing that can raise.
    ///
    /// ### Why is this bad?
    /// `raw` is the C calls made under `rb_protect`; a read that raises
    /// nothing belongs with the values, and a protection of nothing means the
    /// table of what raises and the code disagree.
    pub UNRAISING_CALL_IN_RAW,
    Deny,
    "a call in raw outside raw::call, or a protected call of nothing that raises"
}

declare_lint! {
    pub RAW_RUBY_OUTSIDE_BRIDGE,
    Deny,
    "Ruby's C API outside bridge/, or a raw VALUE outside the typed values"
}

declare_lint! {
    pub RUNS_CALLERS_RUBY,
    Deny,
    "a call that runs the caller's Ruby code"
}

declare_lint! {
    pub RAW_NODE_CROSSING,
    Deny,
    "a node handle made a lifetime-free RawNode in bridge/"
}

declare_lint! {
    pub RUBY_SURFACE_IN_BRIDGE,
    Deny,
    "a Ruby method defined, or an argument list split, in bridge/"
}

declare_lint! {
    /// ### What it does
    /// Reports a call into Ruby's C API (`rb_sys::`) from a function of the
    /// binding layer whose signature holds no proof it runs with the GVL: a
    /// `&Ruby`, a Ruby value, or a handle made only from one.
    ///
    /// ### Why is this bad?
    /// Safe code could call such a function from a thread Ruby does not run,
    /// or with the GVL released, and Ruby's C functions touch the VM.
    pub FFI_WITHOUT_GVL_PROOF,
    Deny,
    "a call into Ruby's C API from a function that holds no proof of the GVL"
}

declare_lint! {
    /// ### What it does
    /// Reports, in the binding layer and the extension's Ruby boundary
    /// (`bridge`, `glue`, `init`) outside `nupur::int`, an `as` cast between
    /// integer types and any naming of `c_long` / `c_ulong`.
    ///
    /// ### Why is this bad?
    /// A C `long` is 32 bits on Windows and 64 where the code is linted, where
    /// `c_long` is `i64` itself - so a truncation there is invisible to rustc
    /// and clippy alike. The conversions are written once, in `nupur::int`.
    pub C_INTEGER_OUTSIDE_INT,
    Deny,
    "an integer cast or a C long at the Ruby boundary outside nupur::int"
}

declare_lint! {
    /// ### What it does
    /// Reports a call of `Ruby::get_unchecked` outside `nupur::rb::method`, of
    /// `rb::method::finish` / `finish_cary` outside a `method!` / `function!`
    /// expansion, and of `rb::method::run_init` outside an `extern "C" fn
    /// Init_*`.
    ///
    /// ### Why is this bad?
    /// Each makes the GVL's token, `Ruby`, which every call into Ruby takes
    /// as its proof; made where Ruby did not call in with the GVL held, the
    /// proof is false.
    pub GVL_TOKEN_OUTSIDE_ENTRY,
    Deny,
    "the GVL's token made outside the entries Ruby calls"
}

declare_lint! {
    /// ### What it does
    /// Reports a `typed::Typed` or `rb::kept::Kept` handed to `mem::forget`
    /// or wrapped in `ManuallyDrop`.
    ///
    /// ### Why is this bad?
    /// The handle's `Drop` is what keeps its object where the GC's stack scan
    /// sees it until after the last borrow of the object's data; forgotten,
    /// the object can be collected while its data is read.
    pub FORGOTTEN_HANDLE,
    Deny,
    "a Typed or Kept handle forgotten, so the object it holds may be collected"
}

declare_lint! {
    pub DISCARDED_RUBY_ERROR,
    Deny,
    "a Result of a Ruby error whose Err is thrown away"
}

declare_lint_pass!(MakiriLints => [
    RUBY_VALUE_ON_HEAP,
    RUBY_LAYER_IN_ENGINE,
    UNGUARDED_NO_UNWIND_SITE,
    RUBY_IN_GVL_RELEASED_BODY,
    INFALLIBLE_ALLOCATION,
    RAISING_CALL_OUTSIDE_PROTECT,
    RAW_RUBY_OUTSIDE_BRIDGE,
    RUNS_CALLERS_RUBY,
    RAW_NODE_CROSSING,
    RUBY_SURFACE_IN_BRIDGE,
    DISCARDED_RUBY_ERROR,
    FFI_WITHOUT_GVL_PROOF,
    C_INTEGER_OUTSIDE_INT,
    UNRAISING_CALL_IN_RAW,
    GVL_TOKEN_OUTSIDE_ENTRY,
    FORGOTTEN_HANDLE,
]);

impl<'tcx> LateLintPass<'tcx> for MakiriLints {
    fn check_item(&mut self, cx: &LateContext<'tcx>, item: &'tcx Item<'tcx>) {
        value_on_heap::check_item(cx, item);
    }

    fn check_local(&mut self, cx: &LateContext<'tcx>, local: &'tcx LetStmt<'tcx>) {
        value_on_heap::check_local(cx, local);
        discarded::check_local(cx, local);
    }

    fn check_fn(
        &mut self,
        cx: &LateContext<'tcx>,
        kind: FnKind<'tcx>,
        decl: &'tcx FnDecl<'tcx>,
        body: &'tcx Body<'tcx>,
        span: Span,
        did: LocalDefId,
    ) {
        value_on_heap::check_fn(cx, kind, decl, body, span, did);
        no_unwind_site::check_fn(cx, kind, body, span, did);
    }

    fn check_path(&mut self, cx: &LateContext<'tcx>, path: &Path<'tcx>, hir_id: HirId) {
        boundaries::check_path(cx, path, hir_id);
        c_integer::check_path(cx, path, hir_id);
    }

    fn check_expr(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
        value_on_heap::check_expr(cx, expr);
        boundaries::check_expr(cx, expr);
        discarded::check_expr(cx, expr);
        gvl_proof::check_expr(cx, expr);
        c_integer::check_expr(cx, expr);
        raising::check_expr(cx, expr);
        entry::check_expr(cx, expr);
        handles::check_expr(cx, expr);
    }
}

/// The traits the lint asks about, looked up once per crate: the crate's
/// own, since `bridge::rb` is the crate's.
/// Whether `did` is the extension's or its binding layer's (the crate
/// `nupur`, where `ReprValue`, `Payload` and `Opaque` are defined).
pub fn ours(cx: &LateContext<'_>, did: DefId) -> bool {
    did.is_local() || cx.tcx.crate_name(did.krate).as_str() == paths::NUPUR
}

#[derive(Default)]
struct Traits {
    repr_value: Option<DefId>,
    wrapped: Vec<DefId>,
}

thread_local! {
    static TRAITS: OnceCell<Traits> = const { OnceCell::new() };
}

/// `bridge::rb::ReprValue`, and the wrapped-object data trait,
/// `bridge::typed::Payload`.
pub(crate) fn traits<'tcx>(cx: &LateContext<'tcx>) -> (Option<DefId>, Vec<DefId>) {
    TRAITS.with(|cell| {
        let t = cell.get_or_init(|| {
            let mut t = Traits::default();
            for did in cx.tcx.all_traits_including_private() {
                if !ours(cx, did) {
                    continue;
                }
                match cx.tcx.item_name(did).as_str() {
                    "ReprValue" => t.repr_value = Some(did),
                    "Payload" => t.wrapped.push(did),
                    _ => {}
                }
            }
            t
        });
        (t.repr_value, t.wrapped.clone())
    })
}
