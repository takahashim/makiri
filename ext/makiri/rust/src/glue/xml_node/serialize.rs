//! `#to_xml` / `#to_s` / `#canonicalize` for XML nodes, and the HTML serializers
//! refused rather than answered wrongly.
//!
//! The serialization itself is `crate::xml::serialize`; this module parses the
//! options, turns its bytes into a String (transcoding when `encoding:` asks),
//! and maps a failure to `Makiri::Error`.

#![forbid(unsafe_code)]

use magnus::{method, prelude::*, Error, Ruby, Value};

use crate::bridge::ruby::makiri_error;
use crate::glue::kwargs::Kwargs;

use super::strings::utf8;
use crate::init::MOD_XML_NODE_METHODS;
use crate::xml::serialize::{self as xml_serialize, Failure};

fn to_xml_opts(ruby: &Ruby, args: &[Value]) -> Result<(i32, Value), Error> {
    let kw = Kwargs::scan(args)?;
    let mut width = 0i32;
    if kw.flag(ruby, "pretty") {
        width = 2;
    }
    if let Some(iv) = kw.value(ruby, "indent") {
        width = i32::try_convert(iv)?.max(0);
    }
    let enc = kw.value(ruby, "encoding").unwrap_or(ruby.qnil().as_value());
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
        Failure::TooDeep => format!(
            "failed to {verb} XML: the tree nests deeper than {} levels",
            crate::xml::model::MAX_DEPTH
        ),
        Failure::PrefixSpace => {
            format!("failed to {verb} XML: ran out of namespace prefixes to declare")
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
            let name = crate::bridge::ruby::to_s(enc_opt)?;
            /* A copy, so the bytes stay valid across the serializer's call. */
            let bytes = crate::bridge::string::ruby_string_bytes(name)?;
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
        let comments = Kwargs::scan(args)?.flag(ruby, "comments");
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
pub fn init_xml_node_serialize() -> Result<(), Error> {
    let m = MOD_XML_NODE_METHODS.module();
    for name in ["to_xml", "to_s"] {
        m.define_method(name, method!(to_xml, -1))?;
    }
    m.define_method("canonicalize", method!(canonicalize, -1))?;

    for name in ["to_html", "inner_html", "outer_html"] {
        m.define_method(name, method!(no_serialize, -1))?;
    }
    Ok(())
}
