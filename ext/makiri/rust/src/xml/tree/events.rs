//! What a parse reports as it goes: the receiving side of a streaming (SAX)
//! parse, and the nothing a tree-building parse reports.
//!
//! The parser is one: a streaming parse runs the same scanner and the same
//! checks - well-formedness, namespaces, duplicate attributes, references,
//! the DTD's limits - as the parse that builds a tree, so the two accept the
//! same input. What differs is decided by [`Events::STREAMING`], a constant,
//! so the tree-building parse ([`Build`]) carries no branch for it:
//!
//! - a tree-building parse links every node into the tree and reports
//!   nothing;
//! - a streaming parse links nothing, reports each node as it is read, and
//!   gives the arena back once it is reported - an element when it closes,
//!   anything else at once - so its memory follows the open elements, not
//!   the document.
//!
//! A report that answers [`Flow::Break`] stops the parse, which then returns
//! `ParseError::Stopped`; why it stopped is the receiver's to keep.

#![forbid(unsafe_code)]

use crate::xml::{Document, NodeId};

/// Whether a parse goes on after a report.
pub type Flow = core::ops::ControlFlow<()>;

/// An XML declaration's pseudo-attributes, as written.
pub struct XmlDecl<'a> {
    pub version: &'a [u8],
    pub encoding: Option<&'a [u8]>,
    pub standalone: Option<&'a [u8]>,
}

/// The receiving side of a parse. Each report runs once the parser has
/// checked what it reports; the bytes are the arena's, valid for the call.
pub trait Events {
    /// Whether the parse streams (see the module doc).
    const STREAMING: bool;

    /// The XML declaration.
    fn xml_decl(&mut self, _decl: &XmlDecl<'_>) -> Flow {
        Flow::Continue(())
    }

    /// An element's start tag, read whole: its name and namespace resolved
    /// and its attributes (`doc.attributes(el)`, declarations included)
    /// checked. A self-closing element's end follows at once.
    fn start_element(&mut self, _doc: &Document, _el: NodeId) -> Flow {
        Flow::Continue(())
    }

    /// An element's end, `el` as its start reported it.
    fn end_element(&mut self, _doc: &Document, _el: NodeId) -> Flow {
        Flow::Continue(())
    }

    /// Character data between two tags, its references expanded.
    /// Whitespace outside the root element is not reported.
    fn text(&mut self, _bytes: &[u8]) -> Flow {
        Flow::Continue(())
    }

    /// A CDATA section's content.
    fn cdata(&mut self, _bytes: &[u8]) -> Flow {
        Flow::Continue(())
    }

    /// A comment's content.
    fn comment(&mut self, _bytes: &[u8]) -> Flow {
        Flow::Continue(())
    }

    /// A processing instruction: its target and its data.
    fn pi(&mut self, _target: &[u8], _data: &[u8]) -> Flow {
        Flow::Continue(())
    }
}

/// The tree-building parse: it reports nothing.
pub struct Build;

