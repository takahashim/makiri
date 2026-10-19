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
    d.link_attribute(a, x, ShortcutRule::Take(Shortcut::Id));
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

#[test]
fn compaction_keeps_every_span_and_bounds_the_store() {
    let mut d = Document::new(0).expect("doc");
    let div = d.names.tag_append_lower(b"div").expect("tag");
    let el = d
        .create_element(div, res::NS_HTML as u32, 0, None)
        .expect("el");
    d.append_child(NodeId::DOCUMENT, el);
    let text = d.create_data(NodeType::Text, b"first").expect("text");
    d.append_child(el, text);
    let detached = d.create_data(NodeType::Comment, b"detached").expect("c");
    let pi = d.create_pi(b"target", b"pi data").expect("pi");
    d.append_child(el, pi);
    let id = d.names.attr_local_name_append(b"title").expect("name");
    let attr = d.create_attr(id, 0, 0, None, b"attr value").expect("attr");
    d.link_attribute(el, attr, ShortcutRule::Keep);

    let big = vec![b'x'; 1024];
    for i in 0..2000u32 {
        let mut v = big.clone();
        v.extend_from_slice(i.to_string().as_bytes());
        d.set_data(text, &v).expect("set");
        if i % 3 == 0 {
            d.set_data(attr, format!("v{i}").as_bytes()).expect("attr");
        }
        if d.compaction_due() {
            d.compact().expect("compact");
        }
        assert!(
            d.bytes.len() < 8 * 1024,
            "the store grew to {}",
            d.bytes.len()
        );
        assert_eq!(d.data(text), Some(&v[..]));
    }
    assert_eq!(d.data(detached), Some(&b"detached"[..]));
    assert_eq!(d.pi_target(pi), Some(&b"target"[..]));
    assert_eq!(d.data(pi), Some(&b"pi data"[..]));
    assert_eq!(d.attr_value(attr), Some(&b"v1998"[..]));
    d.compact().expect("compact");
    assert_eq!(d.dead_bytes(), 0);
    assert_eq!(d.bytes.len(), 1024 + 4 + 8 + 6 + 7 + 5);
}

#[test]
fn text_appended_between_other_stores_grows_the_store_linearly() {
    /* Table foster parenting appends to one text between comments; several
     * texts taking turns must not push each other's room out either. */
    for texts in [1usize, 2, 4] {
        let mut d = Document::new(0).expect("doc");
        let ts: Vec<NodeId> = (0..texts)
            .map(|_| d.create_data(NodeType::Text, b"").expect("text"))
            .collect();
        let n = 20_000;
        for i in 0..n {
            let t = ts[i % texts];
            d.append_text(t, b"a").expect("append");
            d.create_data(NodeType::Comment, b"x").expect("comment");
        }
        for &t in &ts {
            assert_eq!(d.data(t).map(<[u8]>::len), Some(n / texts));
        }
        /* Live bytes are 2n; the rooms and moved copies add a constant
         * factor, where a copy per append made it ~n^2/2 per text. */
        assert!(
            d.bytes.len() < 12 * n,
            "{texts} texts: {} bytes",
            d.bytes.len()
        );
        d.compact().expect("compact");
        for &t in &ts {
            assert_eq!(d.data(t).map(<[u8]>::len), Some(n / texts));
            assert!(d.data(t).expect("data").iter().all(|&c| c == b'a'));
        }
        assert_eq!(d.bytes.len(), 2 * n);
    }
}

#[test]
fn owned_bytes_counts_the_namespace_and_prefix_tables() {
    let mut d = Document::new(0).expect("doc");
    let (bytes, growth) = (d.owned_bytes(), d.table_growth());
    let uri = [b'x'; 100_000];
    d.intern_ns(&uri).expect("ns");
    d.intern_prefix(&uri[..50_000]).expect("prefix");
    assert!(d.owned_bytes() >= bytes + 150_000);
    assert!(d.table_growth() >= growth + 150_000);
}
