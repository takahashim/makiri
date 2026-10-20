//! The custom-function handler bridge: an unknown XPath function, dispatched to
//! a Ruby handler object under `rb_protect`, its arguments and result
//! converted between engine and Ruby values.

#![allow(unsafe_code)]

use magnus::rb_sys::AsRawValue;
use magnus::value::ReprValue;
use magnus::{Error, RString, Ruby, Value};

use crate::bridge::node_set::NodeSet as RubyNodeSet;
use crate::bridge::ruby::VALUE;
use crate::bridge::string::ruby_try_verified_text;
use crate::bridge::wrapper::{node_token_in, NotInDocument};
use crate::engine_error::{ErrorKind, Reported};
use crate::init::{CLASS_NODE, CLASS_NODE_SET};
use crate::xpath::ctx::{Resolver, ResolverCall};
use crate::xpath::limits::Budget;
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
 * a longjmp through the evaluator's C stack. The exception itself is kept in
 * `Bridge::raised`, so the evaluate boundary can re-raise one that is not the
 * handler's error but the caller's control flow (see `Bridge::propagates`). */

pub(super) struct Bridge {
    pub(super) handler: VALUE,
    /// Keepalive, and the document node-set arguments are wrapped under.
    pub(super) document: VALUE,
    /// Every mutator on `document` refuses while this lives.
    pub(super) _reading: crate::bridge::wrapper::DocumentEvaluation,
    /// The first Ruby exception (or `throw`) a handler-side call ended in.
    ///
    /// The bridge lives on `evaluate_query`'s stack frame, never on the heap,
    /// so the exception VALUE inside is seen by the GC's conservative stack
    /// scan for as long as it is held here.
    pub(super) raised: core::cell::OnceCell<Error>,
    /// The Fiber the handler's calls run in (`XPathContext.__handler_fiber`),
    /// `nil` until the first call makes it, and again once one left it
    /// unusable. On this frame's stack, as `raised` is, so the GC sees it.
    ///
    /// The point of it: the evaluator's frames never run in a Fiber the
    /// handler can suspend. A handler that switched Fibers from the caller's
    /// own - an `Enumerator` the evaluation runs inside, abandoned after one
    /// `next` - used to leave this frame suspended for good, with the
    /// document's evaluation guard held: every later edit refused, and
    /// nothing the frames owned ever freed.
    pub(super) fiber: core::cell::Cell<VALUE>,
    /// The methods the handler said it responds to, this evaluation: asked
    /// once each, so a call costs one Fiber switch rather than two. A handful
    /// of names at most, so a list; one that cannot grow only stops caching.
    pub(super) answered: core::cell::RefCell<Vec<crate::bridge::ruby::ID>>,
}

impl Bridge {
    /// Keep `e` for the evaluate boundary; only the first is kept, since the
    /// first one ends the evaluation.
    fn stash(&self, e: Error) {
        let _ = self.raised.set(e);
    }

