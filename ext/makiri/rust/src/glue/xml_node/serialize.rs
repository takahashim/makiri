//! `#to_xml` / `#to_s` / `#canonicalize` for XML nodes, and the HTML serializers
//! refused rather than answered wrongly.
//!
//! The serialization itself is `crate::xml::serialize`; this module parses the
//! options, turns its bytes into a String (transcoding when `encoding:` asks),
//! and maps a failure to `Makiri::Error`.

#![forbid(unsafe_code)]

use crate::bridge::rb::{prelude::*, Error, Ruby, Value};
use crate::method;

use crate::bridge::args::Kwargs;
use crate::bridge::ruby::makiri_error;

use crate::init::MOD_XML_NODE_METHODS;
use crate::xml::serialize::{self as xml_serialize, Failure};

fn to_xml_opts(ruby: &Ruby, args: &[Value]) -> Result<(i32, Value), Error> {
    let kw = Kwargs::scan(ruby, args)?;
    /* Any other keyword is `ArgumentError`: a misspelt one is not quietly
     * the default. */
    let kw = kw.only(ruby, &["pretty", "indent", "encoding"])?;
    let mut width = 0i32;
    if kw.flag(ruby, "pretty")? {
        width = 2;
    }
    if let Some(iv) = kw.value(ruby, "indent")? {
        width = i32::try_from(crate::bridge::ruby::index_arg(iv)?.max(0)).unwrap_or(i32::MAX);
    }
    let enc = kw
        .value(ruby, "encoding")?
        .unwrap_or(ruby.qnil().as_value());
    Ok((width, enc))
}

/// A serialization failure as `Makiri::Error`, worded for `verb`.
fn failure_error(f: Failure, verb: &str) -> Error {
    let msg = match f {
        Failure::DomLooseName => {
            format_args!("cannot {verb} XML containing a DOM-loose element name")
        }
        Failure::DomLooseDoctype => {
            format_args!(
                "cannot {verb} XML containing a DOM-loose doctype (a name or id XML cannot write)"
            )
        }
        Failure::UnwritableData => format_args!(
            "cannot {verb} XML containing character data XML cannot hold (a character \
outside XML's, -- in a comment, or ?> in a processing instruction)"
        ),
        Failure::ForbiddenDeclaration => format_args!(
            "cannot {verb} XML containing a namespace declaration XML forbids \
(such as xmlns:p=\"\", held as set_attribute_ns made it)"
        ),
        Failure::DomLooseAttributeName => {
            format_args!("cannot {verb} XML containing a DOM-loose attribute name")
        }
        Failure::XmlnsElement => {
            format_args!("cannot {verb} XML containing an element in the XMLNS namespace")
        }
        Failure::PiTargetColon => {
            format_args!(
                "cannot {verb} XML containing a processing-instruction target with a colon"
            )
        }
        Failure::PiTargetReserved => {
            format_args!(
                "cannot {verb} XML containing a processing instruction whose target is xml"
            )
        }
        Failure::OutputCap => {
            format_args!("failed to {verb} XML: output exceeded the size limit")
        }
        Failure::Oom => {
            format_args!("failed to {verb} XML: out of memory")
        }
        Failure::ScopeOverflow => {
            format_args!("failed to {verb} XML: too many namespace bindings in scope")
        }
        Failure::TooDeep => format_args!(
            "failed to {verb} XML: the tree nests deeper than {} levels",
            crate::xml::model::MAX_DEPTH
        ),
        Failure::StackLow => {
            format_args!("failed to {verb} XML: the tree nests too deep for this thread's stack")
        }
        Failure::PrefixSpace => {
            format_args!("failed to {verb} XML: ran out of namespace prefixes to declare")
        }
        Failure::NamespaceBudget => {
            format_args!("failed to {verb} XML: namespace planning exceeded its step budget")
        }
        Failure::UnboundPrefix => format_args!(
            "cannot {verb} XML with a namespace prefix bound to nothing (declare it, \
or insert the node where it is declared)"
        ),
        Failure::NamespaceMismatch => format_args!(
            "cannot {verb} XML where one prefix would have to mean two namespaces on an \
element, or an unprefixed attribute has a namespace (canonical form invents no \
prefix); to_xml writes such a tree"
        ),
    };
    makiri_error(msg)
}

