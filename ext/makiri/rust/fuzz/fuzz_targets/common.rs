//! Shared declarations for the three targets.
//!
//! These are the C ABI entry points, not the Rust modules behind them, and that
//! is deliberate. A target that called `makiri_rs::xml::parse` directly would
//! never exercise the contract the extension actually relies on: raw pointer +
//! length, a NUL at `ptr[len]`, no interior NUL, an out-parameter status, an
//! opaque handle whose ownership crosses the boundary. Those rules are where an
//! FFI bug lives, so the fuzzer goes through them.

#![allow(dead_code)]

use core::ffi::{c_char, c_int, c_void};

pub use makiri::xml::abi::Doc;
pub use makiri::xpath_abi::{Error as XPathError, Limits, Node as Ast, VerifiedText, XPathValue};

extern "C" {
    pub fn mkr_xml_parse(src: *const c_char, len: usize, status: *mut i32) -> *mut Doc;
    pub fn mkr_xml_doc_destroy(doc: *mut Doc);
    pub fn mkr_xml_validate_chars(src: *const c_char, len: u32) -> i32;
    pub fn mkr_utf8_valid(src: *const u8, len: usize) -> bool;

    pub fn mkr_xpath_context_new(doc: *mut c_void, node: *mut c_void) -> *mut c_void;
    pub fn mkr_xpath_context_free(ctx: *mut c_void);
    pub fn mkr_xpath_set_engine_kind(ctx: *mut c_void, kind: c_int);
    pub fn mkr_ctx_limits(ctx: *mut c_void) -> *mut Limits;
    pub fn mkr_xpath_register_ns(
        ctx: *mut c_void,
        prefix: VerifiedText,
        uri: VerifiedText,
    ) -> c_int;

    pub fn mkr_xpath_limits_init_defaults(limits: *mut Limits);
    pub fn mkr_parse(expr: VerifiedText, limits: *mut Limits, err: *mut XPathError) -> *mut Ast;
    pub fn mkr_node_free(ast: *mut Ast);
    pub fn mkr_xpath_eval_compiled(
        ctx: *mut c_void,
        ast: *mut Ast,
        out: *mut XPathValue,
        err: *mut XPathError,
    ) -> c_int;
    pub fn mkr_xpath_value_clear(v: *mut XPathValue);
    pub fn mkr_xpath_error_clear(e: *mut XPathError);
}

/// The engine kind the XML monomorphization answers to. Every target pins this;
/// the HTML entries are abort() stubs (fuzz/csupport/stub.c), so getting it
/// wrong crashes rather than silently fuzzing nothing.
pub const ENGINE_XML: c_int = 1;

/// An expression the engine can read, owned for the call.
///
/// The engine's text contract wants a NUL at `ptr[len]` and no interior NUL:
/// the lexer's `strtod` and its `"%.10s"` error path rely on the terminator.
/// libFuzzer hands over exactly `size` bytes with no terminator, so - exactly
/// as the retired C harnesses did with `mkr_strndup` - truncate at the first
/// interior NUL and copy into an owned, terminated buffer.
///
/// UTF-8 validity is deliberately NOT pre-checked here: the lexer's strict
/// decoder rejecting invalid UTF-8 is itself a path worth reaching.
pub struct Expr {
    buf: Vec<u8>,
    len: usize,
}

impl Expr {
    pub fn new(bytes: &[u8]) -> Option<Expr> {
        let len = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        let mut buf = Vec::new();
        // The crate's clippy.toml routes allocations through `falloc` so the
        // OOM sweep can fail them. The HARNESS must not be on that path: an
        // injected failure has to land in the code under test, not in the
        // scaffolding that feeds it. std's fallible reserve is the right call
        // here, and saying so is the point of the allow.
        #[allow(clippy::disallowed_methods)]
        buf.try_reserve_exact(len + 1).ok()?;
        buf.extend_from_slice(&bytes[..len]);
        buf.push(0);
        Some(Expr { buf, len })
    }

    pub fn text(&self) -> VerifiedText {
        VerifiedText {
            ptr: self.buf.as_ptr() as *const c_char,
            len: self.len,
        }
    }
}
