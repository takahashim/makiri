//! The arena, tree and mutation self-checks, run by `cargo test`.
//!
//! They reach edge and overflow states no public API can construct - a budget
//! lowered below what the document already holds, a hand-linked sibling chain, a
//! document node replaced under a live tree - which is why they live inside the
//! crate instead of in `spec/`. The tree is an index arena, so they address
//! nodes by `NodeId` through the `Document`.
//!
//! Each check names the fact it is about, in its own `#[test]` where the state
//! allows one. Where a run of checks genuinely depends on an earlier mutation of
//! the same document, they stay together and a fixture function builds the
//! shared starting point.

#![forbid(unsafe_code)]

use crate::xml::mutate;
use crate::xml::qname;
use crate::xml::tree::{parse, parse_ex, parse_fragment};
use crate::xml::{
    ArenaError, Document, Limits, Link, MutStatus, NodeId, NodeType, Status, MAX_BYTES,
    XMLNS_NS_URI, XML_NS_URI,
};

/* ------------------------------------------------------------------ */
/* helpers                                                            */
/* ------------------------------------------------------------------ */

/// A byte literal as something a failure message can print.
fn show(s: &[u8]) -> String {
    String::from_utf8_lossy(s).into_owned()
}

fn doc_new() -> Box<Document> {
    Document::create(None, 0).expect("a fresh document")
}

/// Parse a literal that must be well-formed.
fn parse_ok(s: &[u8]) -> Box<Document> {
    parse(s).unwrap_or_else(|e| panic!("{} must parse, got {e:?}", show(s)))
}

/// `s` must be refused, and with exactly `want` - a document that fails for the
/// wrong reason is as much a bug as one that is accepted.
fn assert_rejected(s: &[u8], want: Status) {
    match parse(s) {
        Ok(_) => panic!("accepted {}, which must fail with {want:?}", show(s)),
        Err(got) => assert_eq!(got, want, "wrong status refusing {}", show(s)),
    }
}

/* The navigation these checks do is always "the node that must be there", so
 * the accessors below fail with what was missing rather than handing back
 * `NodeId::INVALID` for a later comparison to trip over. */

fn root_of(d: &Document) -> NodeId {
    d.root().expect("a root element")
}
fn child(d: &Document, n: NodeId) -> NodeId {
    d.first_child(n).expect("a first child")
}
fn sibling(d: &Document, n: NodeId) -> NodeId {
    d.next(n).expect("a next sibling")
}
fn attr(d: &Document, n: NodeId) -> NodeId {
    d.first_attr(n).expect("an attribute")
}

/// Parse the first `len` bytes of `src`, checking `len` against the byte budget
/// BEFORE the bytes are touched - a caller can reach the entry point with a
/// length longer than the buffer it holds.
fn parse_ex_len(src: &[u8], len: usize, limits: Option<usize>) -> Result<Box<Document>, Status> {
    let max = limits.filter(|&n| n != 0).unwrap_or(MAX_BYTES);
    if len > max {
        return Err(Status::Limit);
    }
    parse_limited(&src[..len], limits)
}

/// `parse_ex` under an optional byte budget.
fn parse_limited(src: &[u8], limits: Option<usize>) -> Result<Box<Document>, Status> {
    match limits {
        Some(max_bytes) => parse_ex(src, Some(&Limits { max_bytes })),
        None => parse(src),
    }
}

/// A fragment of `src` into `doc`, refused past the document's byte budget.
fn parse_fragment_checked(
    doc: &mut Document,
    src: &[u8],
    inherit_doc_ns: bool,
) -> Result<NodeId, Status> {
    if src.len() > doc.max_bytes {
        return Err(Status::Limit);
    }
    parse_fragment(doc, src, inherit_doc_ns)
}

/* ------------------------------------------------------------------ */
/* the arena                                                          */
/* ------------------------------------------------------------------ */

#[test]
fn a_fresh_node_is_zeroed_and_a_stored_slice_is_copied() {
    let mut doc = doc_new();
    let root = doc.new_node(NodeType::Element).expect("a new element");
    let local = doc.store(b"Feed").expect("room for the name");

    assert!(
        doc.node(root).first_child.is_none(),
        "a fresh node has no first child"
    );
    assert_eq!(doc.type_(root), Some(NodeType::Element));
    assert_eq!(
        doc.span(local),
        b"Feed",
        "the name was copied into the store"
    );
    /* An unset name field is the ABSENT marker, not a stray span into the
     * store - the difference a doctype's `PUBLIC ""` depends on. */
    assert!(
        doc.node(root).qname.is_absent(),
        "an unset qname is ABSENT, not an empty span"
    );
}

#[test]
fn a_thousand_children_link_into_one_chain() {
    let mut doc = doc_new();
    let root = doc.new_node(NodeType::Element).expect("a new element");
    for n in 0..1000 {
        let c = doc
            .new_node(NodeType::Element)
            .unwrap_or_else(|e| panic!("child {n} of 1000: {e:?}"));
        doc.append_child(root, c);
    }

    let mut cnt = 0;
    let mut c = doc.first_child(root);
    while let Some(id) = c {
        cnt += 1;
        c = doc.next(id);
    }
    assert_eq!(cnt, 1000, "every appended child is reachable by `next`");
}

#[test]
fn a_byte_budget_already_spent_refuses_the_next_node() {
    let mut doc = doc_new();
    /* Not reachable through the public API: the budget is lowered to what the
     * document has ALREADY charged, so there is no room for one more node. */
    doc.max_bytes = doc.arena_bytes;
    assert_eq!(
        doc.new_node(NodeType::Element).err(),
        Some(ArenaError::Limit),
        "no room left for a node, and the reason is the budget"
    );
}