fn to_xml(ruby: &Ruby, this: super::XmlSelf, args: &[Value]) -> Result<Value, Error> {
    let (width, enc_opt) = to_xml_opts(ruby, args)?;
    let (to_enc, enc_name) = if enc_opt.is_nil() {
        (None, None)
    } else {
        /* An unknown name raises, and this frame is about to own the
         * serializer's buffer: the lookup returns the error instead. The
         * name is a copy, so the bytes stay valid across the serializer's
         * call. */
        let (e, bytes) = xml_output_encoding_arg(ruby, enc_opt)?;
        (Some(e), Some(bytes))
    };
    /* Only UTF-8 output is the serializer's own bytes. Anything else -
     * US-ASCII included, which cannot hold a character past ASCII and whose
     * declaration Makiri's own parser holds to ASCII - is transcoded, what
     * it has no form for written as a character reference. */
    let transcode = to_enc.filter(|e| !e.is_utf8_or_ascii() || e.is_usascii());

    let (buf, markup) = {
        let read = this.read()?;
        let buf = xml_serialize::to_xml(
            &read,
            this.id,
            width,
            enc_name.as_ref().map(|b| b.as_slice()),
        )
        .map_err(|f| failure_error(f, "serialize"))?;
        /* Copied out with the read held, tested after it: the converter that
         * tests them may load one, which runs Ruby. */
        let markup = match transcode {
            Some(_) => Some(non_ascii_markup(ruby, &read, this.id)?),
            None => None,
        };
        (buf, markup)
    };
    if let (Some(enc), Some(markup)) = (transcode, markup) {
        refuse_unwritable_markup(ruby, enc, markup)?;
    }

    /* A failed allocation is `Err`, and `buf` (and `enc_name`) drop. */
    let mut str = crate::bridge::string::utf8_str_trusting(ruby, buf.as_slice())?;
    drop(buf);
    if let Some(enc) = transcode {
        str = enc.encode(str, crate::nupur::encoding::Unconvertible::HexCharRef)?;
    }
    Ok(str.as_value())
}

/// The kinds of [`xml_serialize::Markup`], by the index [`non_ascii_markup`]
/// stores.
const MARKUP_KINDS: [xml_serialize::Markup; 5] = [
    xml_serialize::Markup::Name,
    xml_serialize::Markup::Doctype,
    xml_serialize::Markup::Comment,
    xml_serialize::Markup::CData,
    xml_serialize::Markup::Pi,
];

/// The strings `to_xml` writes as markup in `n`'s subtree
/// ([`xml_serialize::markup_strings`]) that are not all ASCII, each with
/// its kind, as a flat Array of kind index and String - Ruby objects, so
/// they are where the GC sees them. ASCII is held by every encoding a
/// document can be written in, so only these need the converter.
fn non_ascii_markup(
    ruby: &Ruby,
    doc: &crate::xml::model::Document,
    n: crate::xml::model::NodeId,
) -> Result<crate::bridge::rb::RArray, Error> {
    use core::ops::ControlFlow;
    let found = crate::bridge::build::ary(ruby, 0)?;
    let mut failed = None;
    let _: ControlFlow<()> = xml_serialize::markup_strings(doc, n, &mut |kind, bytes| {
        if bytes.is_ascii() {
            return ControlFlow::Continue(());
        }
        match push_markup(ruby, found, kind, bytes) {
            Ok(()) => ControlFlow::Continue(()),
            Err(e) => {
                failed = Some(e);
                ControlFlow::Break(())
            }
        }
    });
    match failed {
        Some(e) => Err(e),
        None => Ok(found),
    }
}

/// Append `kind`'s index and `bytes`, a string of the arena, to `found`.
fn push_markup(
    ruby: &Ruby,
    found: crate::bridge::rb::RArray,
    kind: xml_serialize::Markup,
    bytes: &[u8],
) -> Result<(), Error> {
    let index = MARKUP_KINDS.iter().position(|&k| k == kind).unwrap_or(0);
    let index = crate::bridge::build::int(ruby, i64::try_from(index).unwrap_or(0))?;
    /* The arena's strings are UTF-8: the parser and every mutator hold them
     * to it. */
    let s = crate::bridge::string::utf8_str_trusting(ruby, bytes)?;
    crate::bridge::build::push(found, index)?;
    crate::bridge::build::push(found, s)
}

