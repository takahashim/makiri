//! The arena, tree and mutation self-checks, run by `cargo test`.
//!
//! They were the C self-tests (mkr_xml_node_selftest / mkr_xml_parse_selftest /
//! mkr_xml_mutate_selftest), ported check-for-check, and reach edge and
//! overflow states no public API can construct. Each returns 0, or the number
//! of the check that failed. The tree is an index arena, so the checks address
//! nodes by `NodeId` through the `Document`.

#![forbid(unsafe_code)]

use crate::xml::mutate;
use crate::xml::qname;
use crate::xml::tree::{parse_ex, parse_fragment};
use crate::xml::{
    Document, Link, MutStatus, NodeId, NodeType, Status, MAX_BYTES, XMLNS_NS_URI, XML_NS_URI,
};

fn name_is(d: &Document, n: NodeId, s: &[u8]) -> bool {
    !n.is_invalid() && d.local(n) == s
}
fn val_is(d: &Document, n: NodeId, s: &[u8]) -> bool {
    !n.is_invalid() && d.value(n) == s
}
fn ns_is(d: &Document, n: NodeId, s: &[u8]) -> bool {
    !n.is_invalid() && d.ns(n) == s
}
fn pfx_is(d: &Document, n: NodeId, s: &[u8]) -> bool {
    !n.is_invalid() && d.prefix(n) == s
}
fn ns_none(d: &Document, n: NodeId) -> bool {
    !n.is_invalid() && d.node(n).ns_uri.len == 0
}
fn next(d: &Document, n: NodeId) -> Option<NodeId> {
    if n.is_invalid() {
        None
    } else {
        d.next(n)
    }
}
fn first(d: &Document, n: NodeId) -> Option<NodeId> {
    if n.is_invalid() {
        None
    } else {
        d.first_child(n)
    }
}

fn doc_new() -> Option<Box<Document>> {
    Document::create(None, 0).ok()
}

fn parse_lit(s: &[u8], st: &mut Status) -> Option<Box<Document>> {
    match parse_ex(s, None) {
        Ok(d) => {
            *st = Status::Ok;
            Some(d)
        }
        Err(e) => {
            *st = e;
            None
        }
    }
}

