//! The arena's own linking and naming, without Lexbor.

#![allow(clippy::disallowed_methods)]

use super::*;

#[test]
fn links_and_attributes_keep_their_lists() {
    let mut d = Document::new(0).expect("doc");
    let div = d.names.tag_append_lower(b"div").expect("tag");
    let a = d.create_element(div, HTML_NS, 0, None).expect("a");
    let b = d.create_element(div, HTML_NS, 0, None).expect("b");
    let c = d
        .create_data(DataKind::Text, NodeName::default(), b"t")
        .expect("c");
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
    let el = d.create_element(div, HTML_NS, 0, None).expect("el");
    d.append_child(NodeId::DOCUMENT, el);
    let text = d
        .create_data(DataKind::Text, NodeName::default(), b"first")
        .expect("text");
    d.append_child(el, text);
    let detached = d
        .create_data(DataKind::Comment, NodeName::default(), b"detached")
        .expect("c");
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
            d.stored_bytes() < 8 * 1024,
            "the store grew to {}",
            d.stored_bytes()
        );
        assert_eq!(d.data(text), Some(&v[..]));
    }
    assert_eq!(d.data(detached), Some(&b"detached"[..]));
    assert_eq!(d.pi_target(pi), Some(&b"target"[..]));
    assert_eq!(d.data(pi), Some(&b"pi data"[..]));
    assert_eq!(d.attr_value(attr), Some(&b"v1998"[..]));
    d.compact().expect("compact");
    assert_eq!(d.dead_bytes(), 0);
    assert_eq!(d.stored_bytes(), 1024 + 4 + 8 + 6 + 7 + 5);
}

#[test]
fn text_appended_between_other_stores_grows_the_store_linearly() {
    /* Table foster parenting appends to one text between comments; several
     * texts taking turns must not push each other's room out either. */
    for texts in [1usize, 2, 4] {
        let mut d = Document::new(0).expect("doc");
        let ts: Vec<NodeId> = (0..texts)
            .map(|_| {
                d.create_data(DataKind::Text, NodeName::default(), b"")
                    .expect("text")
            })
            .collect();
        let n = 20_000;
        for i in 0..n {
            let t = ts[i % texts];
            d.append_text(t, b"a").expect("append");
            d.create_data(DataKind::Comment, NodeName::default(), b"x")
                .expect("comment");
        }
        for &t in &ts {
            assert_eq!(d.data(t).map(<[u8]>::len), Some(n / texts));
        }
        /* Live bytes are 2n; the rooms and moved copies add a constant
         * factor, where a copy per append made it ~n^2/2 per text. */
        assert!(
            d.stored_bytes() < 12 * n,
            "{texts} texts: {} bytes",
            d.stored_bytes()
        );
        d.compact().expect("compact");
        for &t in &ts {
            assert_eq!(d.data(t).map(<[u8]>::len), Some(n / texts));
            assert!(d.data(t).expect("data").iter().all(|&c| c == b'a'));
        }
        assert_eq!(d.stored_bytes(), 2 * n);
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

#[test]
fn compaction_forgets_the_rooms_it_moved() {
    /* A text whose data does not move in the compaction, with a room after
     * it before: the room's bytes are another span's afterwards, so an
     * append must not write into them. Every byte before the text is live
     * here, which random edits seldom arrange. */
    let mut d = Document::new(0).expect("doc");
    let text = |d: &mut Document, b: &[u8]| {
        d.create_data(DataKind::Text, NodeName::default(), b)
            .expect("text")
    };
    text(&mut d, b"pp");
    let c = text(&mut d, b"");
    let t = text(&mut d, b"");
    d.set_data(c, b"c").expect("set");
    /* `t` goes to the tail with room after it: [pp][c][tt..] */
    d.append_text(t, b"tt").expect("append");
    d.compact().expect("compact");
    assert_eq!(d.stored_bytes(), 5);
    let z = text(&mut d, b"zz");
    d.append_text(t, b"uu").expect("append");
    assert_eq!(d.data(t), Some(&b"ttuu"[..]));
    assert_eq!(d.data(z), Some(&b"zz"[..]));
    assert_eq!(d.data(c), Some(&b"c"[..]));
}

/// A seeded xorshift, so a failure replays from the seed it prints.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn one_in(&mut self, n: usize) -> bool {
        self.below(n) == 0
    }
}

