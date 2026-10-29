//! The Ruby side of the SAX parser (`Makiri::XML::SAX::Parser`): a streaming
//! parse (`xml::tree::parse_streaming`) whose reports call the handler's
//! methods as each is read, with the shape Nokogiri's SAX gives them.
//!
//! This runs the caller's Ruby in the middle of a parse, which the rest of
//! the extension never does, and is sound for these reasons
//! (`notes/xml_sax_plan.ja.md`):
//!
//! - every call is a protected one (`raw::call_method`): a raise, a `throw`,
//!   a `break` comes back as `Err`, which stops the parse with an ordinary
//!   return before the trampoline raises it again - nothing longjmps over
//!   the parser's frames;
//! - the parse holds no Document a Ruby object can reach - its arena is a
//!   scratch one, given back node by node - so the handler has nothing of
//!   Makiri's to edit or read while a borrow is held, only the Strings and
//!   Arrays made for it;
//! - the GVL is held throughout, as for any Ruby call;
//! - the handler and the Attribute class are the method's arguments, which
//!   Ruby keeps, and every value made for a call is that call's argument.
//!
//! A handler that switches Fibers and never comes back leaves this frame's
//! parse state (the decoded input, the scratch arena) unreleased - memory,
//! not safety.

#![forbid(unsafe_code)]

use core::ffi::CStr;

use crate::bridge::rb::{Error, Ruby, Value};
use crate::xml::tree::events::{Events, Flow, XmlDecl};
use crate::xml::{Document, NodeId, ParseError};

/// The handler, and what stopped the parse when a call failed.
struct Handler<'r> {
    ruby: &'r Ruby,
    handler: Value,
    /// `Makiri::XML::SAX::Parser::Attribute`, a Struct of Makiri's own.
    attribute: Value,
    started: bool,
    failed: Option<Error>,
}

impl Handler<'_> {
    /// `handler.name(*args)`.
    #[cfg_attr(
        dylint_lib = "makiri_lints",
        allow(
            runs_callers_ruby,
            reason = "a SAX handler's method: calling it is what the parser is for, and the \
                      parse holds nothing Ruby can reach (module doc)"
        )
    )]
    fn call(&self, name: &CStr, args: &[Value]) -> Result<(), Error> {
        crate::bridge::raw::call_method(self.ruby, self.handler, name, args).map(|_| ())
    }

    /// `Attribute.new(localname, prefix, uri, value)`.
    #[cfg_attr(
        dylint_lib = "makiri_lints",
        allow(
            runs_callers_ruby,
            reason = "Makiri's own Struct, made with no borrow held (module doc)"
        )
    )]
    fn attribute(&self, args: &[Value]) -> Result<Value, Error> {
        crate::bridge::raw::call_method(self.ruby, self.attribute, c"new", args)
    }

    fn str(&self, b: &[u8]) -> Result<Value, Error> {
        crate::bridge::build::utf8_str(self.ruby, b)
    }

    /// A String, or `nil` for nothing written (an absent prefix or URI).
    fn str_or_nil(&self, b: &[u8]) -> Result<Value, Error> {
        if b.is_empty() {
            Ok(self.ruby.qnil())
        } else {
            self.str(b)
        }
    }

    /// `start_document`, once, before anything else but the declaration.
    fn start(&mut self) -> Result<(), Error> {
        if !self.started {
            self.started = true;
            self.call(c"start_document", &[])?;
        }
        Ok(())
    }

    /// A report's outcome: an `Err` is kept, and stops the parse.
    fn flow(&mut self, r: Result<(), Error>) -> Flow {
        match r {
            Ok(()) => Flow::Continue(()),
            Err(e) => {
                self.failed = Some(e);
                Flow::Break(())
            }
        }
    }

    fn decl(&mut self, d: &XmlDecl<'_>) -> Result<(), Error> {
        let version = self.str(d.version)?;
        let encoding = self.str_or_nil(d.encoding.unwrap_or_default())?;
        let standalone = self.str_or_nil(d.standalone.unwrap_or_default())?;
        self.call(c"xmldecl", &[version, encoding, standalone])?;
        self.start()
    }

    /// `start_element_namespace(name, attrs, prefix, uri, ns)`: the local
    /// name, the attributes as `Attribute`s, the element's prefix and URI
    /// (`nil` when absent), and its namespace declarations as
    /// `[prefix-or-nil, uri]` pairs.
    fn start_element(&mut self, doc: &Document, el: NodeId) -> Result<(), Error> {
        self.start()?;
        let attrs = crate::bridge::build::ary(self.ruby, 0)?;
        let ns = crate::bridge::build::ary(self.ruby, 0)?;
        for a in doc.attributes(el) {
            if let Some(p) = doc.decl_prefix(a) {
                let pair = crate::bridge::build::ary(self.ruby, 2)?;
                crate::bridge::build::push(pair, self.str_or_nil(p)?)?;
                crate::bridge::build::push(pair, self.str(doc.value(a))?)?;
                crate::bridge::build::push(ns, crate::bridge::rb::ReprValue::as_value(pair))?;
            } else {
                let made = self.attribute(&[
                    self.str(doc.local(a))?,
                    self.str_or_nil(doc.prefix(a))?,
                    self.str_or_nil(doc.ns(a))?,
                    self.str(doc.value(a))?,
                ])?;
                crate::bridge::build::push(attrs, made)?;
            }
        }
        let args = [
            self.str(doc.local(el))?,
            crate::bridge::rb::ReprValue::as_value(attrs),
            self.str_or_nil(doc.prefix(el))?,
            self.str_or_nil(doc.ns(el))?,
            crate::bridge::rb::ReprValue::as_value(ns),
        ];
        self.call(c"start_element_namespace", &args)
    }

    fn end_element(&mut self, doc: &Document, el: NodeId) -> Result<(), Error> {
        let args = [
            self.str(doc.local(el))?,
            self.str_or_nil(doc.prefix(el))?,
            self.str_or_nil(doc.ns(el))?,
        ];
        self.call(c"end_element_namespace", &args)
    }

    fn one(&mut self, name: &CStr, b: &[u8]) -> Result<(), Error> {
        self.start()?;
        let s = self.str(b)?;
        self.call(name, &[s])
    }

    fn pi(&mut self, target: &[u8], data: &[u8]) -> Result<(), Error> {
        self.start()?;
        let args = [self.str(target)?, self.str(data)?];
        self.call(c"processing_instruction", &args)
    }
}

