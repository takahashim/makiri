//! The generator of `html::res::tree` and `html::res::entities`, from the
//! sources the HTML Standard defines them by - no Lexbor:
//!
//! - **`tree.rs`** from the HTML Standard's `source` at the pinned commit
//!   (`spec/conformance/parser_data.rb` fetches it, digest and all): the
//!   special and formatting categories, the element-in-scope sets, and the
//!   SVG tag-name, SVG attribute and foreign attribute adjustment tables. The
//!   tables are indexed by the static ids of `html::res::names`, which keeps
//!   Lexbor's numbering - the Standard numbers nothing.
//! - **`entities.rs`** from the WHATWG `entities.json` (pinned by digest),
//!   built into the static search tree the tokenizer walks, with a port of
//!   Lexbor's `utils/lexbor/html/tokenizer_entities_bst.py` - the layout is
//!   Lexbor's, the list the Standard's.
//!
//! The tests need the fetched files, so they are `#[ignore]`d and `rake
//! conformance:parser` runs them; they fail when a checked-in file is not
//! what the generator makes now, and `MAKIRI_REGENERATE=1` rewrites it.
//! While the reference Lexbor is linked, `lexbor::res_tests` also holds both
//! files to Lexbor's own tables.
//!
//! The Standard's markup is read as text, with three rules each table here
//! needs: HTML comments are dropped first (a commented-out row - SVG 1.2's
//! `solidcolor`, the special `image` - is not the Standard's), a name is
//! MathML or SVG when that word precedes its `<code>`, line break or not, and
//! a table cell reading `(none)` is empty.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::PathBuf;

use crate::html::res::names as res;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn data(rel: &str) -> PathBuf {
    super::spec_dir().join("data").join(rel)
}

/// A byte string as a Rust literal.
pub(crate) fn lit(b: &[u8]) -> String {
    let mut s = String::from("b\"");
    for &c in b {
        match c {
            b'"' | b'\\' => {
                s.push('\\');
                s.push(c as char);
            }
            0x20..=0x7e => s.push(c as char),
            _ => {
                let _ = write!(s, "\\x{c:02x}");
            }
        }
    }
    s.push('"');
    s
}

/// Whether the checked-in `src/html/res/{rel}` is `generated`; with
/// `MAKIRI_REGENERATE` set, write it instead.
fn check_or_write(rel: &str, generated: &str) {
    let path = root().join("src/html/res").join(rel);
    if std::env::var_os("MAKIRI_REGENERATE").is_some() {
        std::fs::write(&path, generated).expect("write a generated table");
        return;
    }
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        current == generated,
        "{} is not what the generator makes now - rerun with MAKIRI_REGENERATE=1",
        path.display()
    );
}

/* ---------- the HTML Standard ---------- */

/// The pinned Standard's source, HTML comments dropped.
fn spec() -> String {
    let path = data("whatwg/html-source");
    let text = std::fs::read_to_string(&path)
        .expect("the HTML Standard's source - run `ruby spec/conformance/parser_data.rb`");
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(i) = rest.find("<!--") {
        out.push_str(&rest[..i]);
        rest = match rest[i..].find("-->") {
            Some(j) => &rest[i + j + 3..],
            None => "",
        };
    }
    out.push_str(rest);
    out
}

/// The text from `start` to the first `end` after it.
fn between<'s>(src: &'s str, start: &str, end: &str) -> &'s str {
    let i = src
        .find(start)
        .unwrap_or_else(|| panic!("the Standard has no {start:?}"));
    let j = src[i..]
        .find(end)
        .unwrap_or_else(|| panic!("no {end:?} after {start:?}"));
    &src[i..i + j]
}

/// The first `<ul class="brief">` list after `start`.
fn list_after<'s>(src: &'s str, start: &str) -> &'s str {
    let i = src
        .find(start)
        .unwrap_or_else(|| panic!("the Standard has no {start:?}"));
    between(&src[i..], "<ul class=\"brief\">", "</ul>")
}