/// What one node holds in the store, read through the arena: a node's data,
/// a PI's target and data, an attribute's value, a doctype's three strings -
/// every span `compact` moves.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Held {
    None,
    Data(Vec<u8>),
    Pi(Vec<u8>, Vec<u8>),
    Value(Vec<u8>),
    Doctype(Vec<u8>, Vec<u8>, Vec<u8>),
}

impl Held {
    fn len(&self) -> usize {
        match self {
            Held::None => 0,
            Held::Data(b) | Held::Value(b) => b.len(),
            Held::Pi(t, d) => t.len() + d.len(),
            Held::Doctype(a, b, c) => a.len() + b.len() + c.len(),
        }
    }
}

fn held(d: &Document, n: NodeId) -> Held {
    if let Some((a, b, c)) = d.doctype(n) {
        return Held::Doctype(a.to_vec(), b.to_vec(), c.to_vec());
    }
    if let Some(t) = d.pi_target(n) {
        return Held::Pi(t.to_vec(), d.data(n).unwrap_or_default().to_vec());
    }
    if let Some(v) = d.attr_value(n) {
        return Held::Value(v.to_vec());
    }
    match d.data(n) {
        Some(b) => Held::Data(b.to_vec()),
        None => Held::None,
    }
}

fn ids(d: &Document) -> impl Iterator<Item = NodeId> {
    (1..=d.node_count()).filter_map(NodeId::from_word)
}

/// Every node's [`Held`] and its tree links, in id order.
fn read_all(d: &Document) -> Vec<(Held, [Option<NodeId>; 4])> {
    ids(d)
        .map(|n| {
            let links = [d.parent(n), d.first_child(n), d.next(n), d.first_attr(n)];
            (held(d, n), links)
        })
        .collect()
}

/// Bytes `len` long, telling the step and the seed that stored them apart,
/// so a span moved onto another's bytes reads differently.
fn bytes(rng: &mut Rng, step: usize) -> Vec<u8> {
    let len = match rng.below(8) {
        0 => 0,
        1 => 200 + rng.below(800),
        _ => rng.below(40),
    };
    let tag = format!("<{step}:{}>", rng.below(1000));
    tag.bytes().cycle().take(len).collect()
}

const SOURCES: [&[u8]; 4] = [
    b"",
    b"<!DOCTYPE html PUBLIC \"-//W3C//DTD HTML 4.01//EN\" \"x\"><title>t</title>\
      <p id=a class=b title=c>one<!--c--><?pi data?><b>two</b>three</p>\
      <template><i>in</i>content</template>",
    /* Foster parenting appends to texts between other stores, which leaves
     * the texts with rooms reserved after them (`ByteStore::append`). */
    b"<table>aa<!--1-->bb<!--2-->cc<tr>dd<!--3-->ee</table><table>x<td>y</table>",
    b"<svg><title>s</title><desc>d</desc></svg><math><mi>m</mi></math>\
      <select><option>o<option selected>p</select><p a=1 b=2 c=3>",
];