impl Events for Handler<'_> {
    const STREAMING: bool = true;

    fn xml_decl(&mut self, d: &XmlDecl<'_>) -> Flow {
        let r = self.decl(d);
        self.flow(r)
    }
    fn start_element(&mut self, doc: &Document, el: NodeId) -> Flow {
        let r = Handler::start_element(self, doc, el);
        self.flow(r)
    }
    fn end_element(&mut self, doc: &Document, el: NodeId) -> Flow {
        let r = Handler::end_element(self, doc, el);
        self.flow(r)
    }
    fn text(&mut self, b: &[u8]) -> Flow {
        let r = self.one(c"characters", b);
        self.flow(r)
    }
    fn cdata(&mut self, b: &[u8]) -> Flow {
        let r = self.one(c"cdata_block", b);
        self.flow(r)
    }
    fn comment(&mut self, b: &[u8]) -> Flow {
        let r = self.one(c"comment", b);
        self.flow(r)
    }
    fn pi(&mut self, target: &[u8], data: &[u8]) -> Flow {
        let r = Handler::pi(self, target, data);
        self.flow(r)
    }
}

/// Parse `source`, a String, calling `handler`'s SAX methods as it goes.
///
/// As Nokogiri's: an input the parse refuses is reported to `error` with the
/// words a document parse raises with, then `end_document`, and the call
/// returns; what the handler raises or throws leaves at once. The input is
/// decoded as `Makiri::XML` decodes it - an undecodable one raises, as there.
pub fn parse(ruby: &Ruby, handler: Value, source: Value, attribute: Value) -> Result<(), Error> {
    let source = crate::bridge::ruby::string_arg(source)?;
    let src =
        crate::bridge::xml_decode::xml_decode_input(ruby, source, Some(crate::xml::MAX_BYTES))?;
    let mut h = Handler {
        ruby,
        handler,
        attribute,
        started: false,
        failed: None,
    };
    match crate::xml::tree::parse_streaming(src.as_slice(), None, &mut h) {
        Ok(()) => {}
        Err(ParseError::Stopped) => {
            return Err(h.failed.take().unwrap_or_else(|| {
                crate::bridge::ruby::internal_error("a SAX parse stopped with no reason")
            }))
        }
        Err(status) => match crate::bridge::xml::document_refusal(status) {
            Some(words) => {
                h.start()?;
                let s = h.str(words.as_bytes())?;
                h.call(c"error", &[s])?;
            }
            None if status == ParseError::Oom => {
                return Err(crate::bridge::ruby::makiri_error(
                    "out of memory parsing XML",
                ))
            }
            None => {
                return Err(crate::bridge::ruby::internal_error(
                    "failed to parse XML document",
                ))
            }
        },
    }
    drop(src);
    h.start()?;
    h.call(c"end_document", &[])
}