/// Every element named by a bare `<code>` in `text`, with its namespace:
/// MathML or SVG when that word precedes the `<code>`, HTML otherwise.
fn elements(text: &str) -> BTreeSet<(usize, String)> {
    let mut out = BTreeSet::new();
    let mut at = 0;
    while let Some(i) = text[at..].find("<code>") {
        let start = at + i + "<code>".len();
        let end = start + text[start..].find("</code>").expect("a closed <code>");
        let before = text[..at + i].trim_end();
        let ns = if before.ends_with("MathML") {
            res::NS_MATH
        } else if before.ends_with("SVG") {
            res::NS_SVG
        } else {
            res::NS_HTML
        };
        out.insert((ns, text[start..end].to_ascii_lowercase()));
        at = end;
    }
    out
}

/// The body rows of the first `<table>` after `start`, each as its cells'
/// text: tags stripped, whitespace trimmed, `(none)` empty.
fn table_after(src: &str, start: &str) -> Vec<Vec<String>> {
    let i = src
        .find(start)
        .unwrap_or_else(|| panic!("the Standard has no {start:?}"));
    let table = between(&src[i..], "<table>", "</table>");
    let body = table.split("<tbody>").nth(1).expect("a table body");
    body.split("<tr>")
        .skip(1)
        .map(|row| {
            row.split("<td>")
                .skip(1)
                .map(|cell| {
                    let mut text = String::new();
                    let mut in_tag = false;
                    for c in cell.chars() {
                        match c {
                            '<' => in_tag = true,
                            '>' => in_tag = false,
                            _ if !in_tag => text.push(c),
                            _ => {}
                        }
                    }
                    let text = text.trim();
                    if text == "(none)" {
                        String::new()
                    } else {
                        text.to_string()
                    }
                })
                .collect()
        })
        .collect()
}

/// The element sets of "the stack of open elements", 13.2.4.3 and 13.2.4.2.
struct Categories {
    special: BTreeSet<(usize, String)>,
    formatting: BTreeSet<(usize, String)>,
    scope: BTreeSet<(usize, String)>,
    list_item_scope: BTreeSet<(usize, String)>,
    button_scope: BTreeSet<(usize, String)>,
    table_scope: BTreeSet<(usize, String)>,
}

fn categories(src: &str) -> Categories {
    let special = elements(between(
        src,
        "<dt><dfn>Special</dfn></dt>",
        "<p class=\"note\">",
    ));
    let formatting = elements(between(src, "<dt><dfn>Formatting</dfn></dt>", "</dd>"));
    let scope = elements(list_after(src, "<dfn data-x=\"has an element in scope\">"));
    /* The other scopes are "all the element types listed above" plus their
     * own. */
    let plus = |marker: &str| -> BTreeSet<(usize, String)> {
        scope
            .iter()
            .cloned()
            .chain(elements(list_after(src, marker)))
            .collect()
    };
    Categories {
        list_item_scope: plus("<dfn data-x=\"has an element in list item"),
        button_scope: plus("<dfn data-x=\"has an element in button"),
        table_scope: elements(list_after(src, "<dfn data-x=\"has an element in table")),
        special,
        formatting,
        scope,
    }
}

/// The categories, as `html::actions::is_category` reads them.
const CATEGORIES: [(&str, u8); 8] = [
    ("_UNDEF", 0),
    ("ORDINARY", 1),
    ("SPECIAL", 2),
    ("FORMATTING", 4),
    ("SCOPE", 8),
    ("SCOPE_LIST_ITEM", 16),
    ("SCOPE_BUTTON", 32),
    ("SCOPE_TABLE", 64),
];

/// The static tag id of the lower-case `name`.
fn tag_id(name: &str) -> Option<usize> {
    res::TAG_NAMES
        .iter()
        .position(|n| n.eq_ignore_ascii_case(name.as_bytes()))
}

