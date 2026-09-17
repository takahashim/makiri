//! The engine's driver: the context an expression is evaluated against - its
//! document and node, its namespace and variable registries, its caps - and the
//! two evaluate entries that pick the backend for it.

#![allow(unsafe_code)]

use super::abi::*;
use super::dom::Dom;
use super::eval;
use super::token::Token;
use crate::falloc::Reserve;
use core::cell::{Cell, Ref, RefCell, RefMut};
use core::ffi::c_void;
use core::marker::PhantomData;

/// One call the evaluator routes to the custom-function resolver.
pub struct ResolverCall<'a> {
    /// The focus: the context node's token, and its position.
    pub node: Token,
    pub pos: usize,
    pub size: usize,
    /// The namespace URI of the call's prefix, when it had one.
    pub ns_uri: Option<&'a [u8]>,
    /// The function's local name.
    pub local: &'a [u8],
    pub args: &'a [Val],
}

/// What answers the function calls an evaluate has no built-in for: the glue's
/// bridge to a Ruby handler, passed to one evaluate.
///
/// `Ok(Some(value))` answers the call; `Ok(None)` means there is no such
/// function, which the evaluator reports; `Err` is the function's own failure,
/// already written to the budget.
///
/// # Safety
/// The evaluator reads what a resolver answers as nodes of the document it is
/// walking, and keeps borrowing that document across the call. So an
/// implementation must answer only nodes of that document, and must not let the
/// document change while `resolve` runs.
pub unsafe trait Resolver {
    fn resolve(
        &self,
        budget: &mut Budget,
        call: &ResolverCall<'_>,
    ) -> Result<Option<Val>, Reported>;
}

/// Per-context registration caps. These bound an abusive Ruby loop that calls
/// register_namespace / register_variable without limit; far above any real use.
const MAX_NAMESPACES: usize = 65536;
const MAX_VARIABLES: usize = 65536;

struct NsEntry {
    prefix: Text,
    uri: Text,
}

struct VarEntry {
    /// None for the unprefixed (only supported) form.
    prefix: Option<Text>,
    name: Text,
    value: Text,
}

/// A context's namespace and variable registrations.
///
/// An evaluate borrows them for the whole walk - a name test keeps the URI it
/// resolved - so they sit in a `RefCell` on the context: a registration made
/// while an evaluate runs (a handler re-entering) cannot take the borrow, and
/// is refused rather than freeing a string the walk still reads.
#[derive(Default)]
pub struct Names {
    ns: Vec<NsEntry>,
    vars: Vec<VarEntry>,
}

impl Names {
    /// The URI registered for `prefix`.
    pub fn lookup_ns(&self, prefix: &[u8]) -> Option<&[u8]> {
        self.ns
            .iter()
            .find(|e| e.prefix.as_slice() == prefix)
            .map(|e| e.uri.as_slice())
    }

    /// The string bound to `$prefix:name` (`prefix` is `None` when unprefixed).
    pub fn variable_text(&self, prefix: Option<&[u8]>, name: &[u8]) -> Option<&[u8]> {
        self.vars
            .iter()
            .find(|e| {
                let prefix_match = match prefix {
                    None => e.prefix.is_none(),
                    Some(p) => e.prefix.as_ref().is_some_and(|t| t.as_slice() == p),
                };
                prefix_match && e.name.as_slice() == name
            })
            .map(|e| e.value.as_slice())
    }
}

/// Which representation a context walks, and the index its `//name` fast path
/// reads. Fixed when the context is made, so the backend that reads a node
/// handle always matches the document the handle came from.
#[derive(Clone, Copy)]
pub enum Backend {
    /// A Lexbor document, through its parse handle, which also carries the
    /// element index. Every evaluate reads the index afresh from the handle: a
    /// mutation between evaluates drops it, and the next evaluate rebuilds it.
    #[cfg(feature = "lexbor")]
    Html {
        parsed: *mut crate::lexbor::adapter::post_parse::Parsed,
    },
    /// A Makiri XML arena. Its element-name index hangs off the document itself
    /// and is built on first use.
    Xml {
        doc: *const crate::xml::model::Document,
    },
}

