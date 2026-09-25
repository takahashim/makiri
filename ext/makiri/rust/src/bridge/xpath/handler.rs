//! The custom-function handler bridge: an unknown XPath function, dispatched to
//! a Ruby handler object under `rb_protect`, its arguments and result
//! converted between engine and Ruby values.

#![allow(unsafe_code)]

use magnus::rb_sys::AsRawValue;
use magnus::value::ReprValue;
use magnus::{Error, Ruby, Value};

use crate::bridge::node_set::NodeSet as RubyNodeSet;
use crate::bridge::ruby::VALUE;
use crate::bridge::string::ruby_try_verified_text;
use crate::bridge::wrapper::{keepalive_document, node_raw};
use crate::init::{CLASS_NODE, CLASS_NODE_SET};
use crate::token::Kind;
use crate::xpath::ctx::{Resolver, ResolverCall};
use crate::xpath::limits::Budget;
use crate::xpath::msg::{Reported, Status};
use crate::xpath::value::{NodeSet, Text, Val};

use super::*;

/// Upper bound on handler arguments, matching the engine's default
/// `max_function_args`. The resolver refuses any call above it, so the fixed
/// argv array below cannot overflow however the limit is tuned - and the stack
/// use stays independent of the runtime argument count.
const HANDLER_MAX_ARGS: usize = 64;
/* ------------------------------------------------------------------ */
/* the custom-function handler bridge                                 */
/* ------------------------------------------------------------------ */

/* When an expression calls a function the engine does not know, it delegates to
 * a resolver. The one installed here dispatches to a Ruby handler object - the
 * method name is the XPath local name with '-' mapped to '_' - converting
 * arguments and the return value between engine and Ruby values. The call runs
 * under rb_protect, so a Ruby exception becomes a clean engine error rather than
 * a longjmp through the evaluator's C stack. */

pub(super) struct Bridge {
    pub(super) handler: VALUE,
    /// Keepalive, and the document node-set arguments are wrapped under.
    pub(super) document: VALUE,
    /// Which backend the document is, for minting a handler's node token.
    pub(super) kind: Kind,
    /// Every mutator on `document` refuses while this lives.
    pub(super) _reading: crate::bridge::wrapper::DocumentEvaluation,
}

// The bridge holds `document`'s evaluation guard for as long as it exists,
// so a handler cannot change the document mid-walk; and `push_result_node`
// admits only nodes whose document is `document`.
impl Resolver for Bridge {
    fn resolve(
        &self,
        budget: &mut Budget,
        call: &ResolverCall<'_>,
    ) -> Result<Option<Val>, Reported> {
        // SAFETY: called by the engine mid-evaluate, under the GVL.
        unsafe { handler_resolver(self, budget, call) }
    }
}