fn generate_tree_tables(src: &str) -> String {
    let cats = categories(src);
    let mut out = String::from(
        "//! GENERATED by `conformance::res_gen` from the HTML Standard (the pinned\n\
         //! `source`, docs/parser_differences.ja.md) - do not edit. The tree\n\
         //! builder's static tables, indexed by `names`' static ids.\n\n\
         use super::names::{NS__LAST_ENTRY, TAG__LAST_ENTRY};\n\n",
    );
    for (n, v) in CATEGORIES {
        let _ = writeln!(out, "pub const CAT_{n}: u8 = {v};");
    }

    /* Every tag a category names must have a static id to carry it. */
    for (ns, name) in cats
        .special
        .iter()
        .chain(&cats.formatting)
        .chain(&cats.list_item_scope)
        .chain(&cats.button_scope)
        .chain(&cats.table_scope)
    {
        assert!(
            tag_id(name).is_some(),
            "{name} (namespace {ns}) has no static tag id - add it to html::res::names"
        );
    }
    let _ = writeln!(
        out,
        "\n/// Each static tag's categories per static namespace: special and\n\
         /// formatting (13.2.4.3; ordinary is neither) and the element-in-scope\n\
         /// sets (13.2.4.2).\n\
         pub static TAG_CATS: [[u8; NS__LAST_ENTRY]; TAG__LAST_ENTRY] = ["
    );
    for name in res::TAG_NAMES {
        let name = String::from_utf8_lossy(name).to_ascii_lowercase();
        let row: Vec<String> = (0..res::NS__LAST_ENTRY)
            .map(|ns| {
                let key = (ns, name.clone());
                let bit = |set: &BTreeSet<(usize, String)>, cat: u8| {
                    if set.contains(&key) {
                        cat
                    } else {
                        0
                    }
                };
                let kind = bit(&cats.special, 2) | bit(&cats.formatting, 4);
                /* Ordinary: neither special nor formatting. */
                let ordinary = if kind == 0 { 1 } else { 0 };
                let bits = kind
                    | ordinary
                    | bit(&cats.scope, 8)
                    | bit(&cats.list_item_scope, 16)
                    | bit(&cats.button_scope, 32)
                    | bit(&cats.table_scope, 64);
                bits.to_string()
            })
            .collect();
        let _ = writeln!(out, "    [{}],", row.join(", "));
    }
    out.push_str("];\n");

    /* 13.2.6.5, foreign content's "any other start tag". */
    let tags: BTreeMap<String, String> = table_after(
        src,
        "token's tag name is one of the ones in the first column of the following table, change the tag",
    )
    .into_iter()
    .map(|r| (r[0].clone(), r[1].clone()))
    .collect();
    for lower in tags.keys() {
        assert!(
            tag_id(lower).is_some(),
            "SVG {lower} has no static tag id - add it to html::res::names"
        );
    }
    let _ = writeln!(
        out,
        "\n/// Each static tag's SVG name in its proper case, empty when it needs\n\
         /// none (13.2.6.5, the SVG tag name table).\n\
         pub static SVG_FIXNAME: [&[u8]; TAG__LAST_ENTRY] = ["
    );
    for name in res::TAG_NAMES {
        let name = String::from_utf8_lossy(name).to_ascii_lowercase();
        let fixed = tags.get(&name).map_or(&b""[..], |p| p.as_bytes());
        let _ = writeln!(out, "    {},", lit(fixed));
    }
    out.push_str("];\n");

    /* 13.2.6.1, "adjust SVG attributes". */
    let rows = table_after(src, "<dfn>adjust SVG attributes</dfn>");
    let _ = writeln!(
        out,
        "\n/// SVG attribute names to adjust: (lower case, proper case)\n\
         /// (13.2.6.1, \"adjust SVG attributes\").\n\
         pub static SVG_ATTR_ADJUST: [(&[u8], &[u8]); {}] = [",
        rows.len()
    );
    for r in &rows {
        let _ = writeln!(
            out,
            "    ({}, {}),",
            lit(r[0].as_bytes()),
            lit(r[1].as_bytes())
        );
    }
    out.push_str("];\n");

    /* 13.2.6.1, "adjust foreign attributes". */
    let rows = table_after(src, "<dfn>adjust foreign attributes</dfn>");
    let _ = writeln!(
        out,
        "\n/// A foreign attribute to adjust: (name, prefix, local name, namespace).\n\
         pub type ForeignAdjust = (&'static [u8], &'static [u8], &'static [u8], usize);\n\n\
         /// Foreign attributes to adjust (13.2.6.1, \"adjust foreign attributes\").\n\
         pub static FOREIGN_ATTR_ADJUST: [ForeignAdjust; {}] = [",
        rows.len()
    );
    for r in &rows {
        let ns = match r[3].as_str() {
            "XLink namespace" => res::NS_XLINK,
            "XML namespace" => res::NS_XML,
            "XMLNS namespace" => res::NS_XMLNS,
            other => panic!("an unknown namespace {other:?} in the foreign attribute table"),
        };
        let _ = writeln!(
            out,
            "    ({}, {}, {}, {ns}),",
            lit(r[0].as_bytes()),
            lit(r[1].as_bytes()),
            lit(r[2].as_bytes())
        );
    }
    out.push_str("];\n");
    out
}

