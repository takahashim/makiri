//! The three C self-tests (mkr_xml_node_selftest / mkr_xml_parse_selftest /
//! mkr_xml_mutate_selftest), ported check-for-check so Makiri.__c_selftest
//! exercises the Rust engine exactly as it did the C one. Test code: raw
//! pointer walks are expected here.

use crate::xml::arena::{arena_alloc, arena_bytes, arena_node, doc_destroy, doc_new};
use crate::xml::tree::{parse_ex_raw, parse_fragment_raw};
use crate::xml::{
    mutate, node_local, node_ns, node_prefix, node_value, qname, Doc, Node, QName, ERR_LIMIT,
    ERR_OOM, ERR_SYNTAX, ERR_VERSION, MAX_BYTES, MUT_BAD_CHARS, MUT_BAD_NS_DECL, MUT_CYCLE,
    MUT_HIERARCHY, MUT_OK, MUT_UNBOUND_NS, OK, T_ATTRIBUTE, T_CDATA, T_COMMENT, T_DOCTYPE,
    T_DOCUMENT, T_ELEMENT, T_FRAGMENT, T_PI, T_TEXT, XMLNS_NS_URI, XML_NS_URI,
};
use core::ffi::c_char;
use core::ptr;

unsafe fn name_is(n: *const Node, s: &[u8]) -> bool {
    !n.is_null() && node_local(n) == s
}
unsafe fn val_is(n: *const Node, s: &[u8]) -> bool {
    !n.is_null() && node_value(n) == s
}
unsafe fn ns_is(n: *const Node, s: &[u8]) -> bool {
    !n.is_null() && node_ns(n) == s
}
unsafe fn pfx_is(n: *const Node, s: &[u8]) -> bool {
    !n.is_null() && node_prefix(n) == s
}
unsafe fn ns_none(n: *const Node) -> bool {
    !n.is_null() && (*n).ns_uri_len == 0
}
unsafe fn next(n: *const Node) -> *mut Node {
    if n.is_null() {
        ptr::null_mut()
    } else {
        (*n).next
    }
}
unsafe fn first(n: *const Node) -> *mut Node {
    if n.is_null() {
        ptr::null_mut()
    } else {
        (*n).first_child
    }
}

unsafe fn parse_lit(s: &[u8], st: &mut i32) -> *mut Doc {
    match parse_ex_raw(s.as_ptr() as *const c_char, s.len(), None) {
        Ok(d) => {
            *st = OK;
            d
        }
        Err(e) => {
            *st = e;
            ptr::null_mut()
        }
    }
}

/// `s` must be rejected with status `want`.
unsafe fn rejects(s: &[u8], want: i32) -> bool {
    let mut st = OK;
    let e = parse_lit(s, &mut st);
    if !e.is_null() {
        doc_destroy(e);
        return false;
    }
    st == want
}

/* ---- mkr_xml_node_selftest ---- */