    /// Run one op in the handler's Fiber: `recv.respond_to?(method)` when
    /// `args` is `None`, else the call `recv.method(*args)`. `recv` is the
    /// handler, or for the `to_s` of its result ([`Bridge::result_to_s`])
    /// that result: any of the handler's Ruby runs here. `None` when that
    /// Ruby switched Fibers instead of answering - its Fiber is then
    /// suspended in the handler, so it is dropped (it holds Ruby frames
    /// only, which Ruby frees with it).
    ///
    /// # Safety
    /// Under `protect`: making the Fiber, the op Array and the resume all
    /// raise, and the handler's own raise comes out of the resume. `recv`
    /// is live on the caller's frame, and `args` holds at most
    /// [`HANDLER_MAX_ARGS`] values.
    unsafe fn in_fiber(
        &self,
        recv: VALUE,
        method: crate::bridge::ruby::ID,
        args: Option<&[VALUE]>,
    ) -> Option<VALUE> {
        let ruby = crate::bridge::ruby::gvl_ruby();
        let nil = ruby.qnil().as_raw();
        let mut fiber = self.fiber.get();
        if fiber == nil {
            // SAFETY: under protect (the caller's contract); the class is live.
            fiber = unsafe {
                crate::bridge::ruby::funcallv(
                    crate::init::CLASS_XPATH_CONTEXT.raw(),
                    crate::bridge::ruby::intern("__handler_fiber"),
                    &[],
                )
            };
            self.fiber.set(fiber);
        }
        // SAFETY: `rb_id2sym` makes no object for an interned id.
        let name = unsafe { rb_sys::rb_id2sym(method) };
        let op = match args {
            None => ruby.ary_new_from_values(&[
                ruby.qfalse().as_value(),
                value_of(recv),
                value_of(name),
            ]),
            Some(a) => {
                /* Built in one raising call, not pushed: `RArray::push`
                 * returns its allocator's raise as `Err`, and a dropped one
                 * called the handler with arguments missing. Under protect,
                 * the raise fails the call instead. */
                let mut vals = [ruby.qnil().as_value(); HANDLER_MAX_ARGS];
                let mut n = 0;
                for (slot, &v) in vals.iter_mut().zip(a) {
                    *slot = value_of(v);
                    n += 1;
                }
                let argv = ruby.ary_new_from_values(&vals[..n]);
                ruby.ary_new_from_values(&[
                    ruby.qtrue().as_value(),
                    value_of(recv),
                    value_of(name),
                    argv.as_value(),
                ])
            }
        };
        // SAFETY: under protect; a live Fiber and a live op.
        let answer = unsafe {
            crate::bridge::ruby::funcallv(
                fiber,
                crate::bridge::ruby::intern("resume"),
                &[op.as_raw()],
            )
        };
        /* An answer is `[the Fiber, result]`; anything else is a value the
         * handler passed to a Fiber switch of its own. */
        let answer = magnus::RArray::from_value(value_of(answer))
            .filter(|a| a.len() == 2)
            .and_then(|a| Some((a.entry::<Value>(0).ok()?, a.entry::<Value>(1).ok()?)));
        match answer {
            Some((who, out)) if who.as_raw() == fiber => Some(out.as_raw()),
            _ => {
                self.fiber.set(nil);
                None
            }
        }
    }

    /// The handler's result as a String: [`crate::bridge::ruby::to_s`], with
    /// the `to_s` run in the handler's Fiber. It is the handler's Ruby too,
    /// and run on the caller's Fiber a `Fiber.yield` in it suspended the
    /// evaluation with the document's guard held, as a handler's own did.
    fn result_to_s(&self, rv: Value) -> Result<RString, HandlerFailure> {
        const NOT_A_STRING: &str = "handler result could not be converted to a string";
        let ruby = crate::bridge::ruby::gvl_ruby();
        if let Some(s) = RString::from_value(rv) {
            if rv.class().as_raw() == ruby.class_string().as_raw() {
                return Ok(s);
            }
        }
        let mut switched = false;
        let r = crate::bridge::ruby::protect(|| {
            // SAFETY: under protect, as `in_fiber` asks; `rv` is live on
            // this frame, and no argument is passed.
            match unsafe {
                self.in_fiber(rv.as_raw(), crate::bridge::ruby::intern("to_s"), Some(&[]))
            } {
                Some(v) => v,
                None => {
                    switched = true;
                    rb_sys::Qnil as VALUE
                }
            }
        });
        match r {
            Err(e) => Err(HandlerFailure::Raised(e, Some(NOT_A_STRING))),
            Ok(_) if switched => Err(HandlerFailure::Msg(SWITCHED)),
            Ok(v) => RString::from_value(value_of(v)).ok_or_else(|| {
                HandlerFailure::Raised(
                    crate::bridge::ruby::type_error("to_s returned a non-String"),
                    Some(NOT_A_STRING),
                )
            }),
        }
    }
}

/// A VALUE this module holds live (the handler, an argument, a Fiber's
/// answer), as a magnus `Value` for the Array calls.
fn value_of(v: VALUE) -> Value {
    // SAFETY: every caller passes a VALUE rooted by the evaluation's frame
    // (`Bridge`'s fields, the argv on this stack) or just returned by Ruby.
    unsafe { crate::bridge::ruby::value(v) }
}

