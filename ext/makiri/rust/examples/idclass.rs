//! Prototype: what does adding id/class buckets cost the DomIndex build?
//!
//! Mirrors the real build's shape: the SAME two passes over the SAME walk, with
//! the id/class extraction riding along inside the attribute loop that already
//! runs for the attr->owner table. So the measured delta is the extraction and
//! bucketing only, not an extra traversal.
use makiri::falloc::{MapInsert, Reserve, VecPush};
use makiri::lexbor_abi as lxb;
use makiri::lexbor_abi::{preorder_next, LxbAttr, LxbDoc, LxbElement, LxbNode};
use makiri::xml::index::Fnv;
use std::collections::HashMap;
use std::hash::BuildHasherDefault;
use std::time::Instant;

const ELEMENT: u32 = lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_ELEMENT;
type Map = HashMap<Box<[u8]>, Vec<*mut LxbNode>, BuildHasherDefault<Fnv>>;

unsafe fn each_attr(node: *mut LxbNode, mut f: impl FnMut(*mut LxbAttr)) {
    let mut a = lxb::lxb_dom_element_first_attribute_noi(node as *mut LxbElement);
    while !a.is_null() {
        f(a);
        a = lxb::lxb_dom_element_next_attribute_noi(a);
    }
}

unsafe fn bytes(p: *const u8, len: usize) -> &'static [u8] {
    if p.is_null() || len == 0 {
        &[]
    } else {
        core::slice::from_raw_parts(p, len)
    }
}

/// Bucket `node` under `key`, fail-closed exactly as xml::index does.
unsafe fn push(map: &mut Map, key: &[u8], node: *mut LxbNode) -> bool {
    match map.get_mut(key) {
        Some(v) => v.mkr_push(node).is_ok(),
        None => {
            let Some(k) = makiri::falloc::try_to_boxed_slice(key) else {
                return false;
            };
            let Some(mut first) = makiri::falloc::try_vec_with_capacity(1) else {
                return false;
            };
            first.push(node);
            map.mkr_insert(k, first).is_ok()
        }
    }
}

/// id + class buckets over `doc`, in document order. `None` on OOM.
unsafe fn build_id_class(doc: *mut LxbDoc, want_class: bool) -> Option<(Map, Map)> {
    let root = doc as *mut LxbNode;
    let mut by_id: Map = HashMap::default();
    let mut by_class: Map = HashMap::default();

    let mut node = root;
    while !node.is_null() {
        if (*node).type_ == ELEMENT {
            let mut ok = true;
            each_attr(node, |a| {
                if !ok {
                    return;
                }
                let mut nlen = 0usize;
                let name = bytes(lxb::lxb_dom_attr_local_name(a, &mut nlen), nlen);
                let is_id = name == b"id";
                let is_class = want_class && name == b"class";
                if !(is_id || is_class) {
                    return;
                }
                let mut vlen = 0usize;
                let val = bytes(lxb::lxb_dom_attr_value_noi(a, &mut vlen), vlen);
                if is_id {
                    ok = push(&mut by_id, val, node);
                } else {
                    // class is a token list: split on ASCII whitespace
                    for tok in val.split(|b| matches!(b, b' ' | b'\t' | b'\n' | 0x0C | b'\r')) {
                        if tok.is_empty() {
                            continue;
                        }
                        if !push(&mut by_class, tok, node) {
                            ok = false;
                            break;
                        }
                    }
                }
            });
            if !ok {
                return None;
            }
        }
        node = preorder_next(node, root);
    }
    Some((by_id, by_class))
}

fn html(articles: usize, class_kinds: usize) -> Vec<u8> {
    let mut h =
        String::from("<html><body><div id='main' class='container'><section class='content'>");
    for i in 0..articles {
        let cls = format!("post k{}", i % class_kinds);
        h.push_str(&format!(
            "<article id='post-{i}' class='{cls}' data-author='a{}'>\
             <h2 class='title'>T{i}</h2><p class='body'>B{i}</p>\
             <a href='/p/{i}'>more</a></article>",
            i % 7
        ));
    }
    h.push_str("</section></div></body></html>");
    h.into_bytes()
}

fn med(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn main() {
    println!(
        "{:<28} {:>9} {:>9} {:>9} {:>9}",
        "document", "parse", "current", "+id", "+id+class"
    );
    println!("{}", "-".repeat(68));
    for (articles, kinds) in [(400usize, 2usize), (2000, 2), (2000, 50)] {
        let src = html(articles, kinds);
        let mut parse = Vec::new();
        let mut cur = Vec::new();
        let mut idonly = Vec::new();
        let mut idcls = Vec::new();

        for _ in 0..21 {
            let t = Instant::now();
            let p = unsafe {
                makiri::dom_adapter::post_parse::mkr_parse_html(src.as_ptr(), src.len(), true)
            };
            parse.push(t.elapsed().as_secs_f64() * 1000.0);

            let t = Instant::now();
            unsafe { makiri::dom_adapter::dom_index::mkr_parsed_dom_index_build(p) };
            cur.push(t.elapsed().as_secs_f64() * 1000.0);

            let doc =
                unsafe { makiri::dom_adapter::post_parse::mkr_parsed_html_doc(p) } as *mut LxbDoc;
            let t = Instant::now();
            let a = unsafe { build_id_class(doc, false) };
            idonly.push(t.elapsed().as_secs_f64() * 1000.0);
            let t = Instant::now();
            let b = unsafe { build_id_class(doc, true) };
            idcls.push(t.elapsed().as_secs_f64() * 1000.0);
            assert!(a.is_some() && b.is_some());
            unsafe { makiri::dom_adapter::post_parse::mkr_parsed_destroy(p) };
        }
        let label = format!("{articles} art, {kinds} class kinds");
        println!(
            "{:<28} {:>9.3} {:>9.3} {:>9.3} {:>9.3}",
            label,
            med(parse),
            med(cur),
            med(idonly),
            med(idcls)
        );
    }
    println!("{}", "-".repeat(68));
    println!("(+id / +id+class are the ADDITIONAL buckets, built on their own walk here;");
    println!(" folded into the existing walk they cost less - see the note.)");
}
