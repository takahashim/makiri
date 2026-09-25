//! The engine's driver: what an expression is evaluated under - the context
//! node, the namespace and variable registries, and the caps - plus the two
//! evaluate entries, which take the document they walk as an argument.
//!
//! [`Session`] is that state with no document in it; [`Context`] pairs one with
//! a document for a caller that holds both in one scope. Both are generic over
//! the `Dom` backend only at the evaluate, so this module names no
//! representation: the HTML and XML documents are lent by their own layers
//! (`lexbor::xpath`, `xml::xpath`).

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
    /// The focus: the context node's token (None when there is none), and its
    /// position.
    pub node: Option<Token>,
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
    /// Bind `prefix` to `uri`, replacing an earlier binding; both are copied.
    /// All or nothing: a failure leaves no trace, not even an unreachable
    /// entry the per-context cap would still count.
    ///
    /// The writer beside the reader ([`lookup_ns`](Self::lookup_ns)), so the
    /// invariant both rely on - an `ns_index` value is an index into `ns` - is
    /// kept in one place.
    fn bind_ns(&mut self, prefix: &[u8], uri: &[u8]) -> Result<(), ContextError> {
        if let Some(&i) = self.ns_index.get(prefix) {
            /* Copy first, so an OOM leaves the old binding in place. */
            self.ns[i].uri = copy(uri)?;
            return Ok(());
        }
        if self.ns.len() >= MAX_NAMESPACES
            || self.ns.falloc_reserve(1).is_err()
            || self.ns_index.falloc_reserve(1).is_err()
        {
            return Err(ContextError::Failed);
        }
        let key = crate::falloc::try_to_boxed_slice(prefix).ok_or(ContextError::Failed)?;
        let entry = NsEntry { uri: copy(uri)? };
        /* Both are reserved above, so neither write below can fail. The index
         * still goes first: if one ever could, a failed insert must not leave
         * a pushed entry the lookup cannot reach but the cap counts. */
        let at = self.ns.len();
        self.ns_index
            .falloc_insert(key, at)
            .map_err(|()| ContextError::Failed)?;
        self.ns.push(entry);
        Ok(())
    }

    /// Bind the unprefixed variable `$name` to `value`, replacing an earlier
    /// binding; both are copied.
    fn bind_var(&mut self, name: &[u8], value: &[u8]) -> Result<(), ContextError> {
        if let Some(e) = self
            .vars
            .iter_mut()
            .find(|e| e.prefix.is_none() && e.name.as_slice() == name)
        {
            e.value = copy(value)?;
            return Ok(());
        }
        if self.vars.len() >= MAX_VARIABLES || self.vars.falloc_reserve(1).is_err() {
            return Err(ContextError::Failed);
        }
        let entry = VarEntry {
            prefix: None,
            name: copy(name)?,
            value: copy(value)?,
        };
        self.vars.push(entry);
        Ok(())
    }

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
                ErrorKind::Runtime,
                "unknown namespace prefix '{}'",
                crate::engine_error::Bytes(prefix)
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

/// What an expression is evaluated under, apart from the document: the context
/// node, the registrations it may name, the caps its runs start from and the
/// namespace-matching mode.
///
/// It names no document, and that is the point. A holder that keeps a context
/// between calls - Ruby's `XPathContext`, for the Document's whole life - keeps
/// THIS, and lends the document to each evaluate as an argument, so no borrow of
/// the document outlives the one evaluate that reads it. A retained `&Document`
/// would still be live while a mutator writes the same arena through `&mut`
/// between two evaluates, which is undefined behaviour even though no read
/// overlaps the write. [`Context`] pairs a session with a document for a caller
/// that holds both for one scope.
///
/// An evaluate needs only `&self`, so a handler may evaluate again on the same
/// session. What may change while one runs is kept in cells; changes that would
/// disturb the walk are refused with [`ContextError::Evaluating`].
pub struct Session {
    node: Cell<Option<Token>>,
    names: RefCell<Names>,

    /* The caps every run under this session starts from. Each evaluate and
     * parse charges a `Budget` of its own made from these. */
    limits: Limits,

    /* Namespace matching for UNPREFIXED name tests. Strict (default) is
     * HTML5-faithful: an unprefixed name resolves in the HTML namespace, so
     * foreign SVG/MathML needs a prefix. Lax matches by local name. */
    lax: bool,

    /* Re-entrancy depth, >0 while an evaluate runs on this session. A nested
     * evaluate just stacks. */
    evaluating: Cell<usize>,
}

impl Session {
    /// A session with `node` (None for none) as the context node, which is an
    /// opaque token the document of each evaluate resolves.
    pub fn new(node: Option<Token>) -> Session {
        Session {
            node: Cell::new(node),
            names: RefCell::new(Names::default()),
            limits: Limits::DEFAULT,
            lax: false,
            evaluating: Cell::new(0),
        }
    }

    /// The caps a run under this session starts from.
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

