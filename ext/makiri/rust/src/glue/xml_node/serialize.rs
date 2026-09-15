//! `#to_xml` / `#to_s` / `#canonicalize` for XML nodes, and the HTML serializers
//! refused rather than answered wrongly.
//!
//! The serialization itself is `crate::xml::serialize`; this module parses the
//! options, turns its bytes into a String (transcoding when `encoding:` asks),
//! and maps a failure to `Makiri::Error`.

#![allow(clippy::missing_safety_doc)]

use core::ffi::c_int;

use magnus::rb_sys::{AsRawValue, FromRawValue};
use magnus::{method, prelude::*, Error, RHash, RString, Ruby, Value};
use rb_sys::VALUE;

use super::abi::*;
use crate::xml::serialize::{self as xml_serialize, Failure};

fn to_xml_opts(ruby: &Ruby, args: &[Value]) -> Result<(i32, Value), Error> {
    if args.is_empty() {
        return Ok((0, ruby.qnil().as_value()));
    }
    let scanned = magnus::scan_args::scan_args::<(), (), (), (), RHash, ()>(args)?;
    let h = scanned.keywords;
    let mut width = 0i32;
    if h.get(ruby.to_symbol("pretty"))
        .is_some_and(|v: Value| v.to_bool())
    {
        width = 2;
    }
    if let Some(iv) = h
        .get(ruby.to_symbol("indent"))
        .filter(|v: &Value| !v.is_nil())
    {
        let n = i32::try_convert(iv)?;
        width = n.max(0);
    }
    let enc = h
        .get(ruby.to_symbol("encoding"))
        .unwrap_or(ruby.qnil().as_value());
    Ok((width, enc))
}

/// A serialization failure as `Makiri::Error`, worded for `verb`.
unsafe fn failure_error(f: Failure, verb: &str) -> Error {
    let msg = match f {
        Failure::DomLooseName => format!("cannot {verb} XML containing a DOM-loose element name"),
        Failure::Output => {
            format!("failed to {verb} XML: output exceeded the size limit or out of memory")
        }
    };
    Error::new(error_class(), msg)
}

fn to_xml(ruby: &Ruby, this: super::XmlSelf, args: &[Value]) -> Result<Value, Error> {
    let (width, enc_opt) = to_xml_opts(ruby, args)?;
    unsafe {
        let (to_enc, enc_name) = if enc_opt.is_nil() {
            (core::ptr::null_mut(), None)
        } else {
            let e = rb_sys::rb_to_encoding(enc_opt.as_raw());
            let name: RString = enc_opt.funcall("to_s", ())?;
            (e, Some(name))
        };

        /* The encoding name is borrowed across a call that allocates nothing
         * Ruby-side. */
        let out = xml_serialize::to_xml(
            &*this.doc(),
            this.id,
            width,
            enc_name.as_ref().map(|name| name.as_slice()),
        );
        let buf = out.map_err(|f| failure_error(f, "serialize"))?;
        let mut str = utf8(ruby, buf.as_slice()).as_value();
        drop(buf);

        if !to_enc.is_null()
            && to_enc != rb_sys::rb_utf8_encoding()
            && to_enc != rb_sys::rb_usascii_encoding()
        {
            const UNDEF_HEX_CHARREF: c_int =
                rb_sys::ruby_econv_flag_type::RUBY_ECONV_UNDEF_HEX_CHARREF as c_int;
            str = Value::from_raw(rb_sys::rb_str_encode(
                str.as_raw(),
                rb_sys::rb_enc_from_encoding(to_enc),
                UNDEF_HEX_CHARREF,
                rb_sys::Qnil as VALUE,
            ));
        }
        Ok(str)
    }
}

fn canonicalize(ruby: &Ruby, this: super::XmlSelf, args: &[Value]) -> Result<Value, Error> {
    let comments = if args.is_empty() {
        false
    } else {
        let scanned = magnus::scan_args::scan_args::<(), (), (), (), RHash, ()>(args)?;
        scanned
            .keywords
            .get(ruby.to_symbol("comments"))
            .is_some_and(|v: Value| v.to_bool())
    };
    unsafe {
        let buf = xml_serialize::canonicalize(&*this.doc(), this.id, comments)
            .map_err(|f| failure_error(f, "canonicalize"))?;
        Ok(utf8(ruby, buf.as_slice()).as_value())
    }
}

fn no_serialize(ruby: &Ruby, _rb_self: Value, _args: &[Value]) -> Result<Value, Error> {
    Err(Error::new(
        ruby.exception_not_imp_error(),
        "Makiri::XML does not HTML-serialize (to_html / inner_html / outer_html); \
         use #to_xml for XML output.",
    ))
}

/// # Safety
/// From `Init_makiri`.
pub unsafe extern "C" fn init_xml_node_serialize() {
    let m = magnus::RModule::from_value(Value::from_raw(mkr_mXmlNodeMethods))
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