/* ---------- the entity tree, ported from tokenizer_entities_bst.py ---------- */

#[derive(Default)]
struct Layer(BTreeMap<u8, LayerEntry>);

#[derive(Default)]
struct LayerEntry {
    next: Layer,
    values: Vec<Vec<u8>>,
}

struct Node {
    key: u8,
    left: usize,
    right: usize,
    next: usize,
    value: Option<Vec<u8>>,
    pending: Option<Layer>,
}

/// `entities_bst_create_tree`: lay out one layer from `idx`, returning the
/// next free index. The arithmetic is the script's, step for step.
fn create_tree(index: Layer, bst: &mut BTreeMap<usize, Node>, mut idx: usize) -> usize {
    let mut index = index.0;
    let mut keys: Vec<u8> = index.keys().copied().collect();
    let mut optv: Vec<(Vec<u8>, usize)> = Vec::new();
    let mut inr: Option<usize> = None;
    while !keys.is_empty() {
        let mid = keys.len() / 2;
        let (key, left_keys, right_keys) =
            (keys[mid], keys[..mid].to_vec(), keys[mid + 1..].to_vec());
        let mut idx_left = if left_keys.is_empty() { 0 } else { idx + 2 };
        let idx_right = if right_keys.is_empty() { 0 } else { idx + 1 };
        if idx_right == 0 && idx_left != 0 {
            idx_left -= 1;
        }
        let at = match inr {
            Some(parent) => bst[&parent].right,
            None => idx,
        };
        let entry = index.remove(&key).expect("a key of this layer");
        assert!(entry.values.len() < 2, "double values");
        bst.insert(
            at,
            Node {
                key,
                left: idx_left,
                right: idx_right,
                next: 0,
                value: entry.values.into_iter().next(),
                pending: Some(entry.next),
            },
        );
        if !right_keys.is_empty() {
            optv.push((right_keys, at));
        }
        if idx_left != 0 {
            idx = idx_left;
        }
        inr = None;
        keys = left_keys;
        if keys.is_empty() {
            let Some((k, parent)) = optv.pop() else { break };
            keys = k;
            inr = Some(parent);
        }
    }
    idx + 1
}

/// `entities_bst_create`: every layer, depth first.
fn create_bst(index: Layer) -> BTreeMap<usize, Node> {
    let mut bst = BTreeMap::new();
    bst.insert(
        0,
        Node {
            key: 0,
            left: 0,
            right: 0,
            next: 0,
            value: None,
            pending: None,
        },
    );
    let mut begin = 1;
    let mut end = create_tree(index, &mut bst, begin);
    let mut idx = end;
    let mut stack = Vec::new();
    loop {
        while begin < end {
            let node = bst.get_mut(&begin).expect("a laid-out node");
            match node.pending.take() {
                Some(layer) if !layer.0.is_empty() => {
                    node.next = idx;
                    stack.push((begin + 1, end));
                    begin = idx;
                    idx = create_tree(layer, &mut bst, idx);
                    end = idx;
                }
                _ => begin += 1,
            }
        }
        let Some((b, e)) = stack.pop() else { break };
        begin = b;
        end = e;
    }
    bst
}

/// One node of the entity search tree, owned.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SbstOwned {
    pub key: u8,
    pub value: Option<Vec<u8>>,
    pub left: usize,
    pub right: usize,
    pub next: usize,
}