/// Parse the first `len` bytes of `src`, checking `len` against the byte
/// budget BEFORE the bytes are touched - the C entry's guard, which a caller
/// can reach with a length longer than the buffer it holds.
fn parse_ex_len(src: &[u8], len: usize, limits: Option<usize>) -> Result<Box<Document>, Status> {
    let max = limits.filter(|&n| n != 0).unwrap_or(MAX_BYTES);
    if len > max {
        return Err(Status::Limit);
    }
    parse_ex(&src[..len], limits)
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

/// `s` must be rejected with status `want`.
fn rejects(s: &[u8], want: Status) -> bool {
    let mut st = Status::Ok;
    parse_lit(s, &mut st).is_none() && st == want
}

/* ---- mkr_xml_node_selftest ---- */

fn node_selftest() -> i32 {
    node_selftest_impl()
}

fn node_selftest_impl() -> i32 {
    let mut idx = 0;
    let doc = Document::create(None, 0);
    idx += 1; /* 1 */
    let Ok(mut doc) = doc else {
        return idx;
    };

    idx += 1; /* 2: node zero-init + byte copy into the store */
    let root = doc.new_node(NodeType::Element);
    let local = doc.store(b"Feed");
    if root.is_err()
        || local.is_err()
        || !doc
            .node(root.as_ref().copied().unwrap_or(NodeId::INVALID))
            .first_child
            .is_none()
        || doc.type_(root.as_ref().copied().unwrap_or(NodeId::INVALID)) != Some(NodeType::Element)
        || doc.span(local.as_ref().copied().unwrap_or(crate::xml::Span::EMPTY)) != b"Feed"
    {
        return idx;
    }
    let root = root.unwrap();

    idx += 1; /* 3: an unset name field is the ABSENT marker, not a stray span */
    if !doc.node(root).qname.is_absent() {
        return idx;
    }

    idx += 1; /* 4: build 1000 children */
    for _ in 0..1000 {
        let Ok(c) = doc.new_node(NodeType::Element) else {
            return idx;
        };
        doc.append_child(root, c);
    }
    let mut cnt = 0;
    let mut c = doc.first_child(root);
    while let Some(id) = c {
        cnt += 1;
        c = doc.next(id);
    }
    if cnt != 1000 || doc.status != Status::Ok {
        return idx;
    }

    idx += 1; /* 5: pathological size fails closed */
    {
        let mut doc = Document::create(None, 0).unwrap();
        doc.max_bytes = doc.arena_bytes; /* no room left for another node */
        if doc.new_node(NodeType::Element).is_ok() || doc.status != Status::Limit {
            return idx;
        }
    }

    idx += 1; /* 6: byte budget enforced inside the allocator */
    {
        let mut doc = Document::create(None, 0).unwrap();
        doc.max_bytes = 4096;
        let mut hit = false;
        for _ in 0..100000 {
            if doc.new_node(NodeType::Element).is_err() {
                hit = doc.status == Status::Limit;
                break;
            }
        }
        if !hit {
            return idx;
        }
    }

    idx += 1; /* 7: node budget enforced */
    {
        let mut doc = Document::create(None, 0).unwrap();
        doc.max_nodes = 10;
        let mut nlimit = false;
        for _ in 0..100 {
            if doc.new_node(NodeType::Element).is_err() {
                nlimit = doc.status == Status::Limit;
                break;
            }
        }
        if !nlimit {
            return idx;
        }
    }

    0
}

/* ---- mkr_xml_parse_selftest ---- */

fn parse_selftest() -> i32 {
    parse_selftest_impl()
}

fn parse_selftest_impl() -> i32 {
    let mut st = Status::Ok;
    let mut i = 0;

    i += 1; /* 1 */
    let d = parse_lit(b"<Feed x='1' y='two'>hi<b/>z</Feed>", &mut st);
    let Some(d) = d.filter(|_| st == Status::Ok) else {
        return i;
    };
    i += 1; /* 2 */
    let doc = &*d;
    let root = doc.root().unwrap_or(NodeId::INVALID);
    if !name_is(doc, root, b"Feed")
        || doc.type_(root) != Some(NodeType::Element)
        || doc.node(root).line != 1
    {
        return i;
    }
    i += 1; /* 3 */
    let a0 = doc.attrs(root).unwrap_or(NodeId::INVALID);
    let a1 = next(doc, a0).unwrap_or(NodeId::INVALID);
    if a0.is_invalid()
        || doc.type_(a0) != Some(NodeType::Attribute)
        || !name_is(doc, a0, b"x")
        || !val_is(doc, a0, b"1")
        || a1.is_invalid()
        || !name_is(doc, a1, b"y")
        || !val_is(doc, a1, b"two")
        || doc.next(a1).is_some()
    {
        return i;
    }
    i += 1; /* 4 */
    let c0 = doc.first_child(root).unwrap_or(NodeId::INVALID);
    let c1 = next(doc, c0).unwrap_or(NodeId::INVALID);
    let c2 = next(doc, c1).unwrap_or(NodeId::INVALID);
    if c0.is_invalid()
        || doc.type_(c0) != Some(NodeType::Text)
        || !val_is(doc, c0, b"hi")
        || c1.is_invalid()
        || doc.type_(c1) != Some(NodeType::Element)
        || !name_is(doc, c1, b"b")
        || doc.first_child(c1).is_some()
        || c2.is_invalid()
        || doc.type_(c2) != Some(NodeType::Text)
        || !val_is(doc, c2, b"z")
        || doc.next(c2).is_some()
    {
        return i;
    }
    i += 1; /* 5 */
    if doc.parent(c1) != Some(root) || doc.prev(c1) != Some(c0) || doc.prev(c2) != Some(c1) {
        return i;
    }

    i += 1; /* 6: case sensitivity */
    let d = parse_lit(b"<X><x/></X>", &mut st);
    let Some(d) = d else {
        return i;
    };
    let doc = &*d;
    let root = doc.root().unwrap_or(NodeId::INVALID);
    let f = doc.first_child(root).unwrap_or(NodeId::INVALID);
    if !name_is(doc, root, b"X") || !name_is(doc, f, b"x") {
        return i;
    }

    i += 1; /* 7: well-formedness errors fail closed */
    for s in [
        &b"<a>"[..],
        b"<a></b>",
        b"<a/><b/>",
        b"x<a/>",
        b"<a x=>",
        b"<a y='<'>",
    ] {
        if !rejects(s, Status::Syntax) {
            return i;
        }
    }

    i += 1; /* 8: references expand */
    let d = parse_lit(
        b"<a x='p&amp;q' y='&#65;&#x42;'>1&lt;2&gt;3&amp;4&apos;5&quot;6</a>",
        &mut st,
    );
    let Some(d) = d.filter(|_| st == Status::Ok) else {
        return i;
    };
    {
        let doc = &*d;
        let r = doc.root().unwrap_or(NodeId::INVALID);
        let ax = doc.attrs(r).unwrap_or(NodeId::INVALID);
        let ay = next(doc, ax).unwrap_or(NodeId::INVALID);
        let tx = doc.first_child(r).unwrap_or(NodeId::INVALID);
        if !val_is(doc, ax, b"p&q")
            || !val_is(doc, ay, b"AB")
            || tx.is_invalid()
            || doc.type_(tx) != Some(NodeType::Text)
            || !val_is(doc, tx, b"1<2>3&4'5\"6")
        {
            return i;
        }
    }

    i += 1; /* 9: bad references fail closed */
    for s in [
        &b"<a>&nbsp;</a>"[..],
        b"<a>x & y</a>",
        b"<a>&#0;</a>",
        b"<a>&#xD800;</a>",
        b"<a>&#;</a>",
    ] {
        if !rejects(s, Status::Syntax) {
            return i;
        }
    }

    i += 1; /* 10: namespaces */
    let d = parse_lit(
        b"<a:e xmlns:a='urn:a' xmlns='urn:d' a:x='1' y='2'><c/></a:e>",
        &mut st,
    );
    let Some(d) = d.filter(|_| st == Status::Ok) else {
        return i;
    };
    {
        let doc = &*d;
        let r = doc.root().unwrap_or(NodeId::INVALID);
        if !name_is(doc, r, b"e") || !pfx_is(doc, r, b"a") || !ns_is(doc, r, b"urn:a") {
            return i;
        }
        let c = doc.first_child(r).unwrap_or(NodeId::INVALID);
        if !name_is(doc, c, b"c") || !ns_is(doc, c, b"urn:d") {
            return i;
        }
        let a = doc.attrs(r).unwrap_or(NodeId::INVALID);
        if a.is_invalid()
            || !name_is(doc, a, b"a")
            || !pfx_is(doc, a, b"xmlns")
            || !ns_is(doc, a, XMLNS_NS_URI)
        {
            return i;
        }
        let a = next(doc, a).unwrap_or(NodeId::INVALID);
        if a.is_invalid() || !name_is(doc, a, b"xmlns") || doc.node(a).prefix.len != 0 {
            return i;
        }
        let a = next(doc, a).unwrap_or(NodeId::INVALID);
        if a.is_invalid()
            || !name_is(doc, a, b"x")
            || !pfx_is(doc, a, b"a")
            || !ns_is(doc, a, b"urn:a")
            || !val_is(doc, a, b"1")
        {
            return i;
        }
        let a = next(doc, a).unwrap_or(NodeId::INVALID);
        if a.is_invalid()
            || !name_is(doc, a, b"y")
            || doc.node(a).prefix.len != 0
            || !ns_none(doc, a)
            || !val_is(doc, a, b"2")
        {
            return i;
        }
    }

    i += 1; /* 11: namespace errors fail closed */
    for s in [
        &b"<a:b/>"[..],
        b"<a x:y='1'/>",
        b"<a xmlns:xml='wrong'/>",
        b"<a:b xmlns:a=''/>",
    ] {
        if !rejects(s, Status::Syntax) {
            return i;
        }
    }

    i += 1; /* 12: attribute-value normalization */
    let d = parse_lit(b"<a x=\"p\tq\nr\" y=\"p&#9;q&#10;r\">u\tv\nw</a>", &mut st);
    let Some(d) = d.filter(|_| st == Status::Ok) else {
        return i;
    };
    {
        let doc = &*d;
        let r = doc.root().unwrap_or(NodeId::INVALID);
        let ax = doc.attrs(r).unwrap_or(NodeId::INVALID);
        let ay = next(doc, ax).unwrap_or(NodeId::INVALID);
        let tx = doc.first_child(r).unwrap_or(NodeId::INVALID);
        if !val_is(doc, ax, b"p q r")
            || !val_is(doc, ay, b"p\tq\nr")
            || tx.is_invalid()
            || doc.type_(tx) != Some(NodeType::Text)
            || !val_is(doc, tx, b"u\tv\nw")
        {
            return i;
        }
    }

    i += 1; /* 13: comment / CDATA / PI nodes; prolog PI + comment retained */
    let d = parse_lit(
        b"<?xml version=\"1.0\"?><?xml-stylesheet href=\"x\"?><!--top--><r><!--c--><![CDATA[a<b]]><?pi dat?></r><?tail t?>",
        &mut st,
    );
    let Some(d) = d.filter(|_| st == Status::Ok) else {
        return i;
    };
    {
        let doc = &*d;
        let r = doc.root().unwrap_or(NodeId::INVALID);
        if !name_is(doc, r, b"r") {
            return i;
        }
        let cm = doc.first_child(r).unwrap_or(NodeId::INVALID);
        let cd = next(doc, cm).unwrap_or(NodeId::INVALID);
        let pi = next(doc, cd).unwrap_or(NodeId::INVALID);
        if cm.is_invalid()
            || doc.type_(cm) != Some(NodeType::Comment)
            || !val_is(doc, cm, b"c")
            || cd.is_invalid()
            || doc.type_(cd) != Some(NodeType::CData)
            || !val_is(doc, cd, b"a<b")
            || pi.is_invalid()
            || doc.type_(pi) != Some(NodeType::Pi)
            || !name_is(doc, pi, b"pi")
            || !val_is(doc, pi, b"dat")
            || doc.next(pi).is_some()
        {
            return i;
        }
        let dn = doc.doc_node();
        let p1 = first(doc, dn).unwrap_or(NodeId::INVALID);
        let p2 = next(doc, p1).unwrap_or(NodeId::INVALID);
        let p3 = next(doc, p2).unwrap_or(NodeId::INVALID);
        let p4 = next(doc, p3).unwrap_or(NodeId::INVALID);
        if p1.is_invalid()
            || doc.type_(p1) != Some(NodeType::Pi)
            || !name_is(doc, p1, b"xml-stylesheet")
            || p2.is_invalid()
            || doc.type_(p2) != Some(NodeType::Comment)
            || !val_is(doc, p2, b"top")
            || p3 != r
            || doc.parent(r) != Some(dn)
            || p4.is_invalid()
            || doc.type_(p4) != Some(NodeType::Pi)
            || !name_is(doc, p4, b"tail")
            || doc.next(p4).is_some()
        {
            return i;
        }
    }

    i += 1; /* 14: §9 fail-closed cases */
    for s in [
        &b"<r/><!DOCTYPE r>"[..],
        b"<r><!-- a--b --></r>",
        b"<r><!-- c </r>",
        b" <?xml version=\"1.0\"?><r/>",
        b"<![CDATA[x]]><r/>",
        b"<r><?a:b x?></r>",              /* NS §7: PI target is an NCName */
        b"<!DOCTYPE r [ <!BOGUS> ]><r/>", /* §5.1: the subset is checked */
        b"<!DOCTYPE r [ <!ENTITY e \"%p;\"> ]><r/>", /* WFC: PEs in Internal Subset */
    ] {
        if !rejects(s, Status::Syntax) {
            return i;
        }
    }
    /* Well-formed, but it declares what Makiri would have to apply. */
    for s in [
        &b"<!DOCTYPE r [ <!ENTITY x \"y\"> ]><r>&x;</r>"[..],
        b"<!DOCTYPE r [ <!ATTLIST r k CDATA \"d\"> ]><r/>",
        b"<!DOCTYPE r [ <!ATTLIST r k ID #IMPLIED> ]><r/>",
        b"<!DOCTYPE r [ <!ENTITY % p \"x\"> %p; ]><r/>",
    ] {
        if !rejects(s, Status::Unsupported) {
            return i;
        }
    }

    i += 1; /* 14b: DOCTYPE recognized, not processed */
    let d = parse_lit(
        b"<!DOCTYPE r SYSTEM \"a>b\" [ <!ELEMENT r (#PCDATA)> ]><r>ok</r>",
        &mut st,
    );
    let Some(d) = d.filter(|_| st == Status::Ok) else {
        return i;
    };
    {
        let doc = &*d;
        let r = doc.root().unwrap_or(NodeId::INVALID);
        if !name_is(doc, r, b"r") || !val_is(doc, first(doc, r).unwrap_or(NodeId::INVALID), b"ok") {
            return i;
        }
        let dt = doc.doctype().unwrap_or(NodeId::INVALID);
        if dt.is_invalid()
            || doc.type_(dt) != Some(NodeType::Doctype)
            || doc.parent(dt) != Some(doc.doc_node())
            || first(doc, doc.doc_node()) != Some(dt)
            || doc.prev(dt).is_some()
            || doc.next(dt) != Some(r)
            || doc.local(dt) != b"r"
            || !doc.node(dt).prefix.is_absent()
            || doc.value(dt) != b"a>b"
        {
            return i;
        }
    }

    i += 1; /* 15: line-ending normalization */
    let d = parse_lit(b"<a x=\"p\r\nq\r\">m\r\nn\ro</a>", &mut st);
    let Some(d) = d.filter(|_| st == Status::Ok) else {
        return i;
    };
    {
        let doc = &*d;
        let r = doc.root().unwrap_or(NodeId::INVALID);
        let ax = doc.attrs(r).unwrap_or(NodeId::INVALID);
        let tx = doc.first_child(r).unwrap_or(NodeId::INVALID);
        if !val_is(doc, ax, b"p q ")
            || tx.is_invalid()
            || doc.type_(tx) != Some(NodeType::Text)
            || !val_is(doc, tx, b"m\nn\no")
        {
            return i;
        }
    }

    i += 1; /* 16: strict names + duplicate attributes + "]]>" */
    for s in [
        &b"<1bad/>"[..],
        b"<a:1b xmlns:a='u'/>",
        b"<a x='1' x='2'/>",
        b"<e xmlns:a='u' xmlns:b='u' a:x='1' b:x='2'/>",
        b"<a>foo]]>bar</a>",
    ] {
        if !rejects(s, Status::Syntax) {
            return i;
        }
    }
    let d = parse_lit(b"<a>1]2]]3</a>", &mut st);
    let Some(d) = d.filter(|_| st == Status::Ok) else {
        return i;
    };
    {
        let doc = &*d;
        let r = doc.root().unwrap_or(NodeId::INVALID);
        if !val_is(doc, first(doc, r).unwrap_or(NodeId::INVALID), b"1]2]]3") {
            return i;
        }
    }

    i += 1; /* 17: XML declaration grammar + reserved / colon PI targets */
    {
        let d = parse_lit(
            b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><r/>",
            &mut st,
        );
        if d.is_none() || st != Status::Ok {
            return i;
        }
        for s in [
            &b"<?xml VERSION=\"1.0\"?><r/>"[..],
            b"<?xml version=\"1.0\" standalone=\"YES\"?><r/>",
            b"<?xml encoding=\"UTF-8\"?><r/>",
            b"<?xml version=\"1.0\"encoding=\"UTF-8\"?><r/>",
            b"<?xml version=\"1.0\" version=\"1.0\"?><r/>",
            b"<?xml version=\"1.0\" valid=\"no\"?><r/>",
            b"<?xml version=\"1.0' ?><r/>",
            b"<?xml version=\"1.0^\"?><r/>",
            b"<?XML version=\"1.0\"?><r/>",
            b"<a x=\"1\"y=\"2\"/>",
            b"<a>&#X58;</a>",
        ] {
            if !rejects(s, Status::Syntax) {
                return i;
            }
        }
        /* §2.8: a 1.x label is read as 1.0; 2.0 is not a VersionNum at all. */
        if parse_ex(b"<?xml version=\"1.1\"?><r/>", None).is_err()
            || parse_ex(b"<?xml version=\"1.5\"?><r/>", None).is_err()
            || !rejects(b"<?xml version=\"2.0\"?><r/>", Status::Syntax)
        {
            return i;
        }
    }

    i += 1; /* 18: byte-budget entry guard (src not dereferenced) */
    {
        let tiny = b"<r/>";
        match parse_ex_len(tiny, MAX_BYTES + 1, None) {
            Err(Status::Limit) => {}
            Ok(_) => return i,
            Err(_) => return i,
        }
        let d = parse_lit(tiny, &mut st);
        if d.is_none() || st != Status::Ok {
            return i;
        }
    }

    i += 1; /* 19: per-parse override */
    {
        let src = b"<root><a/><b/><c/></root>";
        match parse_ex_len(src, src.len(), Some(2)) {
            Err(Status::Limit) => {}
            Ok(_) => return i,
            Err(_) => return i,
        }
        match parse_ex_len(src, src.len(), Some(64)) {
            Err(Status::Limit) => {}
            Ok(_) => return i,
            Err(_) => return i,
        }
        match parse_ex_len(src, src.len(), Some(1024 * 1024)) {
            Ok(d) => {
                if !name_is(&d, d.root().unwrap_or(NodeId::INVALID), b"root") {
                    return i;
                }
            }
            Err(_) => return i,
        }
        match parse_ex_len(src, src.len(), Some(0)) {
            Ok(_) => {}
            Err(_) => return i,
        }
    }

    i += 1; /* 20: fragment - multiple top-level nodes */
    {
        let fsrc = b"<a/>txt<p:b xmlns:p='urn:p'>x</p:b>";
        let fd = doc_new();
        let Some(mut fd) = fd else {
            return i;
        };
        let frag = match parse_fragment_checked(&mut fd, fsrc, false) {
            Ok(f) => f,
            Err(_) => {
                return i;
            }
        };
        let d = &fd;
        if d.type_(frag) != Some(NodeType::Fragment) {
            return i;
        }
        let c0 = d.first_child(frag).unwrap_or(NodeId::INVALID);
        let c1 = next(d, c0).unwrap_or(NodeId::INVALID);
        let c2 = next(d, c1).unwrap_or(NodeId::INVALID);
        if !name_is(d, c0, b"a")
            || d.type_(c0) != Some(NodeType::Element)
            || c1.is_invalid()
            || d.type_(c1) != Some(NodeType::Text)
            || !val_is(d, c1, b"txt")
            || !name_is(d, c2, b"b")
            || !ns_is(d, c2, b"urn:p")
            || d.next(c2).is_some()
        {
            return i;
        }
    }

    i += 1; /* 21: a fragment fails closed */
    {
        let fd = doc_new();
        let Some(mut fd) = fd else {
            return i;
        };
        for s in [
            &b"<?xml version='1.0'?>"[..],
            b"<!DOCTYPE r>",
            b"</x>",
            b"<a>",
            b"<p:a/>",
        ] {
            if parse_fragment_checked(&mut fd, s, false).is_ok() {
                return i;
            }
        }
    }

    i += 1; /* 22: inherit_doc_ns */
    {
        let fsrc = b"<p:a/><plain/>";
        let fd = parse_lit(b"<r xmlns:p='urn:p' xmlns='urn:d'/>", &mut st);
        let Some(mut fd) = fd.filter(|_| st == Status::Ok) else {
            return i;
        };
        let frag = parse_fragment_checked(&mut fd, fsrc, true).unwrap_or(NodeId::INVALID);
        let d = &fd;
        let a = first(d, frag).unwrap_or(NodeId::INVALID);
        let plain = next(d, a).unwrap_or(NodeId::INVALID);
        if frag.is_invalid() || !ns_is(d, a, b"urn:p") || !ns_is(d, plain, b"urn:d") {
            return i;
        }
    }

    0
}

/* ---- mkr_xml_mutate_selftest ---- */

fn mutate_selftest() -> i32 {
    mutate_selftest_impl()
}

fn mutate_selftest_impl() -> i32 {
    let Some(mut doc) = doc_new() else {
        return 1;
    };
    mutate_selftest_body(&mut doc)
}

fn mutate_selftest_body(doc: &mut Document) -> i32 {
    let invalid = NodeId::INVALID;

    /* 1. QName validation + split */
    match qname::split_checked(b"a:b") {
        Some(q) if q.prefix_len == 1 && q.local_len == 1 => {}
        _ => return 2,
    }
    if qname::split_checked(b"1bad").is_some() {
        return 3;
    }
    if qname::split_checked(b"a:b:c").is_some() {
        return 4;
    }
    if qname::split_checked(b":x").is_some() {
        return 5;
    }
    if qname::split_checked(b"x:").is_some() {
        return 6;
    }

    /* 2. root element, attribute set/replace */
    let r = match doc.new_node(NodeType::Element) {
        Ok(n) => n,
        Err(_) => return 7,
    };
    if doc.assign_qname(r, b"r", 0, 0, 1).is_err() {
        return 8;
    }
    let at = match mutate::set_attribute(doc, r, b"id", b"x") {
        Ok(a) => a,
        Err(_) => return 9,
    };
    if doc.node(at).value.len != 1 || doc.value(at) != b"x" || doc.attrs(r) != Some(at) {
        return 9;
    }
    let at = match mutate::set_attribute(doc, r, b"id", b"yy") {
        Ok(a) => a,
        Err(_) => return 10,
    };
    if doc.node(at).value.len != 2 || doc.next(at).is_some() {
        return 10;
    }

    /* 3. fail-closed: non-XML-Char value */
    if mutate::set_attribute(doc, r, b"k", b"\x01") != Err(MutStatus::BadChars) {
        return 11;
    }

    /* 3b. forbidden value sequences */
    if mutate::new_chardata(doc, NodeType::Comment, b"a--b") != Err(MutStatus::BadChars) {
        return 111;
    }
    if mutate::new_chardata(doc, NodeType::Comment, b"x-") != Err(MutStatus::BadChars) {
        return 112;
    }
    if mutate::new_chardata(doc, NodeType::CData, b"a]]>b") != Err(MutStatus::BadChars) {
        return 113;
    }
    let chk = match mutate::new_chardata(doc, NodeType::Comment, b"a-b") {
        Ok(n) => n,
        Err(_) => return 114,
    };
    if mutate::set_content(doc, chk, b"x--y") != MutStatus::BadChars {
        return 115;
    }
    if mutate::set_attribute(doc, r, b"xmlns:q", b"") != Err(MutStatus::BadNsDecl) {
        return 116;
    }
    if mutate::set_attribute(doc, r, b"xmlns", b"").is_err() {
        return 117;
    }

    let det = match mutate::new_element(doc, b"det") {
        Ok(n) => n,
        Err(_) => return 12,
    };
    let at = match mutate::set_attribute(doc, det, b"p:k", b"v") {
        Ok(a) => a,
        Err(_) => return 12,
    };
    if doc.node(at).ns_uri.len != 0 {
        return 12;
    }

    /* 4. the predefined xml: prefix */
    let at = match mutate::set_attribute(doc, r, b"xml:lang", b"en") {
        Ok(a) => a,
        Err(_) => return 13,
    };
    if doc.ns(at) != XML_NS_URI {
        return 13;
    }

    /* 5. xmlns:* declaration then a bound prefix */
    let at = match mutate::set_attribute(doc, r, b"xmlns:p", b"urn:p") {
        Ok(a) => a,
        Err(_) => return 14,
    };
    if doc.ns(at) != XMLNS_NS_URI {
        return 14;
    }
    let at = match mutate::set_attribute(doc, r, b"p:k", b"v") {
        Ok(a) => a,
        Err(_) => return 15,
    };
    if doc.ns(at) != b"urn:p" {
        return 15;
    }

    /* 6. remove by name (idempotent) */
    if !mutate::remove_attribute(doc, r, b"id") {
        return 16;
    }
    if mutate::remove_attribute(doc, r, b"id") {
        return 17;
    }

    /* 7. rename */
    if mutate::rename(doc, r, b"q") != MutStatus::Ok
        || doc.node(r).qname.len != 1
        || doc.local(r) != b"q"
        || doc.node(r).ns_uri.len != 0
    {
        return 18;
    }

    /* 8. content */
    let c1 = match doc.new_node(NodeType::Element) {
        Ok(n) => n,
        Err(_) => return 19,
    };
    doc.set_parent(c1, Some(r));
    doc.node_mut(r).first_child = Link::of(c1);
    doc.node_mut(r).last_child = Link::of(c1);
    if mutate::set_content(doc, r, b"hi") != MutStatus::Ok {
        return 20;
    }
    let fc = doc.first_child(r).unwrap_or(invalid);
    if fc.is_invalid()
        || doc.type_(fc) != Some(NodeType::Text)
        || doc.node(fc).value.len != 2
        || doc.last_child(r) != Some(fc)
        || doc.parent(c1).is_some()
    {
        return 21;
    }
    if mutate::set_content(doc, r, b"") != MutStatus::Ok
        || doc.first_child(r).is_some()
        || doc.last_child(r).is_some()
    {
        return 22;
    }

    /* 9. detach */
    let a1 = match doc.new_node(NodeType::Element) {
        Ok(n) => n,
        Err(_) => return 23,
    };
    let a2 = match doc.new_node(NodeType::Element) {
        Ok(n) => n,
        Err(_) => return 23,
    };
    doc.set_parent(a1, Some(r));
    doc.set_parent(a2, Some(r));
    doc.node_mut(a1).next = Link::of(a2);
    doc.node_mut(a2).prev = Link::of(a1);
    doc.node_mut(r).first_child = Link::of(a1);
    doc.node_mut(r).last_child = Link::of(a2);
    mutate::detach(doc, a1);
    if doc.first_child(r) != Some(a2) || doc.prev(a2).is_some() || doc.parent(a1).is_some() {
        return 24;
    }
    mutate::detach(doc, a2);
    if doc.first_child(r).is_some() || doc.last_child(r).is_some() || doc.parent(a2).is_some() {
        return 25;
    }

    /* 10. a live document node + connected root */
    let docn = match doc.new_node(NodeType::Document) {
        Ok(n) => n,
        Err(_) => return 26,
    };
    doc.doc_node = docn;
    let pr = match mutate::new_element(doc, b"pr") {
        Ok(n) => n,
        Err(_) => return 27,
    };
    if doc.parent(pr).is_some() || doc.node(pr).ns_uri.len != 0 {
        return 27;
    }
    if mutate::set_attribute(doc, pr, b"xmlns:p", b"urn:p").is_err() {
        return 28;
    }
    if mutate::insert_child(doc, docn, pr) != MutStatus::Ok || doc.root() != Some(pr) {
        return 29;
    }
    let tx = match mutate::new_chardata(doc, NodeType::Text, b"hi") {
        Ok(n) => n,
        Err(_) => return 30,
    };
    if doc.node(tx).value.len != 2 {
        return 30;
    }

    /* 11. insert_child resolves the inserted subtree */
    let ne = match mutate::new_element(doc, b"p:c") {
        Ok(n) => n,
        Err(_) => return 31,
    };
    if mutate::insert_child(doc, pr, ne) != MutStatus::Ok
        || doc.first_child(pr) != Some(ne)
        || doc.parent(ne) != Some(pr)
        || doc.ns(ne) != b"urn:p"
    {
        return 32;
    }
    if mutate::insert_child(doc, ne, tx) != MutStatus::Ok || doc.first_child(ne) != Some(tx) {
        return 33;
    }

    /* 12. unbound prefix in the live tree */
    let ub = match mutate::new_element(doc, b"z:c") {
        Ok(n) => n,
        Err(_) => return 34,
    };
    if mutate::insert_child(doc, pr, ub) != MutStatus::UnboundNs
        || doc.parent(ub).is_some()
        || doc.last_child(pr) != Some(ne)
    {
        return 35;
    }

    /* 13. deferred resolution */
    let wrap = match mutate::new_element(doc, b"p:wrap") {
        Ok(n) => n,
        Err(_) => return 36,
    };
    let inner = match mutate::new_element(doc, b"p:inner") {
        Ok(n) => n,
        Err(_) => return 36,
    };
    if mutate::insert_child(doc, wrap, inner) != MutStatus::Ok || doc.node(inner).ns_uri.len != 0 {
        return 37;
    }
    if mutate::insert_child(doc, pr, wrap) != MutStatus::Ok
        || doc.ns(wrap) != b"urn:p"
        || doc.ns(inner) != b"urn:p"
    {
        return 38;
    }

    /* 14. cycle rejection */
    if mutate::insert_child(doc, ne, pr) != MutStatus::Cycle {
        return 39;
    }

    /* 15. sibling order */
    let b1 = match mutate::new_element(doc, b"b1") {
        Ok(n) => n,
        Err(_) => return 40,
    };
    let b2 = match mutate::new_element(doc, b"b2") {
        Ok(n) => n,
        Err(_) => return 40,
    };
    if mutate::insert_before(doc, ne, b1) != MutStatus::Ok
        || doc.first_child(pr) != Some(b1)
        || doc.next(b1) != Some(ne)
    {
        return 41;
    }
    if mutate::insert_after(doc, ne, b2) != MutStatus::Ok || doc.next(ne) != Some(b2) {
        return 42;
    }
    if mutate::insert_before(doc, ne, ne) != MutStatus::Ok
        || doc.next(ne) == Some(ne)
        || doc.prev(ne) == Some(ne)
        || mutate::insert_after(doc, ne, ne) != MutStatus::Ok
        || doc.next(ne) == Some(ne)
    {
        return 99;
    }

    /* 16. replace */
    let rep = match mutate::new_element(doc, b"rep") {
        Ok(n) => n,
        Err(_) => return 43,
    };
    if mutate::replace_node(doc, ne, rep) != MutStatus::Ok
        || doc.parent(ne).is_some()
        || doc.parent(rep) != Some(pr)
        || doc.next(b1) != Some(rep)
    {
        return 44;
    }

    /* 17. cross-document import */
    let doc2 = doc_new();
    let Some(mut doc2) = doc2 else {
        return 45;
    };
    let mut rc = 0;
    match mutate::import_subtree(&mut doc2, doc, pr) {
        Ok(imp) => {
            if doc2.qname(imp) != b"pr" || doc2.first_child(imp).is_none() {
                rc = 46;
            }
        }
        Err(_) => rc = 46,
    }
    if rc != 0 {
        return rc;
    }

    /* 18. single-root rule */
    let root2 = match mutate::new_element(doc, b"root2") {
        Ok(n) => n,
        Err(_) => return 47,
    };
    if mutate::insert_child(doc, docn, root2) != MutStatus::Hierarchy {
        return 48;
    }
    0
}

#[test]
fn node_checks_pass() {
    let rc = node_selftest();
    assert_eq!(rc, 0, "node self-check {rc} failed");
}

#[test]
fn parse_checks_pass() {
    let rc = parse_selftest();
    assert_eq!(rc, 0, "parse self-check {rc} failed");
}

#[test]
fn mutate_checks_pass() {
    let rc = mutate_selftest();
    assert_eq!(rc, 0, "mutate self-check {rc} failed");
}
