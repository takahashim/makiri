//! The engine's driver: the context an expression is evaluated against - the
//! document it walks, the context node, the namespace and variable registries,
//! and the caps - plus the two evaluate entries.
//!
//! The context is generic over the `Dom` backend, so this module names no
//! representation: the HTML and XML contexts are built by their own layers
//! (`lexbor::xpath`, `xml::xpath`) and the Ruby glue holds whichever it was
//! given.

#![forbid(unsafe_code)]

use super::abi::*;
use super::dom::Dom;
use super::eval;
use crate::falloc::{MapInsert, Reserve};
use crate::token::Token;
use core::cell::{Cell, Ref, RefCell, RefMut};
use core::marker::PhantomData;
use std::collections::HashMap;

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
/// A resolver answers nodes of the document this evaluate walks; the Ruby
/// bridge upholds that by checking a handler's node's document before it mints
/// a token, so the evaluator can read an answer's tokens back without checking
/// again.
pub trait Resolver {
    fn resolve(
        &self,
        budget: &mut Budget,
        call: &ResolverCall<'_>,
    ) -> Result<Option<Val>, Reported>;
}

/// Per-context registration caps. These bound an abusive Ruby loop that calls
/// register_namespace / register_variable without limit; far above any real use.
pub const MAX_NAMESPACES: usize = 65536;
const MAX_VARIABLES: usize = 65536;

/// A registered namespace; its prefix is the key it is found by in
/// `Names::ns_index`.
struct NsEntry {
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
    /// prefix -> its entry in `ns`. A registration looked its prefix up by a
    /// scan, so registering n prefixes cost n squared: a 65,000-pair Hash took
    /// six seconds of CPU with the GVL held.
    ns_index: HashMap<Box<[u8]>, usize>,
    vars: Vec<VarEntry>,
}

impl Names {
    /// The URI registered for `prefix`.
    ///
    /// `xml` needs no registration and takes none: Namespaces in XML binds it
    /// to its own URI by definition and forbids binding it elsewhere, and
    /// libxml2 (so Nokogiri) answers it before looking at the registrations.
    pub fn lookup_ns(&self, prefix: &[u8]) -> Option<&[u8]> {
        if prefix == b"xml" {
            return Some(crate::xml::XML_NS_URI);
        }
        self.ns_index
            .get(prefix)
            .map(|&i| self.ns[i].uri.as_slice())
    }

    /// The URI registered for `prefix`, or the RUNTIME error an expression
    /// naming an unbound prefix gets - the one spelling of it, for name tests
    /// and function calls alike.
    pub fn resolve_prefix(&self, prefix: &[u8], err: ErrSink) -> Result<&[u8], Reported> {
        self.lookup_ns(prefix).ok_or_else(|| {
            crate::err_setf!(
                err,
                XP_ERR_RUNTIME,
                "unknown namespace prefix '{}'",
                super::msg::Bytes(prefix)
            )
        })
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

/// Why a context refused a change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextError {
    /// An evaluate on this context is running (a handler re-entered).
    Evaluating,
    /// Out of memory, or past the registration cap.
    Failed,
}

/// An expression's surroundings: the document it is evaluated against, the
/// context node, the registrations it may name, and the caps its runs start
/// from.
///
/// `D` is the backend's document handle (a Lexbor handle for HTML, a borrowed
/// `&xml::Document` for XML), which is why this type is `Context<'d, D>`: a
/// context made from a borrowed XML document cannot outlive it, while the glue,
/// which keeps the document alive through Ruby, makes `Context<'static, _>` and
/// states that contract itself.
///
/// An evaluate needs only `&self`, so a handler may evaluate again on the same
/// context. What may change while one runs is kept in cells; changes that would
/// disturb the walk are refused with [`ContextError::Evaluating`].
pub struct Context<'d, D: Dom<'d>> {
    doc: D,
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