#[test]
fn the_byte_budget_is_enforced_inside_the_allocator() {
    let mut doc = doc_new();
    doc.max_bytes = 4096;
    for _ in 0..100_000 {
        if let Err(st) = doc.new_node(NodeType::Element) {
            assert_eq!(st, ArenaError::Limit);
            return;
        }
    }
    panic!("100000 nodes fitted in a 4096-byte budget");
}

#[test]
fn the_node_budget_is_enforced() {
    let mut doc = doc_new();
    doc.max_nodes = 10;
    for _ in 0..100 {
        if let Err(st) = doc.new_node(NodeType::Element) {
            assert_eq!(st, ArenaError::Limit);
            return;
        }
    }
    panic!("100 nodes fitted in a 10-node budget");
}

/* ------------------------------------------------------------------ */
/* parsing                                                            */
/* ------------------------------------------------------------------ */

#[test]
fn a_document_parses_into_elements_attributes_and_text() {
    let doc = parse_ok(b"<Feed x='1' y='two'>hi<b/>z</Feed>");
    let root = root_of(&doc);
    assert_eq!(doc.local(root), b"Feed");
    assert_eq!(doc.type_(root), Some(NodeType::Element));
    assert_eq!(doc.node(root).line, 1, "the root starts on line 1");

    let a0 = attr(&doc, root);
    let a1 = sibling(&doc, a0);
    assert_eq!(doc.type_(a0), Some(NodeType::Attribute));
    assert_eq!(doc.local(a0), b"x");
    assert_eq!(doc.value(a0), b"1");
    assert_eq!(doc.local(a1), b"y");
    assert_eq!(doc.value(a1), b"two");
    assert!(doc.next(a1).is_none(), "and no third attribute");

    let c0 = child(&doc, root);
    let c1 = sibling(&doc, c0);
    let c2 = sibling(&doc, c1);
    assert_eq!(doc.type_(c0), Some(NodeType::Text));
    assert_eq!(doc.value(c0), b"hi");
    assert_eq!(doc.type_(c1), Some(NodeType::Element));
    assert_eq!(doc.local(c1), b"b");
    assert!(doc.first_child(c1).is_none(), "<b/> is empty");
    assert_eq!(doc.type_(c2), Some(NodeType::Text));
    assert_eq!(doc.value(c2), b"z");
    assert!(doc.next(c2).is_none(), "and no fourth child");

    /* The parent and prev links, which nothing above reads. */
    assert_eq!(doc.parent(c1), Some(root));
    assert_eq!(doc.prev(c1), Some(c0));
    assert_eq!(doc.prev(c2), Some(c1));
}

#[test]
fn element_names_are_case_sensitive() {
    let doc = parse_ok(b"<X><x/></X>");
    let root = root_of(&doc);
    assert_eq!(doc.local(root), b"X");
    assert_eq!(doc.local(child(&doc, root)), b"x");
}

#[test]
fn well_formedness_errors_fail_closed() {
    for s in [
        &b"<a>"[..],
        b"<a></b>",
        b"<a/><b/>",
        b"x<a/>",
        b"<a x=>",
        b"<a y='<'>",
    ] {
        assert_rejected(s, Status::Syntax);
    }
}

#[test]
fn the_predefined_entities_and_character_references_expand() {
    let doc = parse_ok(b"<a x='p&amp;q' y='&#65;&#x42;'>1&lt;2&gt;3&amp;4&apos;5&quot;6</a>");
    let r = root_of(&doc);
    let ax = attr(&doc, r);
    let ay = sibling(&doc, ax);
    let tx = child(&doc, r);
    assert_eq!(doc.value(ax), b"p&q");
    assert_eq!(doc.value(ay), b"AB");
    assert_eq!(doc.type_(tx), Some(NodeType::Text));
    assert_eq!(doc.value(tx), b"1<2>3&4'5\"6");
}

#[test]
fn bad_references_fail_closed() {
    for s in [
        &b"<a>&nbsp;</a>"[..], /* undeclared entity */
        b"<a>x & y</a>",       /* a bare ampersand */
        b"<a>&#0;</a>",        /* U+0000 is not an XML Char */
        b"<a>&#xD800;</a>",    /* a surrogate */
        b"<a>&#;</a>",         /* no digits */
    ] {
        assert_rejected(s, Status::Syntax);
    }
}

#[test]
fn namespaces_resolve_on_elements_and_attributes() {
    let doc = parse_ok(b"<a:e xmlns:a='urn:a' xmlns='urn:d' a:x='1' y='2'><c/></a:e>");
    let r = root_of(&doc);
    assert_eq!(doc.local(r), b"e");
    assert_eq!(doc.prefix(r), b"a");
    assert_eq!(doc.ns(r), b"urn:a");

    let c = child(&doc, r);
    assert_eq!(doc.local(c), b"c");
    assert_eq!(doc.ns(c), b"urn:d", "an unprefixed child takes the default");

    /* The xmlns declarations stay as attribute nodes, in source order, and in
     * the XMLNS namespace (§7.2). */
    let a = attr(&doc, r);
    assert_eq!(doc.local(a), b"a");
    assert_eq!(doc.prefix(a), b"xmlns");
    assert_eq!(doc.ns(a), XMLNS_NS_URI);

    let a = sibling(&doc, a);
    assert_eq!(doc.local(a), b"xmlns");
    assert_eq!(doc.node(a).prefix.len, 0, "`xmlns` alone has no prefix");

    let a = sibling(&doc, a);
    assert_eq!(doc.local(a), b"x");
    assert_eq!(doc.prefix(a), b"a");
    assert_eq!(doc.ns(a), b"urn:a");
    assert_eq!(doc.value(a), b"1");

    let a = sibling(&doc, a);
    assert_eq!(doc.local(a), b"y");
    assert_eq!(doc.node(a).prefix.len, 0);
    assert_eq!(
        doc.node(a).ns_uri.len,
        0,
        "an unprefixed ATTRIBUTE is in no namespace, default or not"
    );
    assert_eq!(doc.value(a), b"2");
}

