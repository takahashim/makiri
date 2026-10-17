//! The arena's own linking and naming, without Lexbor.

#![allow(clippy::disallowed_methods)]

use super::*;

#[test]
fn links_and_attributes_keep_their_lists() {
    let mut d = Document::new(0).expect("doc");
    let div = d.names.tag_append_lower(b"div").expect("tag");
    let a = d
        .create_element(div, res::NS_HTML as u32, 0, None)
        .expect("a");
    let b = d
        .create_element(div, res::NS_HTML as u32, 0, None)
        .expect("b");
    let c = d.create_data(NodeType::Text, b"t").expect("c");
    d.append_child(NodeId::DOCUMENT, a);
    d.append_child(a, c);
    d.insert_before(c, b);
    assert_eq!(d.first_child(a), Some(b));
    assert_eq!(d.next(b), Some(c));
    assert_eq!(d.last_child(a), Some(c));
    d.detach(b);
    assert_eq!(d.first_child(a), Some(c));
    assert_eq!(d.prev(c), None);
    d.insert_after(c, b);
    assert_eq!(d.last_child(a), Some(b));

    let id = d.names.attr_local_name_append(b"id").expect("id");
    let x = d.create_attr(id, 0, 0, None, b"x").expect("attr");
    d.attr_append(a, x);
    assert_eq!(d.id_attr(a), Some(x));
    assert_eq!(d.parent(x), Some(a));
    assert_eq!(d.attr_value(x), Some(&b"x"[..]));
    d.set_data(x, b"y").expect("set");
    assert_eq!(d.attr_value(x), Some(&b"y"[..]));
    d.attr_remove(x);
    assert_eq!(d.first_attr(a), None);
    assert_eq!(d.id_attr(a), None);
}

#[test]
fn namespaces_keep_lexbor_static_ids_and_add_their_own() {
    let mut d = Document::new(0).expect("doc");
    assert_eq!(
        d.lookup_ns(b"http://www.w3.org/2000/svg"),
        Some(res::NS_SVG as u32)
    );
    let own = d.intern_ns(b"urn:x").expect("ns").expect("id");
    assert!(own as usize >= res::NS__LAST_ENTRY);
    assert_eq!(d.ns_uri(own), Some(&b"urn:x"[..]));
    assert_eq!(
        d.intern_prefix(b"xlink").expect("p"),
        Some(res::NS_XLINK as u32)
    );
}