pub unsafe fn node_selftest() -> i32 {
    let mut idx = 0;
    let doc = doc_new();
    idx += 1; /* 1 */
    if doc.is_null() {
        return idx;
    }

    idx += 1; /* 2: node zero-init + name copy */
    let root = arena_node(doc, T_ELEMENT);
    let nm = b"Feed";
    let local = arena_bytes(doc, nm);
    if root.is_null()
        || !(*root).first_child.is_null()
        || (*root).type_ != T_ELEMENT
        || local.is_null()
        || local as *const u8 == nm.as_ptr()
        || crate::xml::bytes(local, 4) != b"Feed"
    {
        doc_destroy(doc);
        return idx;
    }
    idx += 1; /* 3: pointer alignment */
    if (root as usize) % 16 != 0 {
        doc_destroy(doc);
        return idx;
    }

    idx += 1; /* 4: build 1000 children */
    for _ in 0..1000 {
        let c = arena_node(doc, T_ELEMENT);
        if c.is_null() {
            doc_destroy(doc);
            return idx;
        }
        crate::xml::arena::append_child(root, c);
    }
    let mut cnt = 0;
    let mut c = (*root).first_child;
    while !c.is_null() {
        cnt += 1;
        c = (*c).next;
    }
    if cnt != 1000 || (*doc).oom != OK {
        doc_destroy(doc);
        return idx;
    }
    doc_destroy(doc);

    idx += 1; /* 5: pathological size fails closed */
    let doc = doc_new();
    if doc.is_null() {
        return idx;
    }
    if !arena_alloc(doc, usize::MAX - 8).is_null() || (*doc).oom == OK {
        doc_destroy(doc);
        return idx;
    }
    doc_destroy(doc);

    idx += 1; /* 6: byte budget enforced inside the allocator */
    let doc = doc_new();
    if doc.is_null() {
        return idx;
    }
    (*doc).max_bytes = 4096;
    let mut hit = false;
    for _ in 0..100000 {
        if arena_node(doc, T_ELEMENT).is_null() {
            hit = (*doc).oom == ERR_LIMIT;
            break;
        }
    }
    if !hit {
        doc_destroy(doc);
        return idx;
    }
    doc_destroy(doc);

    idx += 1; /* 7: node budget enforced */
    let doc = doc_new();
    if doc.is_null() {
        return idx;
    }
    (*doc).max_nodes = 10;
    let mut nlimit = false;
    for _ in 0..100 {
        if arena_node(doc, T_ELEMENT).is_null() {
            nlimit = (*doc).oom == ERR_LIMIT;
            break;
        }
    }
    if !nlimit {
        doc_destroy(doc);
        return idx;
    }
    doc_destroy(doc);

    idx += 1; /* 8: fail-closed on a NULL document */
    if !arena_node(ptr::null_mut(), T_ELEMENT).is_null()
        || !arena_bytes(ptr::null_mut(), b"x").is_null()
        || crate::xml::arena::arena_spanbuf(ptr::null_mut(), 1).ok
    {
        return idx;
    }
    0
}

/* ---- mkr_xml_parse_selftest ---- */