/// `Makiri::Error` when a string of `markup` holds a character `enc` has no
/// form for: a character reference is no substitute there
/// ([`xml_serialize::markup_strings`]), so the output would be ill-formed or
/// read back as other text. Each string goes through the converter the
/// transcode uses, so the two agree on what `enc` can hold.
fn refuse_unwritable_markup(
    ruby: &Ruby,
    enc: crate::bridge::string::Encoding,
    markup: crate::bridge::rb::RArray,
) -> Result<(), Error> {
    for i in (0..markup.len()).step_by(2) {
        let index = match crate::bridge::rb::Integer::from_value(markup.entry(i)) {
            Some(k) => usize::try_from(k.to_i64()?).ok(),
            None => None,
        };
        let kind = index
            .and_then(|k| MARKUP_KINDS.get(k).copied())
            .unwrap_or(xml_serialize::Markup::Name);
        let Some(s) = crate::bridge::rb::RString::from_value(markup.entry(i + 1)) else {
            continue;
        };
        if let Err(e) = enc.encode(s, crate::nupur::encoding::Unconvertible::Raise) {
            /* Not representable: the refusal. Anything else - a converter
             * that failed to load, an interrupt - is passed on as it was. */
            e.rescue(ruby, &[crate::bridge::rb::Builtin::EncodingError])?;
            let name = core::str::from_utf8(enc.name()).unwrap_or("that encoding");
            return Err(makiri_error(format_args!(
                "to_xml cannot write {} in {name}: it holds a character {name} has no form \
                 for, and a character reference is not markup there",
                kind.what()
            )));
        }
    }
    Ok(())
}

fn canonicalize(ruby: &Ruby, this: super::XmlSelf, args: &[Value]) -> Result<Value, Error> {
    let kw = Kwargs::scan(ruby, args)?;
    let kw = kw.only(ruby, &["comments"])?;
    let comments = kw.flag(ruby, "comments")?;
    let buf = xml_serialize::canonicalize(&*this.read()?, this.id, comments)
        .map_err(|f| failure_error(f, "canonicalize"))?;
    Ok(crate::bridge::string::utf8_str_trusting(ruby, buf.as_slice())?.as_value())
}

fn no_serialize(_rb_self: Value, _args: &[Value]) -> Result<Value, Error> {
    Err(Error::new(
        crate::bridge::rb::Builtin::NotImplementedError,
        "Makiri::XML does not HTML-serialize (to_html / inner_html / outer_html); \
         use #to_xml for XML output.",
    ))
}

/// From `Init_makiri`.
pub fn init_xml_node_serialize(ruby: &Ruby) -> Result<(), Error> {
    let m = MOD_XML_NODE_METHODS.defined(ruby)?;
    for name in ["to_xml", "to_s"] {
        m.define_method(name, method!(to_xml, -1))?;
    }
    m.define_method("canonicalize", method!(canonicalize, -1))?;

    for name in ["to_html", "inner_html", "outer_html"] {
        m.define_method(name, method!(no_serialize, -1))?;
    }
    Ok(())
}

/// `to_xml`'s `encoding:` argument: the encoding to write in, and the name
/// to declare for it - the String or Symbol as the caller wrote it, or an
/// `Encoding`'s own name. (An HTML parse's `encoding:` is read otherwise:
/// `glue::html_doc`'s, which also knows the Encoding Standard's labels.)
///
/// Only a name `Makiri::XML` reads back is declared (`bridge::xml::decode`):
/// one of Ruby's names for the process's encodings (`"locale"`) names no
/// charset, so the encoding's own name is written for it, and ASCII-8BIT
/// (`"BINARY"`) is no charset at all, so it is `ArgumentError`.
fn xml_output_encoding_arg(
    ruby: &Ruby,
    v: Value,
) -> Result<
    (
        crate::bridge::string::Encoding,
        crate::ceiling_buf::OwnedBuf,
    ),
    Error,
> {
    let (e, name) = given_output_encoding(ruby, v)?;
    if e.is_ascii8bit() {
        return Err(crate::bridge::ruby::arg_error(
            "to_xml cannot declare ASCII-8BIT (BINARY): it names no charset",
        ));
    }
    if !crate::xml::encoding_names::is_ruby_special(name.as_slice()) {
        return Ok((e, name));
    }
    crate::ceiling_buf::OwnedBuf::copy_from(e.name())
        .map(|b| (e, b))
        .ok_or_else(|| crate::bridge::ruby::makiri_error("out of memory reading an encoding name"))
}

/// The encoding `v` names, and the name as the caller wrote it.
fn given_output_encoding(
    ruby: &Ruby,
    v: Value,
) -> Result<
    (
        crate::bridge::string::Encoding,
        crate::ceiling_buf::OwnedBuf,
    ),
    Error,
> {
    let e = crate::bridge::string::to_encoding(ruby, v)?;
    let name = match crate::bridge::rb::RString::from_value(v) {
        Some(s) => s,
        None if crate::bridge::rb::Symbol::from_value(v).is_some() => {
            crate::bridge::ruby::name_arg(v)?
        }
        None => {
            return crate::ceiling_buf::OwnedBuf::copy_from(e.name())
                .map(|b| (e, b))
                .ok_or_else(|| {
                    crate::bridge::ruby::makiri_error("out of memory reading an encoding name")
                });
        }
    };
    Ok((e, crate::bridge::string::ruby_string_bytes(name)?))
}