/// Why a handler's call could not be completed. Every message is static text -
/// a static reason at most - so a failure is two words, allocates nothing on a
/// path that is already failing, and is worded only when reported.
#[derive(Clone, Copy, Debug)]
enum HandlerFailure {
    Msg(&'static str),
    /// The handler's string failed the text contract, for this reason.
    InvalidString(&'static core::ffi::CStr),
}

impl HandlerFailure {
    /// The failure as the engine error that ends the evaluation.
    fn report(self, err: crate::xpath::msg::ErrSink) -> Reported {
        match self {
            HandlerFailure::Msg(m) => crate::err_setf!(err, Status::Runtime, "{m}"),
            HandlerFailure::InvalidString(reason) => crate::err_setf!(
                err,
                Status::Runtime,
                "handler returned an invalid string: {}",
                reason.to_string_lossy()
            ),
        }
    }
}

/// Validate a handler-returned node and push it into the result node-set.
///
/// The same-document check compares the node's keepalive Document VALUE against
/// the context's, NOT the HTML-only owner_document field - so it is correct for
/// an XML node too, whose pointer is an arena node rather than a Lexbor one. A
/// node from another document fails closed.
fn push_result_node(
    bridge: &Bridge,
    budget: &mut Budget,
    rb_node: Value,
    set: &mut NodeSet,
) -> Result<(), HandlerFailure> {
    let Ok(node_document) = keepalive_document(rb_node) else {
        return Err(HandlerFailure::Msg("handler returned an unusable node"));
    };
    if node_document.as_raw() != bridge.document {
        return Err(HandlerFailure::Msg(
            "handler returned a node from a different document",
        ));
    }
    let Ok(n) = node_raw(rb_node) else {
        return Err(HandlerFailure::Msg("handler returned an unusable node"));
    };
    /* Same-document is checked above, so this is a node of the context's kind. */
    // SAFETY: a live node of the context's own document.
    let Some(token) = (unsafe { node_token(bridge.kind, n) }) else {
        return Err(HandlerFailure::Msg("handler returned an unusable node"));
    };
    set.push_token(token, budget)
        .map_err(|_| HandlerFailure::Msg("out of memory building handler result"))
}

/// A handler's Ruby return value as an engine value.
fn ruby_to_val(bridge: &Bridge, budget: &mut Budget, rv: Value) -> Result<Val, HandlerFailure> {
    if let Some(b) = crate::bridge::ruby::bool_value(rv.as_raw()) {
        return Ok(Val::boolean(b));
    }
    let ruby = Ruby::get_with(rv);
    if is_numeric(&ruby, rv) {
        return f64::try_convert(rv)
            .map(Val::number)
            .map_err(|_| HandlerFailure::Msg("handler returned a number that could not be read"));
    }
    let is_node = is_kind_of(rv, &CLASS_NODE);
    if is_node || is_kind_of(rv, &CLASS_NODE_SET) {
        let mut set = NodeSet::new();
        if is_node {
            push_result_node(bridge, budget, rv, &mut set)?;
        } else {
            let Ok(source) = <&RubyNodeSet as magnus::TryConvert>::try_convert(rv) else {
                return Err(HandlerFailure::Msg("handler result could not be read"));
            };
            let Ok(count) = source.count() else {
                return Err(HandlerFailure::Msg("handler result could not be read"));
            };
            for i in 0..count {
                let Ok(Some(node)) = source.at(&ruby, i) else {
                    return Err(HandlerFailure::Msg("handler result could not be read"));
                };
                push_result_node(bridge, budget, node, &mut set)?;
            }
        }
        return Ok(Val::nodeset(set));
    }

    /* nil and everything else: coerce to a string (nil -> ""). */
    if rv.is_nil() {
        return Ok(Val::string(Text::default()));
    }
    /* A `to_s` that raises, or returns something other than a String, is
     * refused here rather than read as a String. */
    let Ok(sv) = crate::bridge::ruby::to_s(rv) else {
        return Err(HandlerFailure::Msg(
            "handler result could not be converted to a string",
        ));
    };
    let vv = ruby_try_verified_text(sv, budget.limits.max_string_bytes)
        .map_err(HandlerFailure::InvalidString)?;
    Text::try_copy(vv.as_verified().as_bytes())
        .map(Val::string)
        .ok_or(HandlerFailure::Msg(
            "out of memory converting handler result",
        ))
}

/// Integer and Float both become an XPath number; a String that happens to look
/// numeric does not - the C tested the types, not convertibility.
fn is_numeric(ruby: &Ruby, v: Value) -> bool {
    v.is_kind_of(ruby.class_integer()) || v.is_kind_of(ruby.class_float())
}

/// Everything the protected call needs, and what it produced. `argv` is a
/// fixed array, so the stack use does not depend on the runtime argument count.
struct HandlerCall<'c> {
    bridge: &'c Bridge,
    budget: &'c mut Budget,
    method: crate::bridge::ruby::ID,
    args: &'c [Val],
    /// Set by the body; `None` only when a raise cut it short, which the
    /// protected call reports instead.
    result: Option<Result<Val, HandlerFailure>>,
    argv: [VALUE; HANDLER_MAX_ARGS],
}

/// Runs under `rb_protect`: build the Ruby arguments, invoke the handler,
/// convert the result.
/* A plain Rust fn, NOT `extern "C"`: it is only ever called from the
 * closure below, and that ABI would turn a panic in it into an abort
 * at its own boundary - before `bridge::ruby::protect`'s latch could
 * see it. Nothing passes it to C as a function pointer. */
fn handler_call_body(c: &mut HandlerCall<'_>) {
    // SAFETY: the Document the evaluation holds, which the bridge keeps alive.
    let document = unsafe { crate::bridge::ruby::value(c.bridge.document) };
    for (slot, arg) in c.argv.iter_mut().zip(c.args) {
        // SAFETY: this body runs under `protect`, as `val_to_ruby` asks.
        match unsafe { val_to_ruby(arg.get(), document) } {
            Ok(v) => *slot = v,
            Err(_) => {
                /* Only the size cap or a busy set refuses a push. */
                c.result = Some(Err(HandlerFailure::Msg(
                    "handler argument node-set could not be built",
                )));
                return;
            }
        }
    }
    // SAFETY: under `protect` (the handler's raise comes back as `Err`), with
    // the live handler and the arguments just built.
    let r = unsafe {
        crate::bridge::ruby::funcallv(c.bridge.handler, c.method, &c.argv[..c.args.len()])
    };
    // SAFETY: the live VALUE the handler returned.
    let rv = unsafe { crate::bridge::ruby::value(r) };
    c.result = Some(ruby_to_val(c.bridge, c.budget, rv));
}

/// A Ruby exception out of the handler - its `respond_to?` or the call itself -
/// as the engine error that fails the evaluation.
fn handler_raised(err: crate::xpath::msg::ErrSink, e: &Error) -> Reported {
    crate::err_setf!(
        err,
        Status::Runtime,
        "handler raised: {}",
        crate::bridge::ruby::error_message(e)
    )
}

/// The engine's resolver hook: the Ruby handler's method for the call, or
/// `Ok(None)` when the handler has no such method and the engine reports the
/// function unknown.
unsafe fn handler_resolver(
    bridge: &Bridge,
    budget: &mut Budget,
    call: &ResolverCall<'_>,
) -> Result<Option<Val>, Reported> {
    let err = budget.sink();

    /* The method name: XPath uses '-', Ruby uses '_'. The buffer starts zeroed,
     * so the copy stays NUL-terminated. */
    let mut name = [0u8; 128];
    let n = call.local.len();
    if n >= name.len() {
        return Ok(None); /* too long to map to a Ruby method name */
    }
    for (dst, &b) in name.iter_mut().zip(call.local) {
        *dst = if b == b'-' { b'_' } else { b };
    }

    let method = crate::bridge::ruby::intern(&name);
    /* `respond_to?` - and `respond_to_missing?` behind it - is the handler's own
     * Ruby code, so it is asked under protect: a raise there fails this call like
     * any handler raise, instead of unwinding past the evaluation's guards. */
    match crate::bridge::ruby::respond_to(crate::bridge::ruby::value(bridge.handler), method) {
        Ok(true) => {}
        Ok(false) => return Ok(None), /* let the engine raise "unknown function" */
        Err(e) => return Err(handler_raised(err.clone(), &e)),
    }

    if call.args.len() > HANDLER_MAX_ARGS {
        return Err(crate::err_setf!(
            err,
            Status::Runtime,
            "handler function '{}' called with too many arguments ({} > {})",
            core::str::from_utf8_unchecked(&name[..n]),
            call.args.len(),
            HANDLER_MAX_ARGS
        ));
    }

    let mut state = HandlerCall {
        bridge,
        budget,
        method,
        args: call.args,
        result: None,
        argv: [crate::bridge::ruby::nil().as_raw(); HANDLER_MAX_ARGS],
    };

    /* The call runs under `protect`: the body builds the arguments and converts
     * the result, and any of those can raise. Whatever it produced is owned by
     * `state.result`, so every failure below frees it. */
    let called = crate::bridge::ruby::protect_value(|| {
        handler_call_body(&mut state);
        crate::bridge::ruby::nil().as_raw()
    });
    if let Err(e) = called {
        return Err(handler_raised(err, &e));
    }
    match state.result {
        Some(Ok(v)) => Ok(Some(v)),
        Some(Err(failure)) => Err(failure.report(err)),
        /* The body sets a result on every path that returns normally. */
        None => Err(crate::err_setf!(
            err,
            Status::Runtime,
            "handler produced no result"
        )),
    }
}