#[test]
fn namespace_errors_fail_closed() {
    for s in [
        &b"<a:b/>"[..],            /* unbound prefix */
        b"<a x:y='1'/>",           /* unbound attribute prefix */
        b"<a xmlns:xml='wrong'/>", /* the reserved xml: prefix */
        b"<a:b xmlns:a=''/>",      /* XML 1.0 forbids xmlns:p="" */
    ] {
        assert_rejected(s, Status::Syntax);
    }
}

#[test]
fn attribute_values_normalize_literal_whitespace_but_not_references() {
    let doc = parse_ok(b"<a x=\"p\tq\nr\" y=\"p&#9;q&#10;r\">u\tv\nw</a>");
    let r = root_of(&doc);
    let ax = attr(&doc, r);
    let ay = sibling(&doc, ax);
    let tx = child(&doc, r);
    assert_eq!(doc.value(ax), b"p q r", "§3.3.3 folds a LITERAL tab/LF");
    assert_eq!(
        doc.value(ay),
        b"p\tq\nr",
        "a reference-derived one survives"
    );
    assert_eq!(doc.type_(tx), Some(NodeType::Text));
    assert_eq!(doc.value(tx), b"u\tv\nw", "text is not folded at all");
}

#[test]
fn comments_cdata_and_pis_become_nodes_inside_and_around_the_root() {
    let doc = parse_ok(
        b"<?xml version=\"1.0\"?><?xml-stylesheet href=\"x\"?><!--top--><r><!--c--><![CDATA[a<b]]><?pi dat?></r><?tail t?>",
    );
    let r = root_of(&doc);
    assert_eq!(doc.local(r), b"r");

    let cm = child(&doc, r);
    let cd = sibling(&doc, cm);
    let pi = sibling(&doc, cd);
    assert_eq!(doc.type_(cm), Some(NodeType::Comment));
    assert_eq!(doc.value(cm), b"c");
    assert_eq!(doc.type_(cd), Some(NodeType::CData));
    assert_eq!(doc.value(cd), b"a<b");
    assert_eq!(doc.type_(pi), Some(NodeType::Pi));
    assert_eq!(doc.local(pi), b"pi");
    assert_eq!(doc.value(pi), b"dat");
    assert!(doc.next(pi).is_none(), "and nothing after the PI");

    /* The prolog's PI and comment are kept, as siblings of the root. */
    let dn = doc.doc_node();
    let p1 = child(&doc, dn);
    let p2 = sibling(&doc, p1);
    let p3 = sibling(&doc, p2);
    let p4 = sibling(&doc, p3);
    assert_eq!(doc.type_(p1), Some(NodeType::Pi));
    assert_eq!(doc.local(p1), b"xml-stylesheet");
    assert_eq!(doc.type_(p2), Some(NodeType::Comment));
    assert_eq!(doc.value(p2), b"top");
    assert_eq!(p3, r, "the root is the document node's third child");
    assert_eq!(doc.parent(r), Some(dn));
    assert_eq!(doc.type_(p4), Some(NodeType::Pi));
    assert_eq!(doc.local(p4), b"tail");
    assert!(doc.next(p4).is_none(), "and nothing after the trailing PI");
}

#[test]
fn section_9_violations_fail_closed() {
    for s in [
        &b"<r/><!DOCTYPE r>"[..],                    /* a DOCTYPE after the root */
        b"<r><!-- a--b --></r>",                     /* '--' inside a comment */
        b"<r><!-- c </r>",                           /* unterminated comment */
        b" <?xml version=\"1.0\"?><r/>",             /* the declaration must be first */
        b"<![CDATA[x]]><r/>",                        /* CDATA outside the root */
        b"<r><?a:b x?></r>",                         /* NS §7: PI target is an NCName */
        b"<!DOCTYPE r [ <!BOGUS> ]><r/>",            /* §5.1: the subset is checked */
        b"<!DOCTYPE r [ <!ENTITY e \"%p;\"> ]><r/>", /* WFC: PEs in Internal Subset */
    ] {
        assert_rejected(s, Status::Syntax);
    }
}

#[test]
fn a_dtd_construct_makiri_would_have_to_apply_is_refused_not_ignored() {
    /* Well-formed every one of them; each would change the tree if applied, so
     * the parse fails rather than answering a document the DTD disagrees with. */
    for s in [
        &b"<!DOCTYPE r [ <!ENTITY x \"y\"> ]><r>&x;</r>"[..], /* a declared entity, referenced */
        b"<!DOCTYPE r [ <!ATTLIST r k CDATA \"d\"> ]><r/>",   /* an attribute default */
        b"<!DOCTYPE r [ <!ATTLIST r k ID #IMPLIED> ]><r/>",   /* a non-CDATA type */
        b"<!DOCTYPE r [ <!ENTITY % p \"x\"> %p; ]><r/>",      /* a parameter entity */
    ] {
        assert_rejected(s, Status::Unsupported);
    }
}