    /// `D` names the backend but does not syntax-use `'d` when it is a plain
    /// borrow type; this ties the context's lifetime to a document borrow.
    _doc: PhantomData<&'d ()>,
}

impl<'d, D: Dom<'d>> Context<'d, D> {
    /// A context over `doc`, with `node` ([`Token::null`] for none) as the
    /// context node.
    ///
    /// Safe: `doc` carries its own contract (a live document that is not
    /// restructured while the context lives), and the node is an opaque token
    /// the backend resolves.
    pub fn new(doc: D, node: Token) -> Context<'d, D> {
        Context {
            doc,
            node: Cell::new(node),
            names: RefCell::new(Names::default()),
            limits: Limits::DEFAULT,
            lax: false,
            evaluating: Cell::new(0),
            _doc: PhantomData,
        }
    }

    /// The backend document handle.
    pub fn doc(&self) -> D {
        self.doc
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
        if let Some(&i) = names.ns_index.get(prefix) {
            /* Copy first, so an OOM leaves the old binding in place. */
            names.ns[i].uri = copy(uri)?;
            return Ok(());
        }
        if names.ns.len() >= MAX_NAMESPACES
            || names.ns.falloc_reserve(1).is_err()
            || names.ns_index.falloc_reserve(1).is_err()
        {
            return Err(ContextError::Failed);
        }
        let key = crate::falloc::try_to_boxed_slice(prefix).ok_or(ContextError::Failed)?;
        let entry = NsEntry { uri: copy(uri)? };
        /* The index first: `falloc_insert` reserves again, which is one more
         * injection site even with room already made, so it has to come before
         * anything that cannot be taken back. The push into the reserved `ns`
         * cannot fail. */
        let at = names.ns.len();
        names
            .ns_index
            .falloc_insert(key, at)
            .map_err(|()| ContextError::Failed)?;
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
        if names.vars.len() >= MAX_VARIABLES || names.vars.falloc_reserve(1).is_err() {
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
        if !self.doc.prepare() {
            return Err(self.index_error());
        }
        let run = self.enter()?;
        eval::eval_ast(self, &run.names, self.doc, self.focus_node(), ast, handler)
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
        if !self.doc.prepare() {
            return Err(self.index_error());
        }
        let matched = {
            let run = self.enter()?;
            eval::try_first_match(self, &run.names, self.doc, self.focus_node(), ast)
        }?;
        match matched {
            Some(value) => Ok(value),
            None => self.evaluate(ast, handler),
        }
    }

    /// Mark an evaluate as running for as long as the guard lives, and lend it
    /// the registrations.
    #[allow(clippy::result_large_err)]
    fn enter(&self) -> Result<Running<'_, 'd, D>, Error> {
        let Ok(names) = self.names.try_borrow() else {
            return Err(Error::with(
                XP_ERR_INTERNAL,
                format_args!("evaluate: the context is being changed"),
            ));
        };
        self.evaluating.set(self.evaluating.get() + 1);
        Ok(Running { cx: self, names })
    }

    /// The backend could not build the per-walk index: out of memory.
    fn index_error(&self) -> Error {
        Error::with(
            XP_ERR_OOM,
            format_args!("out of memory building the element index"),
        )
    }

    /// The context node, resolved through the backend.
    fn focus_node(&self) -> Option<D::Node> {
        let t = self.node.get();
        (!t.is_null()).then(|| self.doc.resolve_token(t))
    }
}

/// An evaluate in progress: the borrowed registrations, and the depth count it
/// gives back when it ends.
struct Running<'a, 'd, D: Dom<'d>> {
    cx: &'a Context<'d, D>,
    names: Ref<'a, Names>,
}

impl<'d, D: Dom<'d>> Drop for Running<'_, 'd, D> {
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

/// What a caller does with a context whatever backend it walks - the
/// document-independent part of [`Context`], as an object-safe trait.
///
/// The glue holds a context for either an HTML or an XML document, and used to
/// forward each of these nine operations through a two-armed `match`. With this
/// it picks the backend once and calls through `&dyn QueryContext`.
pub trait QueryContext {
    fn limits(&self) -> Limits;
    fn lax(&self) -> bool;
    fn set_lax(&mut self, lax: bool);
    fn is_evaluating(&self) -> bool;
    fn set_context_node(&self, node: Token) -> Result<(), ContextError>;
    fn register_ns(&self, prefix: &[u8], uri: &[u8]) -> Result<(), ContextError>;
    fn register_variable(&self, name: &[u8], value: &[u8]) -> Result<(), ContextError>;
    #[allow(clippy::result_large_err)]
    fn evaluate(&self, ast: &Ast, handler: Option<&dyn Resolver>) -> Result<XPathValue, Error>;
    #[allow(clippy::result_large_err)]
    fn evaluate_first(
        &self,
        ast: &Ast,
        handler: Option<&dyn Resolver>,
    ) -> Result<XPathValue, Error>;
}

/* Each forwards to the inherent method of the same name, which method
 * resolution prefers - so none of these calls itself. */
impl<'d, D: Dom<'d>> QueryContext for Context<'d, D> {
    fn limits(&self) -> Limits {
        Context::limits(self)
    }
    fn lax(&self) -> bool {
        Context::lax(self)
    }
    fn set_lax(&mut self, lax: bool) {
        Context::set_lax(self, lax)
    }
    fn is_evaluating(&self) -> bool {
        Context::is_evaluating(self)
    }
    fn set_context_node(&self, node: Token) -> Result<(), ContextError> {
        Context::set_context_node(self, node)
    }
    fn register_ns(&self, prefix: &[u8], uri: &[u8]) -> Result<(), ContextError> {
        Context::register_ns(self, prefix, uri)
    }
    fn register_variable(&self, name: &[u8], value: &[u8]) -> Result<(), ContextError> {
        Context::register_variable(self, name, value)
    }
    fn evaluate(&self, ast: &Ast, handler: Option<&dyn Resolver>) -> Result<XPathValue, Error> {
        Context::evaluate(self, ast, handler)
    }
    fn evaluate_first(
        &self,
        ast: &Ast,
        handler: Option<&dyn Resolver>,
    ) -> Result<XPathValue, Error> {
        Context::evaluate_first(self, ast, handler)
    }
}
