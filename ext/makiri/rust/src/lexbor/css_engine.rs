//! What the three users of Lexbor's CSS parser share: owning a Lexbor object
//! until it is handed on, and the parser wired to its arena and selector table.
//! The process-global cell the two long-lived ones keep it in is
//! [`crate::gvl::GvlCell`].
//!
//! The selector matcher (`selectors`), the selector lowering's parser
//! (`css_parser`) and the stylesheet reader (`stylesheet`) each built these by
//! hand, three ways: an RAII owner in one, `if !x.is_null() { destroy }` arms in
//! another, null checks in a `Drop` in the third. The order the pieces are torn
//! down in, and that a half-built set is freed at all, now live here once.

#![allow(unsafe_code)]

use crate::lexbor::abi::consts::STATUS_OK;
use crate::lexbor::abi::{
    lxb_css_memory_clean, lxb_css_memory_create, lxb_css_memory_destroy, lxb_css_memory_init,
    lxb_css_parser_clean, lxb_css_parser_create, lxb_css_parser_destroy, lxb_css_parser_init,
    lxb_css_parser_memory_set_noi, lxb_css_parser_selectors_set_noi, lxb_css_parser_status_noi,
    lxb_css_selector_list_t, lxb_css_selectors_create, lxb_css_selectors_destroy,
    lxb_css_selectors_init, lxb_css_selectors_parse, CssMemory, CssParser, CssSelectors,
};

/// Lexbor's destructor shape: `(object, self_destroy) -> object`.
pub(crate) type Destroy<T> = unsafe extern "C" fn(*mut T, bool) -> *mut T;

/// A Lexbor object this owns, destroyed on drop unless handed on.
///
/// A process-global engine keeps its pieces for the life of the process - that
/// reuse is what makes a query fast - but a half-built one must not leak. Each
/// piece is owned by one of these until the set is whole; [`into_raw`] then
/// hands the pointer to whatever keeps it, and this stops owning it. An object
/// that lives only for one call is simply dropped.
///
/// [`into_raw`]: Owned::into_raw
pub(crate) struct Owned<T>(*mut T, Destroy<T>);

impl<T> Owned<T> {
    /// `None` when Lexbor could not allocate the object.
    pub(crate) fn new(p: *mut T, destroy: Destroy<T>) -> Option<Owned<T>> {
        (!p.is_null()).then_some(Owned(p, destroy))
    }

    #[inline]
    pub(crate) fn as_ptr(&self) -> *mut T {
        self.0
    }

    /// Hand the pointer on; this stops owning it.
    pub(crate) fn into_raw(self) -> *mut T {
        core::mem::ManuallyDrop::new(self).0
    }
}

impl<T> Drop for Owned<T> {
    fn drop(&mut self) {
        // SAFETY: this owns the object - `new` is the only constructor and
        // `into_raw` gives ownership up - so nothing else destroys it.
        unsafe { (self.1)(self.0, true) };
    }
}

/// A selector parser with its arena and selector table, built but not yet
/// handed on. Dropping it destroys all three.
///
/// Fields drop in declaration order, which is the teardown order: the parser
/// and the table point into the arena, so the arena goes last.
pub(crate) struct ParserParts {
    parser: Owned<CssParser>,
    table: Owned<CssSelectors>,
    mem: Owned<CssMemory>,
}

impl ParserParts {
    /// Create, initialise and wire the three. `None` if any step failed, with
    /// whatever was built already destroyed.
    pub(crate) fn build() -> Option<ParserParts> {
        // SAFETY: Lexbor's constructors take no Rust memory; each result is
        // owned by an `Owned` before anything else runs.
        let parts = unsafe {
            ParserParts {
                parser: Owned::new(lxb_css_parser_create(), lxb_css_parser_destroy)?,
                table: Owned::new(lxb_css_selectors_create(), lxb_css_selectors_destroy)?,
                mem: Owned::new(lxb_css_memory_create(), lxb_css_memory_destroy)?,
            }
        };
        // SAFETY: all three are live and owned by `parts`, and the setters
        // only store the pointers they are given.
        unsafe {
            if lxb_css_memory_init(parts.mem.as_ptr(), 128) != STATUS_OK
                || lxb_css_parser_init(parts.parser.as_ptr(), core::ptr::null_mut()) != STATUS_OK
                || lxb_css_selectors_init(parts.table.as_ptr()) != STATUS_OK
            {
                return None;
            }
            lxb_css_parser_memory_set_noi(parts.parser.as_ptr(), parts.mem.as_ptr());
            lxb_css_parser_selectors_set_noi(parts.parser.as_ptr(), parts.table.as_ptr());
        }
        Some(parts)
    }