pub unsafe fn parse_selftest() -> i32 {
    let mut st = OK;
    let mut i = 0;

    i += 1; /* 1 */
    let d = parse_lit(b"<Feed x='1' y='two'>hi<b/>z</Feed>", &mut st);
    if d.is_null() || st != OK {
        if !d.is_null() {
            doc_destroy(d);
        }
        return i;
    }
    i += 1; /* 2 */
    let root = (*d).root;
    if !name_is(root, b"Feed") || (*root).type_ != T_ELEMENT || (*root).line != 1 || (*root).col != 1 {
        doc_destroy(d);
        return i;
    }
    i += 1; /* 3 */
    let a0 = (*root).attrs;
    let a1 = next(a0);
    if a0.is_null()
        || (*a0).type_ != T_ATTRIBUTE
        || !name_is(a0, b"x")
        || !val_is(a0, b"1")
        || a1.is_null()
        || !name_is(a1, b"y")
        || !val_is(a1, b"two")
        || !(*a1).next.is_null()
    {
        doc_destroy(d);
        return i;
    }
    i += 1; /* 4 */
    let c0 = (*root).first_child;
    let c1 = next(c0);
    let c2 = next(c1);
    if c0.is_null()
        || (*c0).type_ != T_TEXT
        || !val_is(c0, b"hi")
        || c1.is_null()
        || (*c1).type_ != T_ELEMENT
        || !name_is(c1, b"b")
        || !(*c1).first_child.is_null()
        || c2.is_null()
        || (*c2).type_ != T_TEXT
        || !val_is(c2, b"z")
        || !(*c2).next.is_null()
    {
        doc_destroy(d);
        return i;
    }
    i += 1; /* 5 */
    if (*c1).parent != root || (*c1).prev != c0 || (*c2).prev != c1 {
        doc_destroy(d);
        return i;
    }
    doc_destroy(d);

    i += 1; /* 6: case sensitivity */
    let d = parse_lit(b"<X><x/></X>", &mut st);
    if d.is_null() || !name_is((*d).root, b"X") || !name_is(first((*d).root), b"x") {
        if !d.is_null() {
            doc_destroy(d);
        }
        return i;
    }
    doc_destroy(d);

    i += 1; /* 7: well-formedness errors fail closed */
    for s in [
        &b"<a>"[..],
        b"<a></b>",
        b"<a/><b/>",
        b"x<a/>",
        b"<a x=>",
        b"<a y='<'>",
    ] {
        if !rejects(s, ERR_SYNTAX) {
            return i;
        }
    }

    i += 1; /* 8: references expand */
    let d = parse_lit(
        b"<a x='p&amp;q' y='&#65;&#x42;'>1&lt;2&gt;3&amp;4&apos;5&quot;6</a>",
        &mut st,
    );
    if d.is_null() || st != OK {
        if !d.is_null() {
            doc_destroy(d);
        }
        return i;
    }
    {
        let r = (*d).root;
        let ax = (*r).attrs;
        let ay = next(ax);
        let tx = (*r).first_child;
        if !val_is(ax, b"p&q")
            || !val_is(ay, b"AB")
            || tx.is_null()
            || (*tx).type_ != T_TEXT
            || !val_is(tx, b"1<2>3&4'5\"6")
        {
            doc_destroy(d);
            return i;
        }
    }
    doc_destroy(d);

    i += 1; /* 9: bad references fail closed */
    for s in [
        &b"<a>&nbsp;</a>"[..],
        b"<a>x & y</a>",
        b"<a>&#0;</a>",
        b"<a>&#xD800;</a>",
        b"<a>&#;</a>",
    ] {
        if !rejects(s, ERR_SYNTAX) {
            return i;
        }
    }

    i += 1; /* 10: namespaces */
    let d = parse_lit(b"<a:e xmlns:a='urn:a' xmlns='urn:d' a:x='1' y='2'><c/></a:e>", &mut st);
    if d.is_null() || st != OK {
        if !d.is_null() {
            doc_destroy(d);
        }
        return i;
    }
    {
        let r = (*d).root;
        if !name_is(r, b"e") || !pfx_is(r, b"a") || !ns_is(r, b"urn:a") {
            doc_destroy(d);
            return i;
        }
        let c = (*r).first_child;
        if !name_is(c, b"c") || !ns_is(c, b"urn:d") {
            doc_destroy(d);
            return i;
        }
        let a = (*r).attrs;
        if a.is_null() || !name_is(a, b"a") || !pfx_is(a, b"xmlns") || !ns_is(a, XMLNS_NS_URI) {
            doc_destroy(d);
            return i;
        }
        let a = next(a);
        if a.is_null() || !name_is(a, b"xmlns") || (*a).prefix_len != 0 {
            doc_destroy(d);
            return i;
        }
        let a = next(a);
        if a.is_null() || !name_is(a, b"x") || !pfx_is(a, b"a") || !ns_is(a, b"urn:a") || !val_is(a, b"1") {
            doc_destroy(d);
            return i;
        }
        let a = next(a);
        if a.is_null() || !name_is(a, b"y") || (*a).prefix_len != 0 || !ns_none(a) || !val_is(a, b"2") {
            doc_destroy(d);
            return i;
        }
    }
    doc_destroy(d);

    i += 1; /* 11: namespace errors fail closed */
    for s in [
        &b"<a:b/>"[..],
        b"<a x:y='1'/>",
        b"<a xmlns:xml='wrong'/>",
        b"<a:b xmlns:a=''/>",
    ] {
        if !rejects(s, ERR_SYNTAX) {
            return i;
        }
    }

    i += 1; /* 12: attribute-value normalization */
    let d = parse_lit(b"<a x=\"p\tq\nr\" y=\"p&#9;q&#10;r\">u\tv\nw</a>", &mut st);
    if d.is_null() || st != OK {
        if !d.is_null() {
            doc_destroy(d);
        }
        return i;
    }
    {
        let r = (*d).root;
        let ax = (*r).attrs;
        let ay = next(ax);
        let tx = (*r).first_child;
        if !val_is(ax, b"p q r")
            || !val_is(ay, b"p\tq\nr")
            || tx.is_null()
            || (*tx).type_ != T_TEXT
            || !val_is(tx, b"u\tv\nw")
        {
            doc_destroy(d);
            return i;
        }
    }
    doc_destroy(d);

    i += 1; /* 13: comment / CDATA / PI nodes; prolog PI + comment retained */
    let d = parse_lit(
        b"<?xml version=\"1.0\"?><?xml-stylesheet href=\"x\"?><!--top--><r><!--c--><![CDATA[a<b]]><?pi dat?></r><?tail t?>",
        &mut st,
    );
    if d.is_null() || st != OK {
        if !d.is_null() {
            doc_destroy(d);
        }
        return i;
    }
    {
        let r = (*d).root;
        if !name_is(r, b"r") {
            doc_destroy(d);
            return i;
        }
        let cm = (*r).first_child;
        let cd = next(cm);
        let pi = next(cd);
        if cm.is_null()
            || (*cm).type_ != T_COMMENT
            || !val_is(cm, b"c")
            || cd.is_null()
            || (*cd).type_ != T_CDATA
            || !val_is(cd, b"a<b")
            || pi.is_null()
            || (*pi).type_ != T_PI
            || !name_is(pi, b"pi")
            || !val_is(pi, b"dat")
            || !(*pi).next.is_null()
        {
            doc_destroy(d);
            return i;
        }
        let dn = (*d).doc_node;
        let p1 = first(dn);
        let p2 = next(p1);
        let p3 = next(p2);
        let p4 = next(p3);
        if p1.is_null()
            || (*p1).type_ != T_PI
            || !name_is(p1, b"xml-stylesheet")
            || p2.is_null()
            || (*p2).type_ != T_COMMENT
            || !val_is(p2, b"top")
            || p3 != r
            || (*r).parent != dn
            || p4.is_null()
            || (*p4).type_ != T_PI
            || !name_is(p4, b"tail")
            || !(*p4).next.is_null()
        {
            doc_destroy(d);
            return i;
        }
    }
    doc_destroy(d);

    i += 1; /* 14: §9 fail-closed cases */
    for s in [
        &b"<r/><!DOCTYPE r>"[..],
        b"<!DOCTYPE r [ <!ENTITY x \"y\"> ]><r>&x;</r>",
        b"<r><!-- a--b --></r>",
        b"<r><!-- c </r>",
        b" <?xml version=\"1.0\"?><r/>",
        b"<![CDATA[x]]><r/>",
    ] {
        if !rejects(s, ERR_SYNTAX) {
            return i;
        }
    }

    i += 1; /* 14b: DOCTYPE recognized, not processed */
    let d = parse_lit(
        b"<!DOCTYPE r SYSTEM \"a>b\" [ <!ELEMENT r (#PCDATA)> ]><r>ok</r>",
        &mut st,
    );
    if d.is_null() || st != OK || !name_is((*d).root, b"r") || !val_is(first((*d).root), b"ok") {
        if !d.is_null() {
            doc_destroy(d);
        }
        return i;
    }
    {
        let dt = (*d).doctype;
        if dt.is_null()
            || (*dt).type_ != T_DOCTYPE
            || (*dt).parent != (*d).doc_node
            || (*(*d).doc_node).first_child != dt
            || !(*dt).prev.is_null()
            || (*dt).next != (*d).root
            || node_local(dt) != b"r"
            || !(*dt).prefix.is_null()
            || node_value(dt) != b"a>b"
        {
            doc_destroy(d);
            return i;
        }
    }
    doc_destroy(d);

    i += 1; /* 15: line-ending normalization */
    let d = parse_lit(b"<a x=\"p\r\nq\r\">m\r\nn\ro</a>", &mut st);
    if d.is_null() || st != OK {
        if !d.is_null() {
            doc_destroy(d);
        }
        return i;
    }
    {
        let r = (*d).root;
        let ax = (*r).attrs;
        let tx = (*r).first_child;
        if !val_is(ax, b"p q ") || tx.is_null() || (*tx).type_ != T_TEXT || !val_is(tx, b"m\nn\no") {
            doc_destroy(d);
            return i;
        }
    }
    doc_destroy(d);

    i += 1; /* 16: strict names + duplicate attributes + "]]>" */
    for s in [
        &b"<1bad/>"[..],
        b"<a:1b xmlns:a='u'/>",
        b"<a x='1' x='2'/>",
        b"<e xmlns:a='u' xmlns:b='u' a:x='1' b:x='2'/>",
        b"<a>foo]]>bar</a>",
    ] {
        if !rejects(s, ERR_SYNTAX) {
            return i;
        }
    }
    let d = parse_lit(b"<a>1]2]]3</a>", &mut st);
    if d.is_null() || st != OK || !val_is(first((*d).root), b"1]2]]3") {
        if !d.is_null() {
            doc_destroy(d);
        }
        return i;
    }
    doc_destroy(d);

    i += 1; /* 17: XML declaration grammar + reserved / colon PI targets */
    {
        let d = parse_lit(b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><r/>", &mut st);
        if d.is_null() || st != OK {
            if !d.is_null() {
                doc_destroy(d);
            }
            return i;
        }
        doc_destroy(d);
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
            /* NOTE: the C list also has "<r><?a:b data?></r>" ("colon in
             * PITarget"), but the C loop measures each entry with
             * sizeof(pointer) - 1 = 7 bytes, so it only ever tested truncated
             * strings. A colon IS permitted in a PITarget (§2.6, as the parser
             * comment says), and both engines accept it - so it is not listed
             * here. */
            b"<a x=\"1\"y=\"2\"/>",
            b"<a>&#X58;</a>",
        ] {
            if !rejects(s, ERR_SYNTAX) {
                return i;
            }
        }
        if !rejects(b"<?xml version=\"1.1\"?><r/>", ERR_VERSION)
            || !rejects(b"<?xml version=\"1.5\"?><r/>", ERR_VERSION)
            || !rejects(b"<?xml version=\"2.0\"?><r/>", ERR_SYNTAX)
        {
            return i;
        }
    }

    i += 1; /* 18: byte-budget entry guard (src not dereferenced) */
    {
        let tiny = b"<r/>";
        match parse_ex_raw(tiny.as_ptr() as *const c_char, MAX_BYTES + 1, None) {
            Err(e) if e == ERR_LIMIT => {}
            Ok(d) => {
                doc_destroy(d);
                return i;
            }
            Err(_) => return i,
        }
        let d = parse_lit(tiny, &mut st);
        if d.is_null() || st != OK {
            if !d.is_null() {
                doc_destroy(d);
            }
            return i;
        }
        doc_destroy(d);
    }

    i += 1; /* 19: per-parse override */
    {
        let src = b"<root><a/><b/><c/></root>";
        let p = src.as_ptr() as *const c_char;
        match parse_ex_raw(p, src.len(), Some(2)) {
            Err(e) if e == ERR_LIMIT => {}
            Ok(d) => {
                doc_destroy(d);
                return i;
            }
            Err(_) => return i,
        }
        match parse_ex_raw(p, src.len(), Some(64)) {
            Err(e) if e == ERR_LIMIT => {}
            Ok(d) => {
                doc_destroy(d);
                return i;
            }
            Err(_) => return i,
        }
        match parse_ex_raw(p, src.len(), Some(1024 * 1024)) {
            Ok(d) => {
                if !name_is((*d).root, b"root") {
                    doc_destroy(d);
                    return i;
                }
                doc_destroy(d);
            }
            Err(_) => return i,
        }
        match parse_ex_raw(p, src.len(), Some(0)) {
            Ok(d) => doc_destroy(d),
            Err(_) => return i,
        }
    }

    i += 1; /* 20: fragment - multiple top-level nodes */
    {
        let fsrc = b"<a/>txt<p:b xmlns:p='urn:p'>x</p:b>";
        let fd = doc_new();
        if fd.is_null() {
            return i;
        }
        let frag = match parse_fragment_raw(fd, fsrc.as_ptr() as *const c_char, fsrc.len(), false) {
            Ok(f) => f,
            Err(_) => {
                doc_destroy(fd);
                return i;
            }
        };
        if (*frag).type_ != T_FRAGMENT {
            doc_destroy(fd);
            return i;
        }
        let c0 = (*frag).first_child;
        let c1 = next(c0);
        let c2 = next(c1);
        if !name_is(c0, b"a")
            || (*c0).type_ != T_ELEMENT
            || c1.is_null()
            || (*c1).type_ != T_TEXT
            || !val_is(c1, b"txt")
            || !name_is(c2, b"b")
            || !ns_is(c2, b"urn:p")
            || !(*c2).next.is_null()
        {
            doc_destroy(fd);
            return i;
        }
        doc_destroy(fd);
    }

    i += 1; /* 21: a fragment fails closed */
    {
        let fd = doc_new();
        if fd.is_null() {
            return i;
        }
        for s in [
            &b"<?xml version='1.0'?>"[..],
            b"<!DOCTYPE r>",
            b"</x>",
            b"<a>",
            b"<p:a/>",
        ] {
            if parse_fragment_raw(fd, s.as_ptr() as *const c_char, s.len(), false).is_ok() {
                doc_destroy(fd);
                return i;
            }
        }
        doc_destroy(fd);
    }

    i += 1; /* 22: inherit_doc_ns */
    {
        let fsrc = b"<p:a/><plain/>";
        let fd = parse_lit(b"<r xmlns:p='urn:p' xmlns='urn:d'/>", &mut st);
        if fd.is_null() || st != OK {
            if !fd.is_null() {
                doc_destroy(fd);
            }
            return i;
        }
        let frag = parse_fragment_raw(fd, fsrc.as_ptr() as *const c_char, fsrc.len(), true).unwrap_or(ptr::null_mut());
        let a = first(frag);
        let plain = next(a);
        if frag.is_null() || !ns_is(a, b"urn:p") || !ns_is(plain, b"urn:d") {
            doc_destroy(fd);
            return i;
        }
        doc_destroy(fd);
    }

    0
}