/// Random edits over a document, with compactions forced between them and
/// run when due, as `HtmlParsed::edit` runs them: every node reads the same
/// across each (detached ones included - a Ruby wrapper can still read
/// them), the store holds exactly the live bytes after one, and the dead
/// count is right throughout, so `compaction_due` decides on true numbers.
#[test]
fn compaction_keeps_what_every_edit_left() {
    for seed in 1..=300u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let src = SOURCES[rng.below(SOURCES.len())];
        let mut d = crate::html::tree::parse_document(src).expect("parse").doc;
        let mut model: Vec<Held> = ids(&d).map(|n| held(&d, n)).collect();
        /* A few texts appended to again and again, between other stores. */
        let mut hot: Vec<NodeId> = Vec::new();
        let mut compactions = 0;
        for step in 0..300 {
            let pick = |rng: &mut Rng, d: &Document| {
                NodeId::from_word(1 + rng.below(d.node_count())).expect("id")
            };
            match rng.below(12) {
                0..=2 => {
                    let n = pick(&mut rng, &d);
                    let b = bytes(&mut rng, step);
                    d.set_data(n, &b).expect("set_data");
                    let m = &mut model[n.to_word() - 1];
                    match m {
                        Held::Data(x) | Held::Value(x) | Held::Pi(_, x) => *x = b,
                        Held::None | Held::Doctype(..) => {}
                    }
                }
                3..=5 => {
                    if hot.is_empty() || rng.one_in(6) {
                        let n = pick(&mut rng, &d);
                        if matches!(model[n.to_word() - 1], Held::Data(_)) {
                            hot.push(n);
                            if hot.len() > 6 {
                                hot.remove(0);
                            }
                        }
                    }
                    if let Some(&n) = hot.get(rng.below(hot.len())) {
                        let b = bytes(&mut rng, step);
                        d.append_text(n, &b).expect("append_text");
                        if let Held::Data(x) = &mut model[n.to_word() - 1] {
                            x.extend_from_slice(&b);
                        }
                    }
                }
                6 => {
                    let b = bytes(&mut rng, step);
                    let kind =
                        [DataKind::Text, DataKind::Comment, DataKind::CDataSection][rng.below(3)];
                    let n = d
                        .create_data(kind, NodeName::default(), &b)
                        .expect("create_data");
                    let at = pick(&mut rng, &d);
                    if matches!(d.kind(at), NodeType::Element | NodeType::DocumentFragment) {
                        d.append_child(at, n);
                    }
                    model.push(Held::Data(b));
                }
                7 => {
                    let (t, b) = (bytes(&mut rng, step), bytes(&mut rng, step));
                    d.create_pi(&t, &b).expect("create_pi");
                    model.push(Held::Pi(t, b));
                }
                8 => {
                    let b = bytes(&mut rng, step);
                    let name = d.names.attr_local_name_append(b"data-x").expect("name");
                    let a = d.create_attr(name, 0, 0, None, &b).expect("create_attr");
                    let el = pick(&mut rng, &d);
                    if d.kind(el) == NodeType::Element {
                        d.link_attribute(el, a, ShortcutRule::Keep);
                    }
                    model.push(Held::Value(b));
                }
                9 => {
                    let (a, b, c) = (
                        bytes(&mut rng, step),
                        bytes(&mut rng, step),
                        bytes(&mut rng, step),
                    );
                    d.create_doctype(NodeName::default(), &a, &b, &c)
                        .expect("create_doctype");
                    model.push(Held::Doctype(a, b, c));
                }
                10 => {
                    /* A document node is never copied (`clone_node` refuses
                     * it first). */
                    let n = pick(&mut rng, &d);
                    if n == NodeId::DOCUMENT {
                        continue;
                    }
                    let from = d.node_count();
                    let copy = d.copy_within(n, rng.one_in(2)).expect("copy");
                    assert_eq!(held(&d, copy), model[n.to_word() - 1], "seed {seed}");
                    /* A copy's subtree is checked against the source by the
                     * copy tests; here it is what compactions must keep. */
                    for i in from + 1..=d.node_count() {
                        let c = NodeId::from_word(i).expect("id");
                        model.push(held(&d, c));
                    }
                }
                _ => {
                    let n = pick(&mut rng, &d);
                    match d.kind(n) {
                        NodeType::Document => {}
                        NodeType::Attribute => d.attr_remove(n),
                        _ => d.detach(n),
                    }
                }
            }
            assert_eq!(model.len(), d.node_count(), "seed {seed} step {step}");
            let live: usize = model.iter().map(Held::len).sum();
            assert_eq!(
                d.stored_bytes() - d.dead_bytes(),
                live,
                "seed {seed} step {step}: the dead count"
            );
            if rng.one_in(8) || d.compaction_due() {
                let before = read_all(&d);
                d.compact().expect("compact");
                compactions += 1;
                assert_eq!(read_all(&d), before, "seed {seed} step {step}");
                assert_eq!(d.dead_bytes(), 0, "seed {seed} step {step}");
                assert_eq!(d.stored_bytes(), live, "seed {seed} step {step}");
            }
            for (i, n) in ids(&d).enumerate() {
                assert_eq!(held(&d, n), model[i], "seed {seed} step {step} node {i}");
            }
        }
        assert!(compactions > 10, "seed {seed}: {compactions} compactions");
    }
}