/// Why a context refused a change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextError {
    /// An evaluate on this context is running (a handler re-entered).
    Evaluating,
    /// Out of memory, or past the registration cap.
    Failed,
}

/// An expression's surroundings: the document and node it is evaluated
/// against, the registrations it may name, and the caps its runs start from.
///
/// The document is held as a pointer and borrowed afresh by each evaluate, so
/// it may change between evaluates. `'d` is how long it is lent: a context
/// made from `&'d Document` cannot outlive it, and the glue, which keeps the
/// document alive through Ruby, makes `Context<'static>` and states that
/// contract itself.
///
/// An evaluate needs only `&self`, so a handler may evaluate again on the same
/// context. What may change while one runs is kept in cells; changes that would
/// disturb the walk are refused with [`ContextError::Evaluating`].
pub struct Context<'d> {
    backend: Backend,
    node: Cell<Token>,
    names: RefCell<Names>,

    /* The caps every run under this context starts from. Each evaluate and
     * parse charges a `Budget` of its own made from these. */
    limits: Limits,

    /* Namespace matching for UNPREFIXED name tests. Strict (default) is
     * HTML5-faithful: an unprefixed name resolves in the HTML namespace, so
     * foreign SVG/MathML needs a prefix. Lax matches by local name. */
    lax: bool,

    /* Re-entrancy depth, >0 while an evaluate runs on this context. A nested
     * evaluate just stacks. */
    evaluating: Cell<usize>,

    _doc: PhantomData<&'d ()>,
}

