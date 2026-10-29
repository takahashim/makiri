//! The streaming XML parse (`xml::tree::parse_streaming`, the ground of the
//! SAX parser) against the tree-building one, on raw bytes.
//!
//! The two run one parser, so they must agree: on an input the tree parse
//! accepts, the stream accepts it too and reports exactly the tree, node for
//! node in document order; on an input it refuses, the stream refuses it with
//! the same status. The one allowed difference is the arena budget: a stream
//! gives its arena back node by node, so a document the tree parse refuses as
//! too big (`Limit`) may stream through.
#![no_main]
/* The harness records what it compares; an injected allocation failure
 * belongs in the code under test, not here (see common.rs's `Expr`). */
#![allow(clippy::disallowed_methods)]

use libfuzzer_sys::fuzz_target;
use makiri::xml::tree::events::{Events, Flow, XmlDecl};
use makiri::xml::tree::{parse, parse_streaming};
use makiri::xml::{ArenaKind, Document, NodeId, ParseError};

#[derive(PartialEq, Eq, Debug)]
enum Ev {
    Start {
        name: Vec<u8>,
        uri: Vec<u8>,
        attrs: Vec<(Vec<u8>, Vec<u8>, Vec<u8>)>,
    },
    End(Vec<u8>),
    Text(Vec<u8>),
    CData(Vec<u8>),
    Comment(Vec<u8>),
    Pi(Vec<u8>, Vec<u8>),
}

fn start(doc: &Document, el: NodeId) -> Ev {
    Ev::Start {
        name: doc.qname(el).to_vec(),
        uri: doc.ns(el).to_vec(),
        attrs: doc
            .attributes(el)
            .map(|a| {
                (
                    doc.qname(a).to_vec(),
                    doc.ns(a).to_vec(),
                    doc.value(a).to_vec(),
                )
            })
            .collect(),
    }
}

/// What the stream reported, adjacent character data joined as the tree
/// joins it (`append_chardata` coalesces a run of one kind).
#[derive(Default)]
struct Record(Vec<Ev>);

impl Record {
    fn chardata(&mut self, b: &[u8], cdata: bool) {
        match (self.0.last_mut(), cdata) {
            (Some(Ev::Text(t)), false) | (Some(Ev::CData(t)), true) => t.extend_from_slice(b),
            _ => self.0.push(if cdata {
                Ev::CData(b.to_vec())
            } else {
                Ev::Text(b.to_vec())
            }),
        }
    }
}

impl Events for Record {
    const STREAMING: bool = true;
    fn xml_decl(&mut self, _: &XmlDecl<'_>) -> Flow {
        Flow::Continue(())
    }
    fn start_element(&mut self, doc: &Document, el: NodeId) -> Flow {
        self.0.push(start(doc, el));
        Flow::Continue(())
    }
    fn end_element(&mut self, doc: &Document, el: NodeId) -> Flow {
        self.0.push(Ev::End(doc.qname(el).to_vec()));
        Flow::Continue(())
    }
    fn text(&mut self, b: &[u8]) -> Flow {
        self.chardata(b, false);
        Flow::Continue(())
    }
    fn cdata(&mut self, b: &[u8]) -> Flow {
        self.chardata(b, true);
        Flow::Continue(())
    }
    fn comment(&mut self, b: &[u8]) -> Flow {
        self.0.push(Ev::Comment(b.to_vec()));
        Flow::Continue(())
    }
    fn pi(&mut self, t: &[u8], d: &[u8]) -> Flow {
        self.0.push(Ev::Pi(t.to_vec(), d.to_vec()));
        Flow::Continue(())
    }
}

/// The tree, walked in document order into the same events.
fn walk(doc: &Document, n: NodeId, out: &mut Vec<Ev>) {
    match doc.type_(n) {
        Some(ArenaKind::Element) => {
            out.push(start(doc, n));
            for c in doc.children(n) {
                walk(doc, c, out);
            }
            out.push(Ev::End(doc.qname(n).to_vec()));
        }
        Some(ArenaKind::Text) => out.push(Ev::Text(doc.value(n).to_vec())),
        Some(ArenaKind::CDataSection) => out.push(Ev::CData(doc.value(n).to_vec())),
        Some(ArenaKind::Comment) => out.push(Ev::Comment(doc.value(n).to_vec())),
        Some(ArenaKind::Pi) => out.push(Ev::Pi(doc.local(n).to_vec(), doc.value(n).to_vec())),
        _ => {}
    }
}

fuzz_target!(|data: &[u8]| {
    let tree = parse(data);
    let mut rec = Record::default();
    let streamed = parse_streaming(data, None, &mut rec);
    match tree {
        Ok(doc) => {
            assert_eq!(
                streamed,
                Ok(()),
                "the stream refused what the tree accepted"
            );
            let mut want = Vec::new();
            for c in doc.children(doc.doc_node()) {
                walk(&doc, c, &mut want);
            }
            assert_eq!(rec.0, want, "the stream reported other than the tree");
        }
        /* The tree's budget; the stream gives its arena back. */
        Err(ParseError::Limit) => {}
        Err(e) => assert_eq!(streamed, Err(e), "the two refused differently"),
    }
});