impl Events for Build {
    const STREAMING: bool = false;
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_methods)] /* test inputs */
    use super::*;
    use crate::xml::model::ArenaKind;
    use crate::xml::tree::{parse, parse_streaming};
    use crate::xml::ParseError;

    /// The reports of a streaming parse, as text, and the highest arena index
    /// an element reached.
    #[derive(Default)]
    struct Record {
        out: Vec<String>,
        top_index: u32,
        stop_at: Option<&'static str>,
    }

    fn s(b: &[u8]) -> String {
        String::from_utf8_lossy(b).into_owned()
    }

    impl Events for Record {
        const STREAMING: bool = true;
        fn xml_decl(&mut self, d: &XmlDecl<'_>) -> Flow {
            let o = |v: Option<&[u8]>| v.map_or("-".to_owned(), s);
            self.out.push(format!(
                "decl {} {} {}",
                s(d.version),
                o(d.encoding),
                o(d.standalone)
            ));
            Flow::Continue(())
        }
        fn start_element(&mut self, doc: &Document, el: NodeId) -> Flow {
            self.top_index = self.top_index.max(el.index());
            let attrs: Vec<String> = doc
                .attributes(el)
                .map(|a| format!("{}={}", s(doc.qname(a)), s(doc.value(a))))
                .collect();
            let uri = s(doc.span(doc.node(el).ns_uri));
            self.out.push(format!(
                "<{} {{{uri}}} [{}]",
                s(doc.qname(el)),
                attrs.join(" ")
            ));
            if self.stop_at.is_some_and(|n| n.as_bytes() == doc.qname(el)) {
                return Flow::Break(());
            }
            Flow::Continue(())
        }
        fn end_element(&mut self, doc: &Document, el: NodeId) -> Flow {
            self.out.push(format!(">{}", s(doc.qname(el))));
            Flow::Continue(())
        }
        fn text(&mut self, b: &[u8]) -> Flow {
            self.out.push(format!("t {}", s(b)));
            Flow::Continue(())
        }
        fn cdata(&mut self, b: &[u8]) -> Flow {
            self.out.push(format!("cdata {}", s(b)));
            Flow::Continue(())
        }
        fn comment(&mut self, b: &[u8]) -> Flow {
            self.out.push(format!("c {}", s(b)));
            Flow::Continue(())
        }
        fn pi(&mut self, t: &[u8], d: &[u8]) -> Flow {
            self.out.push(format!("pi {} {}", s(t), s(d)));
            Flow::Continue(())
        }
    }

    fn stream(src: &str) -> (Result<(), ParseError>, Record) {
        let mut r = Record::default();
        let res = parse_streaming(src.as_bytes(), None, &mut r);
        (res, r)
    }

    /// The same document's tree, walked into the same report text: what the
    /// streaming parse reports is what the tree-building parse builds.
    fn walk(src: &str) -> Vec<String> {
        fn go(doc: &Document, n: NodeId, out: &mut Vec<String>) {
            match doc.type_(n) {
                Some(ArenaKind::Element) => {
                    let attrs: Vec<String> = doc
                        .attributes(n)
                        .map(|a| format!("{}={}", s(doc.qname(a)), s(doc.value(a))))
                        .collect();
                    let uri = s(doc.span(doc.node(n).ns_uri));
                    out.push(format!(
                        "<{} {{{uri}}} [{}]",
                        s(doc.qname(n)),
                        attrs.join(" ")
                    ));
                    for c in doc.children(n) {
                        go(doc, c, out);
                    }
                    out.push(format!(">{}", s(doc.qname(n))));
                }
                Some(ArenaKind::Text) => out.push(format!("t {}", s(doc.value(n)))),
                Some(ArenaKind::CDataSection) => out.push(format!("cdata {}", s(doc.value(n)))),
                Some(ArenaKind::Comment) => out.push(format!("c {}", s(doc.value(n)))),
                Some(ArenaKind::Pi) => {
                    out.push(format!("pi {} {}", s(doc.local(n)), s(doc.value(n))))
                }
                _ => {}
            }
        }
        let doc = parse(src.as_bytes()).unwrap();
        let mut out = Vec::new();
        for c in doc.children(doc.doc_node()) {
            go(&doc, c, &mut out);
        }
        out
    }

    #[test]
    fn a_stream_reports_what_the_tree_holds() {
        let src = r#"<!--before--><?app go?><r xmlns:f="u:f" a="1"><f:x f:b="2">one</f:x><y/><![CDATA[c]]><!--in--><?pi d?>tail</r><!--after-->"#;
        let (res, r) = stream(src);
        res.unwrap();
        assert_eq!(r.out, walk(src));
    }

    #[test]
    fn references_are_expanded_and_the_declaration_reported() {
        let (res, r) =
            stream(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><r>t&amp;x&#65;</r>"#);
        res.unwrap();
        assert_eq!(r.out, ["decl 1.0 UTF-8 yes", "<r {} []", "t t&xA", ">r"]);
    }

    #[test]
    fn the_arena_follows_the_open_elements_not_the_document() {
        /* 10,000 siblings, each with text: a tree would take some 30,000
         * nodes; the stream gives each back when it closes. */
        let src = format!("<r>{}</r>", "<e a=\"1\">text</e>".repeat(10_000));
        let (res, r) = stream(&src);
        res.unwrap();
        assert_eq!(r.out.len(), 2 + 3 * 10_000);
        assert!(r.top_index < 16, "the arena grew to {}", r.top_index);
    }

    #[test]
    fn a_stream_refuses_what_a_tree_refuses_at_the_same_point() {
        for bad in [
            "<r><a></b></r>",
            "<r>",
            "<r/><r/>",
            "<p:r/>",
            "<r a='1' a='2'/>",
            "x<r/>",
        ] {
            assert!(parse(bad.as_bytes()).is_err(), "{bad}");
            let (res, _) = stream(bad);
            assert!(matches!(res, Err(e) if e != ParseError::Stopped), "{bad}");
        }
        /* Reports run up to the refusal. */
        let (_, r) = stream("<r><a>ok</b></r>");
        assert_eq!(r.out, ["<r {} []", "<a {} []", "t ok"]);
    }

    #[test]
    fn a_receiver_stops_the_parse() {
        let mut r = Record {
            stop_at: Some("b"),
            ..Record::default()
        };
        let res = parse_streaming(b"<r><a/><b/><c/></r>", None, &mut r);
        assert_eq!(res, Err(ParseError::Stopped));
        assert_eq!(r.out, ["<r {} []", "<a {} []", ">a", "<b {} []"]);
    }
}
