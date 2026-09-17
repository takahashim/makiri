//! `Makiri::XPathContext` and `Node#xpath` / `#at_xpath` (glue/ruby_xpath.c).
//!
//!   `XPathContext.new(node, namespace_matching:)`, `#evaluate(expr, handler = nil)`,
//!   `#register_namespace`, `#register_variable`, `#node=`
//!   `Node#xpath(expr, handler = nil, namespace_matching:)` / `#at_xpath(...)`
//!
//! # The wrapper and the handler bridge live in the bridge
//!
//! The `XPathContext` TypedData, its AST cache, the Ruby custom-function handler
//! bridge (`rb_protect`, raw `VALUE`, `extern "C"`) and the query path all touch
//! raw Ruby ABI, so they live in [`crate::bridge::xpath`] beside the other
//! seams. This module only re-exports the entries `Init_makiri` and the XML
//! query glue name here, so it holds no unsafe of its own.

#![forbid(unsafe_code)]

pub use crate::bridge::xpath::{
    evaluate_query, init_xpath, query_result, ruby_exception_message, ruby_try_verified_text,
    xpath_error,
};
pub use crate::init::{CLASS_XPATH_CONTEXT, EXC_XPATH_LIMIT_EXCEEDED, EXC_XPATH_SYNTAX_ERROR};
