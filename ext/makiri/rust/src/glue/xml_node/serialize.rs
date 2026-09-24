//! `#to_xml` / `#to_s` / `#canonicalize` for XML nodes, and the HTML serializers
//! refused rather than answered wrongly.
//!
//! The serialization itself is `crate::xml::serialize`; this module parses the
//! options, turns its bytes into a String (transcoding when `encoding:` asks),
//! and maps a failure to `Makiri::Error`.

#![forbid(unsafe_code)]

use magnus::{method, prelude::*, Error, RHash, RString, Ruby, Value};

use crate::bridge::ruby::makiri_error;

use super::strings::utf8;
use crate::init::MOD_XML_NODE_METHODS;
use crate::xml::serialize::{self as xml_serialize, Failure};

fn to_xml_opts(ruby: &Ruby, args: &[Value]) -> Result<(i32, Value), Error> {
    if args.is_empty() {
        return Ok((0, ruby.qnil().as_value()));
    }
    let scanned = magnus::scan_args::scan_args::<(), (), (), (), RHash, ()>(args)?;
    let h = scanned.keywords;
    let mut width = 0i32;
    if h.get(ruby.sym_new("pretty"))
        .is_some_and(|v: Value| v.to_bool())
    {
        width = 2;
    }
    if let Some(iv) = h
        .get(ruby.sym_new("indent"))
        .filter(|v: &Value| !v.is_nil())
    {
        let n = i32::try_convert(iv)?;
        width = n.max(0);
    }
    let enc = h
        .get(ruby.sym_new("encoding"))
        .unwrap_or(ruby.qnil().as_value());
    Ok((width, enc))
}

/// A serialization failure as `Makiri::Error`, worded for `verb`.
fn failure_error(f: Failure, verb: &str) -> Error {
    let msg = match f {
        Failure::DomLooseName => format!("cannot {verb} XML containing a DOM-loose element name"),
        Failure::PiTargetColon => {
            format!("cannot {verb} XML containing a processing-instruction target with a colon")
        }
        Failure::Output => {
            format!("failed to {verb} XML: output exceeded the size limit or out of memory")
        }
        Failure::NamespaceBudget => {
            format!("failed to {verb} XML: namespace planning exceeded its step budget")
        }
        Failure::UnboundPrefix => format!(
            "cannot {verb} XML with a namespace prefix bound to nothing (declare it, \
or insert the node where it is declared)"
        ),
        Failure::NamespaceMismatch => format!(
            "cannot {verb} XML whose namespace declarations no longer match its names \
(a node moved from under its declaration, or one removed); to_xml writes the \
declarations it needs"
        ),
    };
    makiri_error(msg)
}

fn to_xml(ruby: &Ruby, this: super::XmlSelf, args: &[Value]) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let (width, enc_opt) = to_xml_opts(ruby, args)?;
        let (to_enc, enc_name) = if enc_opt.is_nil() {
            (None, None)
        } else {
            /* An unknown name raises, and this frame is about to own the
             * serializer's buffer: the lookup returns the error instead. */
            let e = crate::bridge::string::to_encoding(enc_opt)?;
            let name: RString = enc_opt.funcall("to_s", ())?;
            /* A copy, so the bytes stay valid across the serializer's call. */
            let bytes = crate::bridge::string::ruby_string_bytes(name.as_value())?;
            (Some(e), Some(bytes))
        };

        let out = xml_serialize::to_xml(
            this.doc_ref(),
            this.id,
            width,
            enc_name.as_ref().map(|b| b.as_slice()),
        );
        let buf = out.map_err(|f| failure_error(f, "serialize"))?;
        let mut str = utf8(ruby, buf.as_slice()).as_value();
        drop(buf);

        if let Some(enc) = to_enc {
            if enc.needs_transcode() {
                str = enc.encode_charref(str)?;
            }
        }
        Ok(str)
    })
}

fn canonicalize(ruby: &Ruby, this: super::XmlSelf, args: &[Value]) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let comments = if args.is_empty() {
            false
        } else {
            let scanned = magnus::scan_args::scan_args::<(), (), (), (), RHash, ()>(args)?;
            scanned
                .keywords
                .get(ruby.sym_new("comments"))
                .is_some_and(|v: Value| v.to_bool())
        };
        let buf = xml_serialize::canonicalize(this.doc_ref(), this.id, comments)
            .map_err(|f| failure_error(f, "canonicalize"))?;
        Ok(utf8(ruby, buf.as_slice()).as_value())
    })
}

fn no_serialize(ruby: &Ruby, _rb_self: Value, _args: &[Value]) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        Err(Error::new(
            ruby.exception_not_imp_error(),
            "Makiri::XML does not HTML-serialize (to_html / inner_html / outer_html); \
         use #to_xml for XML output.",
        ))
    })
}

/// From `Init_makiri`.
pub fn init_xml_node_serialize() {
    let m = magnus::RModule::from_value(MOD_XML_NODE_METHODS.value())
        .expect("Makiri::XML::NodeMethods");
    for name in ["to_xml", "to_s"] {
        m.define_method(name, method!(to_xml, -1)).expect("#to_xml");
    }
    m.define_method("canonicalize", method!(canonicalize, -1))
        .expect("#canonicalize");

    for name in ["to_html", "inner_html", "outer_html"] {
        m.define_method(name, method!(no_serialize, -1))
            .expect("#to_html");
    }
}