/// The search tree of the pinned `entities.json`.
pub(crate) fn build_entities() -> Vec<SbstOwned> {
    let text = std::fs::read_to_string(data("whatwg/entities.json"))
        .expect("entities.json - run `ruby spec/conformance/parser_data.rb`");
    let json = super::json::parse(&text).expect("entities.json parses");
    let super::json::Json::Obj(map) = json else {
        panic!("entities.json is an object")
    };
    let mut index = Layer::default();
    for (name, v) in &map {
        let chars = v
            .get("characters")
            .and_then(|c| c.as_str())
            .expect("characters")
            .bytes
            .clone();
        let name = name.as_bytes();
        let mut layer = &mut index;
        for (pos, &c) in name.iter().enumerate().skip(1) {
            let e = layer.0.entry(c).or_default();
            if pos == name.len() - 1 {
                e.values.push(chars.clone());
            }
            layer = &mut e.next;
        }
    }
    let bst = create_bst(index);
    let n = bst.keys().last().copied().unwrap_or(0) + 1;
    assert_eq!(bst.len(), n, "the tree's indices are contiguous");
    bst.into_values()
        .map(|n| SbstOwned {
            key: n.key,
            value: n.value,
            left: n.left,
            right: n.right,
            next: n.next,
        })
        .collect()
}

/// `entities.rs` for the search tree `table`.
pub(crate) fn generate_entities(table: &[SbstOwned]) -> String {
    let mut out = String::from(
        "//! GENERATED by `conformance::res_gen` from the WHATWG entities.json - do not\n\
         //! edit. The named character references as a static search tree (the\n\
         //! layout of Lexbor's `html/tokenizer/res.h`), one byte per layer after\n\
         //! the `&`, with the replacements in a table of their own.\n\n\
         use super::SbstEntry;\n\n\
         const fn e(key: u8, value: u16, left: u16, right: u16, next: u16) -> SbstEntry {\n    \
         SbstEntry { key, value, left, right, next }\n}\n\n",
    );
    let values: Vec<&Vec<u8>> = table.iter().filter_map(|t| t.value.as_ref()).collect();
    let _ = writeln!(
        out,
        "pub static ENTITY_VALUES: [&[u8]; {}] = [",
        values.len()
    );
    for v in &values {
        let _ = writeln!(out, "    {},", lit(v));
    }
    out.push_str("];\n\n");
    let _ = writeln!(out, "pub static ENTITIES: [SbstEntry; {}] = [", table.len());
    let mut next_value = 0usize;
    for t in table {
        let v = if t.value.is_some() {
            next_value += 1;
            next_value
        } else {
            0
        };
        let _ = writeln!(
            out,
            "    e(0x{:02x}, {v}, {}, {}, {}),",
            t.key, t.left, t.right, t.next
        );
    }
    out.push_str("];\n");
    out
}

/// The checked-in `entities.rs` is what `generate_entities` makes of
/// `table`, without writing it - for an oracle other than entities.json
/// (Lexbor's table, `lexbor::res_tests`).
#[cfg(feature = "lexbor")]
pub(crate) fn entities_file_is(table: &[SbstOwned]) -> bool {
    let path = root().join("src/html/res/entities.rs");
    std::fs::read_to_string(path).unwrap_or_default() == generate_entities(table)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "needs the fetched HTML Standard source; run by `rake conformance:parser`"]
    fn tree_tables_are_the_standards() {
        check_or_write("tree.rs", &generate_tree_tables(&spec()));
    }

    #[test]
    #[ignore = "needs entities.json; run by `rake conformance:parser`"]
    fn entities_are_the_whatwg_list() {
        check_or_write("entities.rs", &generate_entities(&build_entities()));
    }

    /// The reading rules hold on the Standard as pinned: a commented-out row
    /// is dropped, a namespace word across a line break is read, `(none)` is
    /// empty.
    #[test]
    #[ignore = "needs the fetched HTML Standard source; run by `rake conformance:parser`"]
    fn the_standard_is_read_as_written() {
        let src = spec();
        let cats = categories(&src);
        assert!(cats.special.contains(&(res::NS_MATH, "ms".into())));
        assert!(!cats.special.contains(&(res::NS_HTML, "image".into())));
        assert!(cats.scope.contains(&(res::NS_HTML, "select".into())));
        assert_eq!(cats.formatting.len(), 14);
        let tags = table_after(
            &src,
            "token's tag name is one of the ones in the first column of the following table, change the tag",
        );
        assert!(tags.iter().all(|r| r[0] != "solidcolor"));
        let foreign = table_after(&src, "<dfn>adjust foreign attributes</dfn>");
        assert!(foreign.iter().any(|r| r[0] == "xmlns" && r[1].is_empty()));
    }
}