    /// A view of the three for as long as `self` lives.
    ///
    /// [`into_parser`] is the way to a `SelectorParser` in production, and it
    /// gives ownership up for good - the process-global engine is never
    /// destroyed. A test that used it would leak the whole object graph, which
    /// is what LeakSanitizer reports. This borrows instead, so `self`'s `Drop`
    /// still frees all three.
    ///
    /// [`into_parser`]: ParserParts::into_parser
    #[cfg(test)]
    pub(crate) fn as_parser(&self) -> SelectorParser {
        SelectorParser {
            parser: self.parser.as_ptr(),
            table: self.table.as_ptr(),
            mem: self.mem.as_ptr(),
        }
    }

    /// Hand the three on, for the life of the process.
    pub(crate) fn into_parser(self) -> SelectorParser {
        let ParserParts { parser, table, mem } = self;
        SelectorParser {
            parser: parser.into_raw(),
            table: table.into_raw(),
            mem: mem.into_raw(),
        }
    }
}

/// A process-lifetime selector parser: its pieces are never destroyed.
///
/// It lives in a [`crate::gvl::GvlCell`], and every method's contract is that
/// the borrow of that cell it came out of is still live - which is what
/// serialises the calls, and what keeps two users off one arena.
///
/// `Copy`, and handed out BY VALUE: a caller can then hold the parser while it
/// also touches the rest of the global it lives in, which a reference into
/// that global would make a second live borrow.
#[derive(Clone, Copy)]
pub(crate) struct SelectorParser {
    parser: *mut CssParser,
    /// Handed to the parser at build time and never read again, but owned
    /// here: the parser lives for the process, and this says so.
    #[allow(dead_code)]
    table: *mut CssSelectors,
    mem: *mut CssMemory,
}

impl SelectorParser {
    /// Parse `selector` into the arena: the list, or `None` for a selector
    /// Lexbor rejects.
    ///
    /// Both conditions matter: Lexbor can hand back a list AND a non-OK status
    /// for a partially-recovered parse, and a recovered selector is not the one
    /// the caller wrote. Either way the parser is left for the caller to clean.
    ///
    /// # Safety
    /// Its cell's borrow is live.
    pub(crate) unsafe fn parse(self, selector: &[u8]) -> Option<*mut lxb_css_selector_list_t> {
        /* `contains_guard` decides what reaches the parser; `Err` is OOM, and
         * the original bytes are never a fallback. */
        let guarded = match crate::lexbor::contains_guard::neutralized(selector) {
            Ok(g) => g,
            Err(_) => return None,
        };
        let bytes = guarded.as_deref().unwrap_or(selector);

        // SAFETY: a live parser, used under its cell's borrow; `bytes` is a live slice
        // the parser only reads. The pointer is the slice's own even when it
        // is empty, which the parser may look at.
        unsafe {
            let list = lxb_css_selectors_parse(self.parser, bytes.as_ptr(), bytes.len());
            (!list.is_null() && lxb_css_parser_status_noi(self.parser) == STATUS_OK).then_some(list)
        }
    }

    /// Empty the arena, freeing every list parsed into it.
    ///
    /// # Safety
    /// Its cell's borrow is live, and no list parsed into the arena is used
    /// afterwards.
    pub(crate) unsafe fn clean_arena(self) {
        // SAFETY: a live arena, used under its cell's borrow; the caller keeps no list.
        unsafe { lxb_css_memory_clean(self.mem) };
    }

    /// Return the parser to its CLEAN stage, keeping what the arena holds.
    ///
    /// # Safety
    /// Its cell's borrow is live.
    pub(crate) unsafe fn clean_parser(self) {
        // SAFETY: a live parser, used under its cell's borrow.
        unsafe { lxb_css_parser_clean(self.parser) };
    }

    /// Empty the arena - every list parsed into it is gone - and return the
    /// parser to its CLEAN stage.
    ///
    /// # Safety
    /// Its cell's borrow is live, and no list parsed into the arena is used
    /// afterwards.
    pub(crate) unsafe fn clean_all(self) {
        // SAFETY: live objects, used under its cell's borrow; the caller keeps no list.
        unsafe {
            lxb_css_memory_clean(self.mem);
            lxb_css_parser_clean(self.parser);
        }
    }
}

/// A `lexbor_str_t` as a slice, or `None` when its data pointer is NULL.
///
/// The NULL-vs-empty distinction is load-bearing: for a namespace, NULL means
/// "no pipe was written" and empty means "an explicit no-namespace".
///
/// # Safety
/// `s` must be a string Lexbor built in a parse's arena.
pub(crate) unsafe fn lexbor_str(s: &crate::lexbor::abi::lexbor_str_t) -> Option<&[u8]> {
    if s.data.is_null() {
        None
    } else {
        // SAFETY: Lexbor keeps `length` bytes at `data`, in the same arena.
        Some(unsafe { core::slice::from_raw_parts(s.data, s.length) })
    }
}