#[test]
fn a_doctype_is_recognized_and_kept_but_not_processed() {
    let doc = parse_ok(b"<!DOCTYPE r SYSTEM \"a>b\" [ <!ELEMENT r (#PCDATA)> ]><r>ok</r>");
    let r = root_of(&doc);
    assert_eq!(doc.local(r), b"r");
    assert_eq!(doc.value(child(&doc, r)), b"ok");

    let dt = doc.doctype().expect("a doctype node");
    assert_eq!(doc.type_(dt), Some(NodeType::Doctype));
    assert_eq!(doc.parent(dt), Some(doc.doc_node()));
    assert_eq!(doc.first_child(doc.doc_node()), Some(dt));
    assert!(doc.prev(dt).is_none(), "the doctype comes first");
    assert_eq!(doc.next(dt), Some(r), "and the root next");
    assert_eq!(doc.local(dt), b"r");
    assert!(
        doc.node(dt).prefix.is_absent(),
        "no PUBLIC id was written, which is not the same as an empty one"
    );
    assert_eq!(doc.value(dt), b"a>b", "the SYSTEM id may hold '>'");
}

/// A doctype keeps its PUBLIC id in `prefix`, so a copy that split its name as
/// a qname read the id's length as a prefix length: the copy's PUBLIC id was
/// the name's bytes and whatever followed them, and an absent one came back
/// as `PUBLIC ""`.
#[test]
fn a_copied_doctype_keeps_its_name_and_both_ids() {
    /// (source, PUBLIC id, SYSTEM id)
    type Case = (&'static [u8], Option<&'static [u8]>, Option<&'static [u8]>);
    let cases: [Case; 4] = [
        (
            b"<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Strict//EN\" \"x.dtd\"><html/>",
            Some(b"-//W3C//DTD XHTML 1.0 Strict//EN"),
            Some(b"x.dtd"),
        ),
        (b"<!DOCTYPE r SYSTEM \"r.dtd\"><r/>", None, Some(b"r.dtd")),
        (b"<!DOCTYPE r PUBLIC \"\" \"\"><r/>", Some(b""), Some(b"")),
        (b"<!DOCTYPE r><r/>", None, None),
    ];
    for (src, public, system) in cases {
        let mut doc = parse_ok(src);
        let dt = doc.doctype().expect("a doctype node");
        let name = crate::falloc::try_to_vec(doc.local(dt)).expect("name");

        let mut other = doc_new();
        let imported = mutate::import_subtree(&mut other, &doc, dt).expect("import");
        let cloned = mutate::clone_node(&mut doc, dt, true).expect("clone");
        for (d, copy) in [(&*other, imported), (&*doc, cloned)] {
            let ids = d.doctype_ids(copy).expect("a doctype copy");
            assert_eq!(d.local(copy), &name[..], "{}", show(src));
            assert_eq!(d.qname(copy), &name[..], "{}", show(src));
            assert_eq!(ids.public, public, "PUBLIC of {}", show(src));
            assert_eq!(ids.system, system, "SYSTEM of {}", show(src));
        }
    }
}

#[test]
fn line_endings_normalize_to_lf() {
    let doc = parse_ok(b"<a x=\"p\r\nq\r\">m\r\nn\ro</a>");
    let r = root_of(&doc);
    let ax = attr(&doc, r);
    let tx = child(&doc, r);
    assert_eq!(
        doc.value(ax),
        b"p q ",
        "CRLF folds to LF, then LF to a space"
    );
    assert_eq!(doc.type_(tx), Some(NodeType::Text));
    assert_eq!(doc.value(tx), b"m\nn\no");
}

#[test]
fn strict_names_duplicate_attributes_and_a_bare_cdata_close_fail_closed() {
    for s in [
        &b"<1bad/>"[..],                                 /* not a NameStartChar */
        b"<a:1b xmlns:a='u'/>",                          /* nor is the local part */
        b"<a x='1' x='2'/>",                             /* §9.3, by raw QName */
        b"<e xmlns:a='u' xmlns:b='u' a:x='1' b:x='2'/>", /* §9.3, by (ns, local) */
        b"<a>foo]]>bar</a>",                             /* §2.4 */
    ] {
        assert_rejected(s, Status::Syntax);
    }
    /* Only the full "]]>" is forbidden, not the brackets that lead to it. */
    let doc = parse_ok(b"<a>1]2]]3</a>");
    let r = root_of(&doc);
    assert_eq!(doc.value(child(&doc, r)), b"1]2]]3");
}

#[test]
fn the_xml_declaration_grammar_is_enforced() {
    parse_ok(b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><r/>");
    for s in [
        &b"<?xml VERSION=\"1.0\"?><r/>"[..], /* keywords are lowercase */
        b"<?xml version=\"1.0\" standalone=\"YES\"?><r/>", /* so are the values */
        b"<?xml encoding=\"UTF-8\"?><r/>",   /* version is required */
        b"<?xml version=\"1.0\"encoding=\"UTF-8\"?><r/>", /* S-separated */
        b"<?xml version=\"1.0\" version=\"1.0\"?><r/>", /* once each */
        b"<?xml version=\"1.0\" valid=\"no\"?><r/>", /* and no others */
        b"<?xml version=\"1.0' ?><r/>",      /* mismatched quotes */
        b"<?xml version=\"1.0^\"?><r/>",     /* not a VersionNum */
        b"<?XML version=\"1.0\"?><r/>",      /* §2.6 reserved target */
        b"<a x=\"1\"y=\"2\"/>",              /* §3.1 S-separated */
        b"<a>&#X58;</a>",                    /* §4.1: lowercase 'x' only */
    ] {
        assert_rejected(s, Status::Syntax);
    }
    /* §2.8: a 1.x label is read as 1.0; 2.0 is not a VersionNum at all. */
    parse_ok(b"<?xml version=\"1.1\"?><r/>");
    parse_ok(b"<?xml version=\"1.5\"?><r/>");
    assert_rejected(b"<?xml version=\"2.0\"?><r/>", Status::Syntax);
}

#[test]
fn the_byte_budget_is_checked_before_the_source_is_read() {
    let tiny = b"<r/>";
    /* An over-long LENGTH is refused without `parse_ex_len` ever slicing - which
     * it could not, holding only four bytes. */
    assert_eq!(
        parse_ex_len(tiny, MAX_BYTES + 1, None).err(),
        Some(Status::Limit)
    );
    parse_ok(tiny);
}

#[test]
fn a_per_parse_byte_budget_overrides_the_default() {
    let src = b"<root><a/><b/><c/></root>";
    for max in [2, 64] {
        assert_eq!(
            parse_ex_len(src, src.len(), Some(max)).err(),
            Some(Status::Limit),
            "{max} bytes cannot hold this document's arena"
        );
    }
    let d = parse_ex_len(src, src.len(), Some(1024 * 1024)).expect("a megabyte is plenty");
    assert_eq!(d.local(root_of(&d)), b"root");
    /* 0 is not "no room": it selects the default budget. */
    parse_ex_len(src, src.len(), Some(0)).expect("0 means the default budget");
}

#[test]
fn a_fragment_holds_several_top_level_nodes() {
    let mut fd = doc_new();
    let frag = parse_fragment_checked(&mut fd, b"<a/>txt<p:b xmlns:p='urn:p'>x</p:b>", false)
        .expect("a well-formed fragment");
    assert_eq!(fd.type_(frag), Some(NodeType::Fragment));

    let c0 = child(&fd, frag);
    let c1 = sibling(&fd, c0);
    let c2 = sibling(&fd, c1);
    assert_eq!(fd.type_(c0), Some(NodeType::Element));
    assert_eq!(fd.local(c0), b"a");
    assert_eq!(fd.type_(c1), Some(NodeType::Text));
    assert_eq!(fd.value(c1), b"txt");
    assert_eq!(fd.local(c2), b"b");
    assert_eq!(fd.ns(c2), b"urn:p", "a fragment may declare its own prefix");
    assert!(fd.next(c2).is_none(), "and no fourth top-level node");
}

#[test]
fn a_fragment_fails_closed() {
    let mut fd = doc_new();
    for s in [
        &b"<?xml version='1.0'?>"[..], /* no declaration in a fragment */
        b"<!DOCTYPE r>",               /* nor a doctype */
        b"</x>",                       /* nor an unmatched end tag */
        b"<a>",                        /* nor an unclosed element */
        b"<p:a/>",                     /* and an unbound prefix is still unbound */
    ] {
        assert!(
            parse_fragment_checked(&mut fd, s, false).is_err(),
            "accepted the fragment {}",
            show(s)
        );
    }
}

#[test]
fn a_fragment_can_inherit_the_documents_namespaces() {
    let mut fd = parse_ok(b"<r xmlns:p='urn:p' xmlns='urn:d'/>");
    let frag = parse_fragment_checked(&mut fd, b"<p:a/><plain/>", true)
        .expect("the root's prefixes are in scope");
    let a = child(&fd, frag);
    let plain = sibling(&fd, a);
    assert_eq!(fd.ns(a), b"urn:p", "the declared prefix resolved");
    assert_eq!(fd.ns(plain), b"urn:d", "and so did the default");
}

/* ------------------------------------------------------------------ */
/* mutation                                                           */
/* ------------------------------------------------------------------ */

/// A document holding one detached, named element - the starting point for every
/// check about a node that is not yet in a tree.
fn detached_element(name: &[u8]) -> (Box<Document>, NodeId) {
    let mut doc = doc_new();
    let el = mutate::new_element(&mut doc, name).expect("a new element");
    (doc, el)
}

#[test]
fn split_checked_accepts_a_qname_and_refuses_the_ncname_violations() {
    let q = qname::split_checked(b"a:b").expect("a:b is a QName");
    assert_eq!(q.prefix_len, 1);
    assert_eq!(q.local_len, 1);
    for bad in [
        &b"1bad"[..], /* not a NameStartChar */
        b"a:b:c",     /* a second colon */
        b":x",        /* empty prefix */
        b"x:",        /* empty local part */
    ] {
        assert!(
            qname::split_checked(bad).is_none(),
            "accepted {} as a QName",
            show(bad)
        );
    }
}

#[test]
fn setting_an_attribute_twice_replaces_its_value() {
    let (mut doc, r) = detached_element(b"r");
    let at = mutate::set_attribute(&mut doc, r, b"id", b"x").expect("a new attribute");
    assert_eq!(doc.node(at).value.len, 1);
    assert_eq!(doc.value(at), b"x");
    assert_eq!(
        doc.first_attr(r),
        Some(at),
        "it is the element's first attribute"
    );

    let again = mutate::set_attribute(&mut doc, r, b"id", b"yy").expect("a replacement");
    assert_eq!(doc.node(again).value.len, 2);
    assert_eq!(doc.value(again), b"yy");
    /* Asserted on the ELEMENT, not on `again`: a freshly APPENDED attribute is
     * also the tail, so `doc.next(again).is_none()` holds either way and cannot
     * tell a replacement from a second attribute. */
    assert_eq!(
        doc.first_attr(r),
        Some(at),
        "a replacement reuses the existing attribute node"
    );
    assert_eq!(again, at, "and hands the same node back");
    assert!(
        doc.next(at).is_none(),
        "so the element still has exactly one attribute"
    );
}

#[test]
fn an_attribute_value_that_is_not_xml_char_is_refused() {
    let (mut doc, r) = detached_element(b"r");
    assert_eq!(
        mutate::set_attribute(&mut doc, r, b"k", b"\x01"),
        Err(MutStatus::BadChars)
    );
}

#[test]
fn a_leaf_value_holding_its_own_close_sequence_is_refused() {
    let mut doc = doc_new();
    /* Each of these would end the construct early once serialized, so the value
     * is refused rather than escaped. */
    assert_eq!(
        mutate::new_chardata(&mut doc, NodeType::Comment, b"a--b"),
        Err(MutStatus::BadChars)
    );
    assert_eq!(
        mutate::new_chardata(&mut doc, NodeType::Comment, b"x-"),
        Err(MutStatus::BadChars),
        "a trailing '-' would make '-->' out of the close"
    );
    assert_eq!(
        mutate::new_chardata(&mut doc, NodeType::CData, b"a]]>b"),
        Err(MutStatus::BadChars)
    );

    let ok = mutate::new_chardata(&mut doc, NodeType::Comment, b"a-b").expect("one '-' is fine");
    assert_eq!(
        mutate::set_content(&mut doc, ok, b"x--y"),
        Err(MutStatus::BadChars),
        "and the rule holds on a later write, not just at creation"
    );
}

#[test]
fn a_prefix_may_not_be_bound_to_the_empty_namespace() {
    let (mut doc, r) = detached_element(b"r");
    assert_eq!(
        mutate::set_attribute(&mut doc, r, b"xmlns:q", b""),
        Err(MutStatus::BadNsDecl(
            crate::xml::qname::NsDeclError::PrefixToEmpty
        ))
    );
    /* `xmlns=""` is different: it un-declares the DEFAULT namespace, which XML
     * 1.0 allows. */
    mutate::set_attribute(&mut doc, r, b"xmlns", b"").expect("xmlns=\"\" is legal");
}

#[test]
fn an_unbound_prefix_on_a_detached_element_defers_rather_than_failing() {
    let (mut doc, det) = detached_element(b"det");
    let at = mutate::set_attribute(&mut doc, det, b"p:k", b"v")
        .expect("an unbound prefix is not an error while detached");
    assert_eq!(doc.node(at).ns_uri.len, 0, "it simply has no namespace yet");
}

#[test]
fn the_xml_prefix_is_bound_without_a_declaration() {
    let (mut doc, r) = detached_element(b"r");
    let at = mutate::set_attribute(&mut doc, r, b"xml:lang", b"en").expect("xml: is predefined");
    assert_eq!(doc.ns(at), XML_NS_URI);
}

#[test]
fn a_declaration_binds_the_prefix_for_a_later_attribute() {
    let (mut doc, r) = detached_element(b"r");
    let decl = mutate::set_attribute(&mut doc, r, b"xmlns:p", b"urn:p").expect("a declaration");
    assert_eq!(
        doc.ns(decl),
        XMLNS_NS_URI,
        "the declaration itself is in the XMLNS namespace"
    );
    let at = mutate::set_attribute(&mut doc, r, b"p:k", b"v").expect("now p: is bound");
    assert_eq!(doc.ns(at), b"urn:p");
}

#[test]
fn removing_an_attribute_is_idempotent() {
    let (mut doc, r) = detached_element(b"r");
    mutate::set_attribute(&mut doc, r, b"id", b"x").expect("an attribute to remove");
    assert!(mutate::remove_attribute(&mut doc, r, b"id"), "removed once");
    assert!(
        !mutate::remove_attribute(&mut doc, r, b"id"),
        "and reports nothing to remove the second time"
    );
}

#[test]
fn set_content_replaces_the_children_with_one_text_node() {
    let (mut doc, r) = detached_element(b"r");
    /* Hand-linked, so the child is there without an insertion having run. */
    let c1 = doc.new_node(NodeType::Element).expect("a child");
    doc.set_parent(c1, Some(r));
    doc.node_mut(r).first_child = Link::of(c1);
    doc.node_mut(r).last_child = Link::of(c1);

    assert_eq!(mutate::set_content(&mut doc, r, b"hi"), Ok(()));
    let fc = child(&doc, r);
    assert_eq!(doc.type_(fc), Some(NodeType::Text));
    assert_eq!(doc.node(fc).value.len, 2);
    assert_eq!(doc.last_child(r), Some(fc), "it is the only child");
    assert!(
        doc.parent(c1).is_none(),
        "the old child was detached, not destroyed"
    );

    assert_eq!(mutate::set_content(&mut doc, r, b""), Ok(()));
    assert!(doc.first_child(r).is_none(), "empty content means no child");
    assert!(doc.last_child(r).is_none());
}

/// `r` with `n` hand-linked element children. Hand-linked because the public
/// insert would resolve namespaces and sync document meta, and these checks are
/// about the link surgery alone.
fn chain_of_children(n: usize) -> (Box<Document>, NodeId, Vec<NodeId>) {
    let (mut doc, r) = detached_element(b"r");
    let mut kids = Vec::new();
    for _ in 0..n {
        let c = doc.new_node(NodeType::Element).expect("a child");
        doc.set_parent(c, Some(r));
        if let Some(&prev) = kids.last() {
            doc.node_mut(prev).next = Link::of(c);
            doc.node_mut(c).prev = Link::of(prev);
        } else {
            doc.node_mut(r).first_child = Link::of(c);
        }
        doc.node_mut(r).last_child = Link::of(c);
        kids.push(c);
    }
    (doc, r, kids)
}

#[test]
fn detach_unlinks_the_head_and_then_the_only_remaining_child() {
    let (mut doc, r, kids) = chain_of_children(2);
    let (a1, a2) = (kids[0], kids[1]);

    mutate::detach(&mut doc, a1);
    assert_eq!(doc.first_child(r), Some(a2), "the head moved on");
    assert!(doc.prev(a2).is_none(), "and its prev was cleared");
    assert!(doc.parent(a1).is_none(), "the removed node is detached");

    mutate::detach(&mut doc, a2);
    assert!(doc.first_child(r).is_none(), "emptying clears first_child");
    assert!(doc.last_child(r).is_none(), "and last_child");
    assert!(doc.parent(a2).is_none());
}

#[test]
fn detach_clears_the_removed_nodes_own_links_and_joins_its_neighbours() {
    /* The MIDDLE child, which is the only position where the detached node's own
     * `prev` is non-empty: detaching the head or the tail leaves it NONE either
     * way, so neither can catch a `clear_links` that forgets `prev`. */
    let (mut doc, r, kids) = chain_of_children(3);
    let (a1, a2, a3) = (kids[0], kids[1], kids[2]);

    mutate::detach(&mut doc, a2);

    assert!(doc.parent(a2).is_none(), "the removed node has no parent");
    assert!(doc.prev(a2).is_none(), "nor a prev");
    assert!(doc.next(a2).is_none(), "nor a next");
    assert_eq!(doc.next(a1), Some(a3), "its neighbours joined up");
    assert_eq!(doc.prev(a3), Some(a1));
    assert_eq!(doc.first_child(r), Some(a1), "the ends are unchanged");
    assert_eq!(doc.last_child(r), Some(a3));
}

/// A document whose document node holds a connected root `pr` declaring
/// `xmlns:p="urn:p"`. Being CONNECTED is what makes prefix resolution happen
/// rather than defer, so most insertion checks need it.
fn connected_root() -> (Box<Document>, NodeId, NodeId) {
    let mut doc = doc_new();
    /* A live document node of this document's own, so `sync_doc_meta` fires and
     * re-derives `root` from the tree. */
    let docn = doc.new_node(NodeType::Document).expect("a document node");
    doc.doc_node = docn;

    let pr = mutate::new_element(&mut doc, b"pr").expect("a root element");
    assert!(
        doc.parent(pr).is_none(),
        "a factory element starts detached"
    );
    assert_eq!(
        doc.node(pr).ns_uri.len,
        0,
        "and with no namespace decided yet"
    );
    mutate::set_attribute(&mut doc, pr, b"xmlns:p", b"urn:p").expect("a declaration");
    assert_eq!(mutate::insert_child(&mut doc, docn, pr), Ok(()));
    assert_eq!(doc.root(), Some(pr), "inserting it made it the root");
    (doc, docn, pr)
}

/// [`connected_root`] with one `p:c` child already resolved under the root.
fn connected_tree() -> (Box<Document>, NodeId, NodeId, NodeId) {
    let (mut doc, docn, pr) = connected_root();
    let ne = mutate::new_element(&mut doc, b"p:c").expect("a prefixed element");
    assert_eq!(mutate::insert_child(&mut doc, pr, ne), Ok(()));
    (doc, docn, pr, ne)
}

#[test]
fn new_chardata_copies_its_text() {
    let mut doc = doc_new();
    let tx = mutate::new_chardata(&mut doc, NodeType::Text, b"hi").expect("a text node");
    assert_eq!(doc.node(tx).value.len, 2);
}

#[test]
fn inserting_a_subtree_resolves_its_prefixes_against_the_new_context() {
    let (mut doc, _docn, pr) = connected_root();
    let ne = mutate::new_element(&mut doc, b"p:c").expect("a prefixed element");
    assert_eq!(mutate::insert_child(&mut doc, pr, ne), Ok(()));
    assert_eq!(doc.first_child(pr), Some(ne));
    assert_eq!(doc.parent(ne), Some(pr));
    assert_eq!(doc.ns(ne), b"urn:p", "the prefix resolved on insertion");

    let tx = mutate::new_chardata(&mut doc, NodeType::Text, b"hi").expect("a text node");
    assert_eq!(mutate::insert_child(&mut doc, ne, tx), Ok(()));
    assert_eq!(doc.first_child(ne), Some(tx));
}

#[test]
fn an_unbound_prefix_in_the_live_tree_is_refused_and_changes_nothing() {
    let (mut doc, _docn, pr, ne) = connected_tree();
    let ub = mutate::new_element(&mut doc, b"z:c").expect("an element with an unbound prefix");
    assert_eq!(
        mutate::insert_child(&mut doc, pr, ub),
        Err(MutStatus::UnboundNs),
        "connected, so an unbound prefix is an error rather than deferred"
    );
    assert!(doc.parent(ub).is_none(), "the refused node stayed detached");
    assert_eq!(
        doc.last_child(pr),
        Some(ne),
        "and the container is untouched"
    );
}

#[test]
fn resolution_is_deferred_until_the_subtree_joins_the_document() {
    let (mut doc, _docn, pr) = connected_root();
    let wrap = mutate::new_element(&mut doc, b"p:wrap").expect("an outer element");
    let inner = mutate::new_element(&mut doc, b"p:inner").expect("an inner element");

    assert_eq!(mutate::insert_child(&mut doc, wrap, inner), Ok(()));
    assert_eq!(
        doc.node(inner).ns_uri.len,
        0,
        "still detached, so nothing was resolved"
    );

    assert_eq!(mutate::insert_child(&mut doc, pr, wrap), Ok(()));
    assert_eq!(doc.ns(wrap), b"urn:p");
    assert_eq!(
        doc.ns(inner),
        b"urn:p",
        "joining the document resolved the whole subtree, not just its root"
    );
}

#[test]
fn inserting_an_ancestor_into_its_own_descendant_is_a_cycle() {
    let (mut doc, _docn, pr, ne) = connected_tree();
    assert_eq!(
        mutate::insert_child(&mut doc, ne, pr),
        Err(MutStatus::Cycle)
    );
}

#[test]
fn insert_before_and_after_place_a_sibling_on_the_right_side() {
    let (mut doc, _docn, pr, ne) = connected_tree();
    let b1 = mutate::new_element(&mut doc, b"b1").expect("a preceding sibling");
    let b2 = mutate::new_element(&mut doc, b"b2").expect("a following sibling");

    assert_eq!(mutate::insert_before(&mut doc, ne, b1), Ok(()));
    assert_eq!(doc.first_child(pr), Some(b1));
    assert_eq!(doc.next(b1), Some(ne));

    assert_eq!(mutate::insert_after(&mut doc, ne, b2), Ok(()));
    assert_eq!(doc.next(ne), Some(b2));
}

#[test]
fn inserting_a_node_next_to_itself_is_a_no_op_not_a_self_loop() {
    let (mut doc, _docn, _pr, ne) = connected_tree();
    assert_eq!(mutate::insert_before(&mut doc, ne, ne), Ok(()));
    assert_ne!(doc.next(ne), Some(ne), "no forward self-link");
    assert_ne!(doc.prev(ne), Some(ne), "no backward self-link");
    assert_eq!(mutate::insert_after(&mut doc, ne, ne), Ok(()));
    assert_ne!(doc.next(ne), Some(ne));
}

#[test]
fn replace_node_swaps_one_child_for_another() {
    let (mut doc, _docn, pr, ne) = connected_tree();
    let b1 = mutate::new_element(&mut doc, b"b1").expect("a preceding sibling");
    assert_eq!(mutate::insert_before(&mut doc, ne, b1), Ok(()));

    let rep = mutate::new_element(&mut doc, b"rep").expect("a replacement");
    assert_eq!(mutate::replace_node(&mut doc, ne, rep), Ok(()));
    assert!(doc.parent(ne).is_none(), "the replaced node is detached");
    assert_eq!(doc.parent(rep), Some(pr));
    assert_eq!(doc.next(b1), Some(rep), "in the slot it vacated");
}

#[test]
fn import_subtree_deep_copies_into_another_document() {
    let (doc, _docn, pr, _ne) = connected_tree();
    let mut doc2 = doc_new();
    let imp = mutate::import_subtree(&mut doc2, &doc, pr).expect("an imported copy");
    assert_eq!(doc2.qname(imp), b"pr");
    assert!(
        doc2.first_child(imp).is_some(),
        "deep, so the children came too"
    );
}

#[test]
fn a_document_takes_only_one_root_element() {
    let (mut doc, docn, _pr) = connected_root();
    let root2 = mutate::new_element(&mut doc, b"root2").expect("a second root");
    assert_eq!(
        mutate::insert_child(&mut doc, docn, root2),
        Err(MutStatus::Hierarchy)
    );
}

#[test]
fn node_id_tokens_fail_closed_outside_their_document() {
    // A node-set token is not authenticated, so the checked accessor must
    // reject anything that does not name a live slot in THIS document: a
    // foreign document's handle (same index, different stamp), an out-of-range
    // index, and the null handle.
    let mut a = Document::create(None, 0).expect("doc a");
    let mut b = Document::create(None, 0).expect("doc b");
    let na = a.new_node(NodeType::Element).expect("node a");
    let nb = b.new_node(NodeType::Element).expect("node b");

    // The handle resolves in its own document.
    assert_eq!(a.try_node(na).map(|n| n.type_), Some(NodeType::Element));
    assert_eq!(b.try_node(nb).map(|n| n.type_), Some(NodeType::Element));

    // Same slot index, different document stamp -> rejected.
    assert_eq!(na.index(), nb.index());
    assert!(b.try_node(na).is_none());
    assert!(a.try_node(nb).is_none());

    // Out-of-range index -> rejected, not a panic.
    let oob = NodeId::new(u32::MAX - 1, na.stamp());
    assert!(a.try_node(oob).is_none());

    // The null handle -> rejected.
    assert!(a.try_node(NodeId::INVALID).is_none());
    assert!(NodeId::INVALID.is_invalid());
}

/// A node spliced next to ITSELF is a sibling ring, and the engine follows
/// `next` without a bound everywhere - so the first traversal afterwards would
/// hang the host with no way out. `splice_between` refuses instead.
///
/// This is the shape a real bug reached: `a.add_next_sibling(b)` with b already
/// after a, where the insertion anchored on b, detached b, and then spliced b
/// before what was now itself.
#[test]
#[should_panic(expected = "cannot be its own parent or sibling")]
fn a_node_cannot_be_spliced_next_to_itself() {
    let (mut doc, r) = detached_element(b"r");
    let a = doc.new_node(NodeType::Element).expect("a child");
    doc.splice_between(r, a, None, Some(a));
}

#[test]
#[should_panic(expected = "cannot be its own parent or sibling")]
fn a_node_cannot_be_spliced_after_itself() {
    let (mut doc, r) = detached_element(b"r");
    let a = doc.new_node(NodeType::Element).expect("a child");
    doc.splice_between(r, a, Some(a), None);
}

#[test]
#[should_panic(expected = "cannot be its own parent or sibling")]
fn a_node_cannot_be_its_own_parent() {
    let (mut doc, r) = detached_element(b"r");
    doc.splice_between(r, r, None, None);
}

#[test]
#[should_panic(expected = "cannot be its own parent or sibling")]
fn an_attribute_cannot_be_linked_after_itself() {
    let (mut doc, r) = detached_element(b"r");
    let at = doc.new_node(NodeType::Attribute).expect("an attribute");
    doc.link_attr(r, Some(at), at);
}