/// What a handler that switched Fibers fails its call with.
const SWITCHED: &str =
    "handler switched Fibers (Fiber.yield, or an Enumerator's yielder, inside an XPath handler)";

/// Whether `e` must reach the caller of the query unchanged rather than become
/// `Makiri::Error`: anything that is not a `StandardError` - an interrupt,
/// `exit`, Timeout's own exception - and a non-local exit such as `throw`,
/// which is no exception at all.
pub(super) fn propagates(e: &Error) -> bool {
    !e.is_kind_of(crate::bridge::ruby::gvl_ruby().exception_standard_error())
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
/// a static reason at most - so a failure allocates nothing on a path that is
/// already failing, and is worded only when reported.
#[derive(Clone, Debug)]
enum HandlerFailure {
    Msg(&'static str),
    /// The handler's string failed the text contract, for this reason.
    InvalidString(&'static str),
    /// A Ruby call raised (or threw). Worded with the static text when there
    /// is one, else as "handler raised: <message>"; the exception itself goes
    /// to the bridge.
    Raised(Error, Option<&'static str>),
}

impl HandlerFailure {
    /// The failure as the engine error that ends the evaluation.
    fn report(self, bridge: &Bridge, err: crate::engine_error::ErrSink) -> Reported {
        match self {
            HandlerFailure::Msg(m) => crate::err_setf!(err, ErrorKind::Runtime, "{m}"),
            HandlerFailure::InvalidString(reason) => crate::err_setf!(
                err,
                ErrorKind::Runtime,
                "handler returned an invalid string: {reason}"
            ),
            HandlerFailure::Raised(e, wording) => {
                /* One the boundary re-raises unchanged is not worded at all:
                 * asking it for a message would run more Ruby, and for a
                 * `throw` there is none. */
                let reported = if propagates(&e) {
                    crate::err_setf!(err, ErrorKind::Runtime, "handler raised")
                } else if let Some(m) = wording {
                    crate::err_setf!(err, ErrorKind::Runtime, "{m}")
                } else {
                    handler_raised(err, &e)
                };
                bridge.stash(e);
                reported
            }
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
    // SAFETY: the live Document the bridge holds for this evaluation.
    let document = unsafe { crate::bridge::ruby::value(bridge.document) };
    let token = node_token_in(rb_node, document).map_err(|e| match e {
        NotInDocument::Unusable(e) => {
            HandlerFailure::Raised(e, Some("handler returned an unusable node"))
        }
        NotInDocument::Foreign => {
            HandlerFailure::Msg("handler returned a node from a different document")
        }
    })?;
    set.push(token, budget)
        .map_err(|_| HandlerFailure::Msg("out of memory building handler result"))
}

/// A handler's Ruby return value as an engine value.
fn ruby_to_val(bridge: &Bridge, budget: &mut Budget, rv: Value) -> Result<Val, HandlerFailure> {
    if let Some(b) = crate::bridge::ruby::bool_value(rv.as_raw()) {
        return Ok(Val::boolean(b));
    }
    let ruby = Ruby::get_with(rv);
    if is_numeric(&ruby, rv) {
        return f64::try_convert(rv).map(Val::number).map_err(|e| {
            HandlerFailure::Raised(e, Some("handler returned a number that could not be read"))
        });
    }
    let is_node = is_kind_of(rv, &CLASS_NODE);
    if is_node || is_kind_of(rv, &CLASS_NODE_SET) {
        let mut set = NodeSet::new();
        if is_node {
            push_result_node(bridge, budget, rv, &mut set)?;
        } else {
            const UNREADABLE: &str = "handler result could not be read";
            let unreadable = |e| HandlerFailure::Raised(e, Some(UNREADABLE));
            let source =
                <&RubyNodeSet as magnus::TryConvert>::try_convert(rv).map_err(unreadable)?;
            let count = source.count().map_err(unreadable)?;
            for i in 0..count {
                let Some(node) = source.at(&ruby, i).map_err(unreadable)? else {
                    return Err(HandlerFailure::Msg(UNREADABLE));
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
    let sv = bridge.result_to_s(rv)?;
    let vv = ruby_try_verified_text(sv, budget.limits.max_string_bytes)
        .map_err(HandlerFailure::InvalidString)?;
    Text::try_copy(vv.as_bytes())
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
    let Some(r) = (unsafe {
        c.bridge
            .in_fiber(c.bridge.handler, c.method, Some(&c.argv[..c.args.len()]))
    }) else {
        c.result = Some(Err(HandlerFailure::Msg(SWITCHED)));
        return;
    };
    // SAFETY: the live VALUE the handler returned.
    let rv = unsafe { crate::bridge::ruby::value(r) };
    c.result = Some(ruby_to_val(c.bridge, c.budget, rv));
}

/// A Ruby exception out of the handler - its `respond_to?` or the call itself -
/// as the engine error that fails the evaluation.
fn handler_raised(err: crate::engine_error::ErrSink, e: &Error) -> Reported {
    crate::err_setf!(
        err,
        ErrorKind::Runtime,
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

    /* The method name: XPath uses '-', Ruby uses '_'. */
    let mut buf = [0u8; 128];
    let n = call.local.len();
    if n > buf.len() {
        return Ok(None); /* too long to map to a Ruby method name */
    }
    for (dst, &b) in buf.iter_mut().zip(call.local) {
        *dst = if b == b'-' { b'_' } else { b };
    }
    /* The expression was checked UTF-8, and swapping one ASCII byte for
     * another keeps it so; a name that somehow is not has no method. */
    let Ok(name) = core::str::from_utf8(&buf[..n]) else {
        return Ok(None);
    };

    /* Under protect: this frame is inside an evaluation - its guards and the
     * engine's state are on the stack above - so a raise from the symbol
     * table must not unwind past it. */
    let method = match crate::bridge::ruby::intern_protected(name) {
        Ok(m) => m,
        Err(e) => return Err(HandlerFailure::Raised(e, None).report(bridge, err)),
    };
    /* `respond_to?` - and `respond_to_missing?` behind it - is the handler's own
     * Ruby code, so it is asked under protect, and in the handler's Fiber: a
     * raise there fails this call like any handler raise, and a Fiber switch
     * suspends no frame of the evaluator's. */
    let known = bridge
        .answered
        .try_borrow()
        .is_ok_and(|a| a.contains(&method));
    let mut switched = false;
    let answer = if known {
        Ok(rb_sys::Qtrue as VALUE)
    } else {
        crate::bridge::ruby::protect(|| {
            // SAFETY: under protect, as `in_fiber` asks.
            match unsafe { bridge.in_fiber(bridge.handler, method, None) } {
                Some(v) => v,
                None => {
                    switched = true;
                    rb_sys::Qfalse as VALUE
                }
            }
        })
    };
    match answer {
        Err(e) => return Err(HandlerFailure::Raised(e, None).report(bridge, err)),
        Ok(_) if switched => return Err(HandlerFailure::Msg(SWITCHED).report(bridge, err)),
        Ok(v) if v == rb_sys::Qfalse as VALUE || v == rb_sys::Qnil as VALUE => {
            return Ok(None); /* let the engine raise "unknown function" */
        }
        Ok(_) if !known => {
            if let Ok(mut a) = bridge.answered.try_borrow_mut() {
                use crate::falloc::VecPush;
                let _ = a.falloc_push(method);
            }
        }
        Ok(_) => {}
    }

    if call.args.len() > HANDLER_MAX_ARGS {
        return Err(crate::err_setf!(
            err,
            ErrorKind::Runtime,
            "handler function '{}' called with too many arguments ({} > {})",
            name,
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
        return Err(HandlerFailure::Raised(e, None).report(bridge, err));
    }
    match state.result {
        Some(Ok(v)) => Ok(Some(v)),
        Some(Err(failure)) => Err(failure.report(bridge, err)),
        /* The body sets a result on every path that returns normally. */
        None => Err(crate::err_setf!(
            err,
            ErrorKind::Runtime,
            "handler produced no result"
        )),
    }
}