/* ---- mkr_xml_mutate_selftest ---- */

unsafe fn split(name: &[u8]) -> Option<QName> {
    qname::split_checked(name).map(|sp| crate::xml::qname_from(name, &sp))
}

pub unsafe fn mutate_selftest() -> i32 {
    let doc = doc_new();
    if doc.is_null() {
        return 1;
    }
    let rc = mutate_selftest_body(doc);
    doc_destroy(doc);
    rc
}

unsafe fn mutate_selftest_body(doc: *mut Doc) -> i32 {
    /* 1. QName validation + split */
    match split(b"a:b") {
        Some(q) if q.prefix_len == 1 && q.local_len == 1 => {}
        _ => return 2,
    }
    if split(b"1bad").is_some() {
        return 3;
    }
    if split(b"a:b:c").is_some() {
        return 4;
    }
    if split(b":x").is_some() {
        return 5;
    }
    if split(b"x:").is_some() {
        return 6;
    }

    /* 2. root element, attribute set/replace */
    let r = arena_node(doc, T_ELEMENT);
    if r.is_null() {
        return 7;
    }
    let rn = b"r";
    let rq = QName {
        qname: rn.as_ptr() as *const c_char,
        qname_len: 1,
        prefix: rn.as_ptr() as *const c_char,
        prefix_len: 0,
        local: rn.as_ptr() as *const c_char,
        local_len: 1,
    };
    if crate::xml::arena::qname_assign(doc, r, &rq) != 0 {
        return 8;
    }
    let mut at: *mut Node = ptr::null_mut();
    if mutate::set_attribute(doc, r, b"id", b"x", &mut at) != MUT_OK
        || at.is_null()
        || (*at).value_len != 1
        || node_value(at) != b"x"
        || (*r).attrs != at
    {
        return 9;
    }
    if mutate::set_attribute(doc, r, b"id", b"yy", &mut at) != MUT_OK
        || (*at).value_len != 2
        || !(*(*r).attrs).next.is_null()
    {
        return 10;
    }

    /* 3. fail-closed: non-XML-Char value */
    if mutate::set_attribute(doc, r, b"k", b"\x01", ptr::null_mut()) != MUT_BAD_CHARS {
        return 11;
    }

    /* 3b. forbidden value sequences */
    let mut chk: *mut Node = ptr::null_mut();
    if mutate::new_chardata(doc, T_COMMENT, b"a--b", &mut chk) != MUT_BAD_CHARS {
        return 111;
    }
    if mutate::new_chardata(doc, T_COMMENT, b"x-", &mut chk) != MUT_BAD_CHARS {
        return 112;
    }
    if mutate::new_chardata(doc, T_CDATA, b"a]]>b", &mut chk) != MUT_BAD_CHARS {
        return 113;
    }
    if mutate::new_chardata(doc, T_COMMENT, b"a-b", &mut chk) != MUT_OK || chk.is_null() {
        return 114;
    }
    if mutate::set_content(doc, chk, b"x--y") != MUT_BAD_CHARS {
        return 115;
    }
    if mutate::set_attribute(doc, r, b"xmlns:q", b"", ptr::null_mut()) != MUT_BAD_NS_DECL {
        return 116;
    }
    if mutate::set_attribute(doc, r, b"xmlns", b"", ptr::null_mut()) != MUT_OK {
        return 117;
    }

    let mut det: *mut Node = ptr::null_mut();
    if mutate::new_element(doc, b"det", &mut det) != MUT_OK
        || mutate::set_attribute(doc, det, b"p:k", b"v", &mut at) != MUT_OK
        || (*at).ns_uri_len != 0
    {
        return 12;
    }

    /* 4. the predefined xml: prefix */
    if mutate::set_attribute(doc, r, b"xml:lang", b"en", &mut at) != MUT_OK || node_ns(at) != XML_NS_URI {
        return 13;
    }

    /* 5. xmlns:* declaration then a bound prefix */
    if mutate::set_attribute(doc, r, b"xmlns:p", b"urn:p", &mut at) != MUT_OK || node_ns(at) != XMLNS_NS_URI {
        return 14;
    }
    if mutate::set_attribute(doc, r, b"p:k", b"v", &mut at) != MUT_OK || node_ns(at) != b"urn:p" {
        return 15;
    }

    /* 6. remove by name (idempotent) */
    if mutate::remove_attribute(r, b"id") != 1 {
        return 16;
    }
    if mutate::remove_attribute(r, b"id") != 0 {
        return 17;
    }

    /* 7. rename */
    if mutate::rename(doc, r, b"q") != MUT_OK || (*r).qname_len != 1 || node_local(r) != b"q" || (*r).ns_uri_len != 0 {
        return 18;
    }

    /* 8. content */
    let c1 = arena_node(doc, T_ELEMENT);
    if c1.is_null() {
        return 19;
    }
    (*c1).parent = r;
    (*r).first_child = c1;
    (*r).last_child = c1;
    if mutate::set_content(doc, r, b"hi") != MUT_OK {
        return 20;
    }
    if (*r).first_child.is_null()
        || (*(*r).first_child).type_ != T_TEXT
        || (*(*r).first_child).value_len != 2
        || (*r).last_child != (*r).first_child
        || !(*c1).parent.is_null()
    {
        return 21;
    }
    if mutate::set_content(doc, r, b"") != MUT_OK || !(*r).first_child.is_null() || !(*r).last_child.is_null() {
        return 22;
    }

    /* 9. detach */
    let a1 = arena_node(doc, T_ELEMENT);
    let a2 = arena_node(doc, T_ELEMENT);
    if a1.is_null() || a2.is_null() {
        return 23;
    }
    (*a1).parent = r;
    (*a2).parent = r;
    (*a1).next = a2;
    (*a2).prev = a1;
    (*r).first_child = a1;
    (*r).last_child = a2;
    mutate::detach(a1);
    if (*r).first_child != a2 || !(*a2).prev.is_null() || !(*a1).parent.is_null() {
        return 24;
    }
    mutate::detach(a2);
    if !(*r).first_child.is_null() || !(*r).last_child.is_null() || !(*a2).parent.is_null() {
        return 25;
    }

    /* 10. a live document node + connected root */
    let docn = arena_node(doc, T_DOCUMENT);
    if docn.is_null() {
        return 26;
    }
    (*doc).doc_node = docn;
    let (mut pr, mut ne, mut tx): (*mut Node, *mut Node, *mut Node) = (ptr::null_mut(), ptr::null_mut(), ptr::null_mut());
    if mutate::new_element(doc, b"pr", &mut pr) != MUT_OK || pr.is_null() || !(*pr).parent.is_null() || (*pr).ns_uri_len != 0 {
        return 27;
    }
    if mutate::set_attribute(doc, pr, b"xmlns:p", b"urn:p", ptr::null_mut()) != MUT_OK {
        return 28;
    }
    if mutate::insert_child(doc, docn, pr) != MUT_OK || (*doc).root != pr {
        return 29;
    }
    if mutate::new_chardata(doc, T_TEXT, b"hi", &mut tx) != MUT_OK || (*tx).value_len != 2 {
        return 30;
    }

    /* 11. insert_child resolves the inserted subtree */
    if mutate::new_element(doc, b"p:c", &mut ne) != MUT_OK {
        return 31;
    }
    if mutate::insert_child(doc, pr, ne) != MUT_OK || (*pr).first_child != ne || (*ne).parent != pr || node_ns(ne) != b"urn:p" {
        return 32;
    }
    if mutate::insert_child(doc, ne, tx) != MUT_OK || (*ne).first_child != tx {
        return 33;
    }

    /* 12. unbound prefix in the live tree */
    let mut ub: *mut Node = ptr::null_mut();
    if mutate::new_element(doc, b"z:c", &mut ub) != MUT_OK {
        return 34;
    }
    if mutate::insert_child(doc, pr, ub) != MUT_UNBOUND_NS || !(*ub).parent.is_null() || (*pr).last_child != ne {
        return 35;
    }

    /* 13. deferred resolution */
    let (mut wrap, mut inner): (*mut Node, *mut Node) = (ptr::null_mut(), ptr::null_mut());
    if mutate::new_element(doc, b"p:wrap", &mut wrap) != MUT_OK || mutate::new_element(doc, b"p:inner", &mut inner) != MUT_OK {
        return 36;
    }
    if mutate::insert_child(doc, wrap, inner) != MUT_OK || (*inner).ns_uri_len != 0 {
        return 37;
    }
    if mutate::insert_child(doc, pr, wrap) != MUT_OK || node_ns(wrap) != b"urn:p" || node_ns(inner) != b"urn:p" {
        return 38;
    }

    /* 14. cycle rejection */
    if mutate::insert_child(doc, ne, pr) != MUT_CYCLE {
        return 39;
    }

    /* 15. sibling order */
    let (mut b1, mut b2): (*mut Node, *mut Node) = (ptr::null_mut(), ptr::null_mut());
    if mutate::new_element(doc, b"b1", &mut b1) != MUT_OK || mutate::new_element(doc, b"b2", &mut b2) != MUT_OK {
        return 40;
    }
    if mutate::insert_before(doc, ne, b1) != MUT_OK || (*pr).first_child != b1 || (*b1).next != ne {
        return 41;
    }
    if mutate::insert_after(doc, ne, b2) != MUT_OK || (*ne).next != b2 {
        return 42;
    }
    if mutate::insert_before(doc, ne, ne) != MUT_OK
        || (*ne).next == ne
        || (*ne).prev == ne
        || mutate::insert_after(doc, ne, ne) != MUT_OK
        || (*ne).next == ne
    {
        return 99;
    }

    /* 16. replace */
    let mut rep: *mut Node = ptr::null_mut();
    if mutate::new_element(doc, b"rep", &mut rep) != MUT_OK {
        return 43;
    }
    if mutate::replace_node(doc, ne, rep) != MUT_OK || !(*ne).parent.is_null() || (*rep).parent != pr || (*b1).next != rep {
        return 44;
    }

    /* 17. cross-document import */
    let doc2 = doc_new();
    if doc2.is_null() {
        return 45;
    }
    let mut imp: *mut Node = ptr::null_mut();
    let irc = mutate::import_subtree(doc2, pr, &mut imp);
    let mut rc = 0;
    if irc != MUT_OK
        || imp.is_null()
        || imp == pr
        || crate::xml::node_qname(imp) != b"pr"
        || (*imp).first_child.is_null()
        || (*imp).first_child == (*pr).first_child
    {
        rc = 46;
    }
    doc_destroy(doc2);
    if rc != 0 {
        return rc;
    }

    /* 18. single-root rule */
    let mut root2: *mut Node = ptr::null_mut();
    if mutate::new_element(doc, b"root2", &mut root2) != MUT_OK {
        return 47;
    }
    if mutate::insert_child(doc, docn, root2) != MUT_HIERARCHY {
        return 48;
    }
    let _ = (ERR_OOM, T_PI);
    0
}