    /// True while an evaluate is in progress on this session, nested ones
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
        self.node.set(Some(node));
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
        self.names_mut()?.bind_ns(prefix, uri)
    }

    /// Bind the unprefixed variable `$name` to the string `value`, replacing an
    /// earlier binding. Both are copied.
    pub fn register_variable(&self, name: &[u8], value: &[u8]) -> Result<(), ContextError> {
        self.names_mut()?.bind_var(name, value)
    }

    /// Evaluate `ast` over `doc` with the context node as the focus; `handler`
    /// answers the function calls there is no built-in for.
    ///
    /// `doc` is borrowed for this call alone. Each call runs on an evaluation of
    /// its own - its budget, its caches - and only reads the session, so a
    /// handler that evaluates again on this same session cannot disturb the
    /// walk it was called from.
    #[allow(clippy::result_large_err)]
    pub fn evaluate<'d, D: Dom<'d>>(
        &self,
        doc: D,
        ast: &Ast,
        handler: Option<&dyn Resolver>,
    ) -> Result<XPathValue, Error> {
        doc.prepare().map_err(index_error)?;
        let run = self.enter()?;
        eval::eval_ast(self, &run.names, doc, self.focus_node(doc), ast, handler)
    }

    /// [`evaluate`](Self::evaluate) through the `at_xpath` first-match fast
    /// path when the shape allows it, and the full evaluator otherwise.
    #[allow(clippy::result_large_err)]
    pub fn evaluate_first<'d, D: Dom<'d>>(
        &self,
        doc: D,
        ast: &Ast,
        handler: Option<&dyn Resolver>,
    ) -> Result<XPathValue, Error> {
        /* The fast path charges every visited node to a budget of its own, so it
         * is bounded fail-closed exactly like the full evaluator; it only runs
         * for recognised shapes, which call no functions, so it needs no
         * handler. */
        doc.prepare().map_err(index_error)?;
        let matched = {
            let run = self.enter()?;
            eval::try_first_match(self, &run.names, doc, self.focus_node(doc), ast)
        }?;
        match matched {
            Some(value) => Ok(value),
            None => self.evaluate(doc, ast, handler),
        }
    }

    /// Mark an evaluate as running for as long as the guard lives, and lend it
    /// the registrations.
    #[allow(clippy::result_large_err)]
    fn enter(&self) -> Result<Running<'_>, Error> {
        let Ok(names) = self.names.try_borrow() else {
            return Err(Error::with(
                ErrorKind::Internal,
                format_args!("evaluate: the context is being changed"),
            ));
        };
        self.evaluating.set(self.evaluating.get() + 1);
        Ok(Running { cx: self, names })
    }

    /// The context node, resolved through `doc`.
    fn focus_node<'d, D: Dom<'d>>(&self, doc: D) -> Option<D::Node> {
        self.node.get().map(|t| doc.resolve_token(t))
    }
}

/// The backend could not build the per-walk index: out of memory.
fn index_error(status: ErrorKind) -> Error {
    Error::with(
        status,
        format_args!("out of memory building the element index"),
    )
}

/// An evaluate in progress: the borrowed registrations, and the depth count it
/// gives back when it ends.
struct Running<'a> {
    cx: &'a Session,
    names: Ref<'a, Names>,
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.cx.evaluating.set(self.cx.evaluating.get() - 1);
    }
}

/// A [`Session`] paired with the document it evaluates, for a caller that
/// holds both for one scope - the engine's tests, the fuzz harnesses.
///
/// `D` is the backend's document handle (a Lexbor handle for HTML, a borrowed
/// `&xml::Document` for XML), which is why this type is `Context<'d, D>`: a
/// context made from a borrowed document cannot outlive it. A holder that
/// keeps the registrations across calls keeps the [`Session`] instead and
/// lends the document per evaluate. Derefs to the session for everything but
/// the two evaluates.
pub struct Context<'d, D: Dom<'d>> {
    doc: D,
    session: Session,
    /// `D` names the backend but does not syntax-use `'d` when it is a plain
    /// borrow type; this ties the context's lifetime to a document borrow.
    _doc: PhantomData<&'d ()>,
}

impl<'d, D: Dom<'d>> Context<'d, D> {
    /// A context over `doc`, with `node` (None for none) as the context node.
    pub fn new(doc: D, node: Option<Token>) -> Context<'d, D> {
        Context {
            doc,
            session: Session::new(node),
            _doc: PhantomData,
        }
    }

    /// The backend document handle.
    pub fn doc(&self) -> D {
        self.doc
    }

    /// [`Session::evaluate`] over this context's document.
    #[allow(clippy::result_large_err)]
    pub fn evaluate(&self, ast: &Ast, handler: Option<&dyn Resolver>) -> Result<XPathValue, Error> {
        self.session.evaluate(self.doc, ast, handler)
    }

    /// [`Session::evaluate_first`] over this context's document.
    #[allow(clippy::result_large_err)]
    pub fn evaluate_first(
        &self,
        ast: &Ast,
        handler: Option<&dyn Resolver>,
    ) -> Result<XPathValue, Error> {
        self.session.evaluate_first(self.doc, ast, handler)
    }
}

impl<'d, D: Dom<'d>> core::ops::Deref for Context<'d, D> {
    type Target = Session;
    fn deref(&self) -> &Session {
        &self.session
    }
}

impl<'d, D: Dom<'d>> core::ops::DerefMut for Context<'d, D> {
    fn deref_mut(&mut self) -> &mut Session {
        &mut self.session
    }
}

fn copy(bytes: &[u8]) -> Result<Text, ContextError> {
    Text::try_copy(bytes).ok_or(ContextError::Failed)
}

/// The result of an evaluate, owned: dropping it frees the node-set's array or
/// the string.
pub type XPathValue = Val;
