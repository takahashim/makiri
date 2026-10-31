//! The exceptions an XML edit's or parse's status raises, worded once.

#![forbid(unsafe_code)]

use super::*;

/// A mutation's or translation's `Result` with its failure as the Ruby
/// exception [`xml_mut_error`] maps the status to.
pub fn xml_mut_result<T>(r: Result<T, MutError>) -> Result<T, Error> {
    r.map_err(xml_mut_error)
}

/// The exception for a failed mutation's status.
pub(super) fn xml_mut_error(st: MutError) -> Error {
    let msg: &str = match st {
        MutError::Oom => "out of memory mutating XML",
        MutError::BadName => {
            return crate::bridge::ruby::arg_error("not a well-formed XML name");
        }
        MutError::BadDomName(why) => return crate::bridge::ruby::arg_error(why),
        MutError::InvalidCharacter(why) => return crate::bridge::ruby::arg_error(why),
        MutError::UnboundNs => "namespace prefix is not bound in this scope",
        MutError::Type => "operation unsupported for this node type",
        MutError::PreInsert(e) => return crate::bridge::dom_error::pre_insert_error(e),
        MutError::BadNsDecl(why) => match why {
            NsDeclError::Xmlns => "namespace declaration not permitted: xmlns cannot be declared",
            NsDeclError::XmlElsewhere => {
                "namespace declaration not permitted: xml can only be bound to \
http://www.w3.org/XML/1998/namespace"
            }
            NsDeclError::ReservedUri { default: false } => {
                "namespace declaration not permitted: the XML and XMLNS namespaces cannot be \
bound to another prefix"
            }
            NsDeclError::ReservedUri { default: true } => {
                "namespace declaration not permitted: the XML and XMLNS namespaces cannot be \
the default namespace"
            }
            NsDeclError::PrefixToEmpty => {
                "namespace declaration not permitted: a prefix cannot be bound to the empty \
namespace (only xmlns=\"\" undeclares, and only the default)"
            }
        },
        MutError::DuplicateAttr => {
            "the element already has an attribute with that namespace and local name"
        }
        MutError::BadNsName => {
            "the namespace does not fit the qualified name (a prefix needs a namespace; \
xml and xmlns take only their own)"
        }
        MutError::Internal => "internal error mutating XML (no document)",
        /* The document's own budget, not the machine's memory - so the same
         * exception a parse raises for the same cause. */
        MutError::Limit => {
            return Error::new(
                &EXC_XML_LIMIT_EXCEEDED,
                "XML document exceeded its byte or node budget",
            )
        }
    };
    makiri_error(msg)
}

/* ------------------------------------------------------------------ */
/* documents: parsing, readers, fragments                             *
 * ------------------------------------------------------------------ */

/// A `Makiri::XML::SyntaxError`-family error for a parse status.
pub(super) fn parse_status_error(status: ParseError, unit: Unit) -> Error {
    match status {
        ParseError::Syntax => Error::new(&EXC_XML_SYNTAX_ERROR, unit.malformed()),
        ParseError::Limit => Error::new(&EXC_XML_LIMIT_EXCEEDED, unit.budget()),
        ParseError::Unsupported => Error::new(
            &EXC_XML_SYNTAX_ERROR,
            "unsupported DTD construct: Makiri does not apply attribute defaults or \
             non-CDATA attribute types, expand parameter entities, or expand entities \
             a DTD declares",
        ),
        ParseError::Oom => makiri_error(unit.failed()),
        /* The parser's own invariant (`ExpandErr::Overflow`: an expansion
         * outgrew its input), never the input's fault. */
        ParseError::Internal => crate::bridge::ruby::internal_error(unit.failed()),
    }
}

/// Which entry point failed. The two carry their own wording rather than one
/// composed string: the document path says "malformed XML" where the fragment
/// path says "malformed XML fragment", and the messages are observable.
#[derive(Clone, Copy)]
pub(super) enum Unit {
    Document,
    Fragment,
}

impl Unit {
    fn malformed(self) -> &'static str {
        match self {
            Unit::Document => "malformed XML",
            Unit::Fragment => "malformed XML fragment",
        }
    }
    fn budget(self) -> &'static str {
        match self {
            Unit::Document => "XML document budget exceeded",
            Unit::Fragment => "XML fragment budget exceeded",
        }
    }
    fn failed(self) -> &'static str {
        match self {
            Unit::Document => "failed to parse XML document",
            Unit::Fragment => "failed to parse XML fragment",
        }
    }
}