impl<'d> Context<'d> {
    /// A context over `backend`'s document, with `node` (null for none) as the
    /// context node.
    ///
    /// # Safety
    /// For `'d`, the document and the index `backend` names must stay live, and
    /// `node` must be null or a node of that document. The document must not
    /// change while an evaluate on this context runs.
    pub unsafe fn new(backend: Backend, node: Token) -> Context<'d> {
        Context {
            backend,
            node: Cell::new(node),
            names: RefCell::new(Names::default()),
            limits: Limits::DEFAULT,
            lax: false,
            evaluating: Cell::new(0),
            _doc: PhantomData,
        }
    }

    /// A context over an XML document, rooted at `node`.
    pub fn xml(doc: &'d crate::xml::model::Document, node: crate::xml::NodeId) -> Context<'d> {
        let node = Token::from_ptr(node.to_token() as *mut c_void);
        // SAFETY: the document is borrowed for `'d`, so it lives and cannot be
        // changed while the context does; an XML node handle is checked on
        // every read, so any id is a valid one.
        unsafe { Context::new(Backend::Xml { doc }, node) }
    }

    pub fn backend(&self) -> Backend {
        self.backend
    }

    /// The caps a run under this context starts from.
    pub fn limits(&self) -> Limits {
        self.limits
    }

    pub fn limits_mut(&mut self) -> &mut Limits {
        &mut self.limits
    }

    /// namespace_matching: :lax - the unprefixed element rule is relaxed.
    pub fn lax(&self) -> bool {
        self.lax
    }

    pub fn set_lax(&mut self, lax: bool) {
        self.lax = lax;
    }

    /// True while an evaluate is in progress on this context, nested ones
    /// included.
    pub fn is_evaluating(&self) -> bool {
        self.evaluating.get() > 0
    }

    /// Rebind the context node. Refused while an evaluate runs, which read the
    /// node when it started.
    ///
    /// # Safety
    /// `node` must be null or a node of this context's document.
    pub fn set_context_node(&self, node: Token) -> Result<(), ContextError> {
        if self.is_evaluating() {
            return Err(ContextError::Evaluating);
        }
        self.node.set(node);
        Ok(())
    }

    fn names_mut(&self) -> Result<RefMut<'_, Names>, ContextError> {
        if self.is_evaluating() {
            return Err(ContextError::Evaluating);
        }
        self.names
            .try_borrow_mut()
            .map_err(|_| ContextError::Evaluating)
    }

    /// Bind `prefix` to `uri`, replacing an earlier binding. Both are copied.
    pub fn register_ns(&self, prefix: &[u8], uri: &[u8]) -> Result<(), ContextError> {
        let mut names = self.names_mut()?;
        if let Some(e) = names.ns.iter_mut().find(|e| e.prefix.as_slice() == prefix) {
            /* Copy first, so an OOM leaves the old binding in place. */
            e.uri = copy(uri)?;
            return Ok(());
        }
        if names.ns.len() >= MAX_NAMESPACES || names.ns.mkr_reserve(1).is_err() {
            return Err(ContextError::Failed);
        }
        let entry = NsEntry {
            prefix: copy(prefix)?,
            uri: copy(uri)?,
        };
        names.ns.push(entry);
        Ok(())
    }

    /// Bind the unprefixed variable `$name` to the string `value`, replacing an
    /// earlier binding. Both are copied.
    pub fn register_variable(&self, name: &[u8], value: &[u8]) -> Result<(), ContextError> {
        let mut names = self.names_mut()?;
        if let Some(e) = names
            .vars
            .iter_mut()
            .find(|e| e.prefix.is_none() && e.name.as_slice() == name)
        {
            e.value = copy(value)?;
            return Ok(());
        }
        if names.vars.len() >= MAX_VARIABLES || names.vars.mkr_reserve(1).is_err() {
            return Err(ContextError::Failed);
        }
        let entry = VarEntry {
            prefix: None,
            name: copy(name)?,
            value: copy(value)?,
        };
        names.vars.push(entry);
        Ok(())
    }

    /// Evaluate `ast` with the context node as the focus; `handler` answers the
    /// function calls there is no built-in for.
    ///
    /// Each call runs on an evaluation of its own - its budget, its caches - and
    /// only reads the context, so a handler that evaluates again on this same
    /// context cannot disturb the walk it was called from.
    #[allow(clippy::result_large_err)]
    pub fn evaluate(&self, ast: &Ast, handler: Option<&dyn Resolver>) -> Result<XPathValue, Error> {
        let run = self.enter()?;
        match self.backend {
            Backend::Xml { doc } => {
                // SAFETY: `new`'s contract - the document is live for `'d` and
                // unchanged while this evaluate runs.
                let Some(doc) = (unsafe { doc.as_ref() }) else {
                    return Err(self.no_document());
                };
                let node = self.focus_node(doc);
                eval::eval_ast(self, &run.names, doc, node, ast, handler)
            }
            #[cfg(feature = "lexbor")]
            Backend::Html { parsed } => {
                let doc = self.html_dom(parsed)?;
                let node = self.focus_node(doc);
                eval::eval_ast(self, &run.names, doc, node, ast, handler)
            }
        }
    }

    /// [`evaluate`](Self::evaluate) through the `at_xpath` first-match fast
    /// path when the shape allows it, and the full evaluator otherwise.
    #[allow(clippy::result_large_err)]
    pub fn evaluate_first(
        &self,
        ast: &Ast,
        handler: Option<&dyn Resolver>,
    ) -> Result<XPathValue, Error> {
        /* The fast path charges every visited node to a budget of its own, so it
         * is bounded fail-closed exactly like the full evaluator; it only runs
         * for recognised shapes, which call no functions, so it needs no
         * handler. */
        let matched = {
            let run = self.enter()?;
            match self.backend {
                Backend::Xml { doc } => {
                    // SAFETY: as in `evaluate`.
                    match unsafe { doc.as_ref() } {
                        Some(doc) => {
                            eval::try_first_match(self, &run.names, doc, self.focus_node(doc), ast)
                        }
                        None => Ok(None),
                    }
                }
                #[cfg(feature = "lexbor")]
                Backend::Html { parsed } => {
                    let doc = self.html_dom(parsed)?;
                    eval::try_first_match(self, &run.names, doc, self.focus_node(doc), ast)
                }
            }
        }?;
        match matched {
            Some(value) => Ok(value),
            None => self.evaluate(ast, handler),
        }
    }

    /// The HTML document an evaluate reads, with the element index as it is
    /// now - rebuilt if a mutation since the last evaluate dropped it.
    ///
    /// The index is required, not an optimisation: building it also backfills
    /// each attribute's parent, which the parent and ancestor axes read. An
    /// evaluate that cannot build it fails closed rather than answer wrongly.
    #[cfg(feature = "lexbor")]
    #[allow(clippy::result_large_err)]
    fn html_dom<'e>(
        &self,
        parsed: *mut crate::lexbor::adapter::post_parse::Parsed,
    ) -> Result<crate::xpath::dom_html::HtmlDom<'e>, Error> {
        // SAFETY: `new`'s contract - the handle is live for `'d`, and its
        // document does not change while this evaluate runs.
        let Some(parsed) = (unsafe { parsed.as_mut() }) else {
            return Err(self.no_document());
        };
        let raw_doc = parsed.html_doc() as *mut crate::lexbor_abi::LxbDoc;
        // SAFETY: as above.
        let Some(doc) = (unsafe { crate::lexbor::adapter::html::HtmlDoc::from_raw(raw_doc) }) else {
            return Err(self.no_document());
        };
        let Some(index) = parsed.dom_index() else {
            let budget = Budget::with_limits(self.limits);
            let _ = crate::err_setf!(
                budget.sink(),
                XP_ERR_OOM,
                "out of memory building the attribute index"
            );
            return Err(budget.take_error());
        };
        let index = index as *const crate::lexbor::adapter::dom_index::DomIndex;
        // SAFETY: the index has an allocation of its own, which only a mutation
        // frees, and none runs during this evaluate (a nested one only reads
        // it), so it outlives the borrow of the handle it came from.
        let index = unsafe { &*index };
        Ok(crate::xpath::dom_html::HtmlDom::new(doc, index))
    }

    /// Mark an evaluate as running for as long as the guard lives, and lend it
    /// the registrations.
    #[allow(clippy::result_large_err)]
    fn enter(&self) -> Result<Running<'_, 'd>, Error> {
        let Ok(names) = self.names.try_borrow() else {
            let budget = Budget::with_limits(self.limits);
            let _ = crate::err_setf!(
                budget.sink(),
                XP_ERR_INTERNAL,
                "evaluate: the context is being changed"
            );
            return Err(budget.take_error());
        };
        self.evaluating.set(self.evaluating.get() + 1);
        Ok(Running { cx: self, names })
    }

    /// The context node, read from its handle for the document `doc` this
    /// evaluate borrowed.
    fn focus_node<'e, D: Dom<'e>>(&self, doc: D) -> Option<D::Node> {
        let t = self.node.get();
        (!t.is_null()).then(|| doc.resolve_token(t))
    }

    fn no_document(&self) -> Error {
        let budget = Budget::with_limits(self.limits);
        let _ = crate::err_setf!(budget.sink(), XP_ERR_RUNTIME, "evaluate with no document");
        budget.take_error()
    }
}

/// An evaluate in progress: the borrowed registrations, and the depth count it
/// gives back when it ends.
struct Running<'a, 'd> {
    cx: &'a Context<'d>,
    names: Ref<'a, Names>,
}

impl Drop for Running<'_, '_> {
    fn drop(&mut self) {
        self.cx.evaluating.set(self.cx.evaluating.get() - 1);
    }
}

fn copy(bytes: &[u8]) -> Result<Text, ContextError> {
    Text::try_copy(bytes).ok_or(ContextError::Failed)
}

/// The result of an evaluate, owned: dropping it frees the node-set's array or
/// the string.
pub type XPathValue = Val;
