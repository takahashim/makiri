//! Lexbor's reader for the parser conformance harness (`crate::conformance`),
//! and the tree-construction tests run through it.
//!
//! Lexbor is the parser Makiri ships today and the reference the port is
//! compared against, so it is read into the neutral [`Tree`] exactly as any
//! other parser will be. The tests here check that reader and record where
//! Lexbor stands against the pinned WPT data: every test passes or is listed in
//! `spec/conformance/parser_known_divergences.txt`.

#![allow(unsafe_code)]
// Test-only: the harness builds owned trees and sorted lists with std's
// infallible allocation, which `clippy.toml` bans for the engine so that an
// OOM there raises. Aborting a test process on OOM is fine.
#![allow(clippy::disallowed_methods)]

use crate::conformance::dat::{self, DatTest, Scripting};
use crate::conformance::tree::{self, Attr, Element, Node, Quirks, Tree, MATHML_NS, SVG_NS};
use crate::conformance::{known, tree_construction_dir};
use crate::lexbor::adapter::html::{HtmlNode, NsId};
use crate::lexbor::adapter::post_parse::{empty_html_document, parse_html};
use crate::lexbor::adapter::tree_guard::DepthLimit;
use crate::lexbor::fragment::{FragmentContext, FragmentTag, TransientFragment};
use crate::node_type::NodeType;

/// Parse `input` as a document and read it.
pub fn parse_document(input: &[u8]) -> Tree {
    let parsed = parse_html(input, false, DepthLimit::UNLIMITED).expect("Lexbor parses a document");
    // SAFETY: `parsed` owns the document and outlives every handle read here.
    let doc = unsafe { parsed.raw_doc().as_doc() };
    let quirks = match doc.compat_mode() {
        0 => Quirks::NoQuirks,
        1 => Quirks::Quirks,
        _ => Quirks::LimitedQuirks,
    };
    Tree {
        quirks: Some(quirks),
        children: doc.as_node().children().map(read).collect(),
    }
}

/// Parse `input` as a fragment in `context` (`td`, or `svg desc` for a
/// foreign context element) and read the nodes it produced.
pub fn parse_fragment(input: &[u8], context: &str) -> Tree {
    let host = empty_html_document(0).expect("Lexbor makes a document");
    // SAFETY: `host` owns the document and outlives the parse and the reads.
    let doc = unsafe { host.raw_doc().as_doc() };
    let (ns, name) = match context.split_once(' ') {
        Some(("svg", local)) => (NsId::SVG, local),
        Some(("math", local)) => (NsId::MATH, local),
        Some((ns, _)) => panic!("unknown fragment context namespace {ns:?}"),
        None => (NsId::HTML, context),
    };
    /* A name Lexbor's static table lacks (`svg g`) is a context with no
     * special meaning to the tree builder; intern it in the host document. */
    let lower = name.to_ascii_lowercase();
    let tag = doc
        .tag_id(lower.as_bytes())
        .or_else(|| doc.intern_tag(lower.as_bytes()))
        .unwrap_or_else(|| panic!("Lexbor could not intern tag {name:?}"));
    let ctx = FragmentContext::Tag {
        doc: host.raw_doc(),
        at: FragmentTag { tag, ns: Some(ns) },
    };
    // SAFETY: `host` is live for the call and the input is only read.
    let frag = unsafe { TransientFragment::parse(input, false, &ctx, DepthLimit::UNLIMITED) }
        .expect("Lexbor parses a fragment");
    // SAFETY: `frag` owns the parsed nodes and outlives the reads.
    let root = unsafe { frag.root().as_node() };
    Tree {
        quirks: None,
        children: root.children().map(read).collect(),
    }
}

fn bytes(b: &[u8]) -> Vec<u8> {
    b.to_vec()
}

/// `prefix:local` split at the first colon.
fn split_qname(q: &[u8]) -> (Option<Vec<u8>>, Vec<u8>) {
    match q.iter().position(|&c| c == b':') {
        Some(i) => (Some(q[..i].to_vec()), q[i + 1..].to_vec()),
        None => (None, q.to_vec()),
    }
}

fn read(n: HtmlNode<'_>) -> Node {
    match n.node_type() {
        NodeType::Element => {
            let el = n.element().expect("an element node is an element");
            let prefix = if n.has_prefix() {
                split_qname(el.qualified_name()).0
            } else {
                None
            };
            let attrs = el
                .attrs()
                .map(|a| match a.own_ns_uri() {
                    Some(uri) => {
                        let (prefix, _) = split_qname(a.qualified_name());
                        Attr {
                            ns: Some(bytes(uri)),
                            prefix,
                            local: bytes(a.dom_local_name()),
                            value: bytes(a.value()),
                        }
                    }
                    None => Attr {
                        ns: None,
                        prefix: None,
                        local: bytes(a.qualified_name()),
                        value: bytes(a.value()),
                    },
                })
                .collect();
            Node::Element(Element {
                ns: n.ns_uri().map(bytes),
                prefix,
                local: bytes(el.dom_local_name()),
                attrs,
                children: n.children().map(read).collect(),
                contents: n
                    .template_content()
                    .map(|c| c.children().map(read).collect()),
            })
        }
        NodeType::Text | NodeType::CDataSection => Node::Text(bytes(n.data().unwrap_or_default())),
        NodeType::Comment => Node::Comment(bytes(n.data().unwrap_or_default())),
        NodeType::Pi => Node::Pi {
            target: bytes(n.pi_target().unwrap_or_default()),
            data: bytes(n.data().unwrap_or_default()),
        },
        NodeType::DocumentType => Node::Doctype {
            name: bytes(n.node_name()),
            public_id: bytes(n.doctype_public_id().unwrap_or_default()),
            system_id: bytes(n.doctype_system_id().unwrap_or_default()),
        },
        other => panic!("a parser made a {other:?} node"),
    }
}

/// Run one `.dat` test through Lexbor: its html5lib dump, or `None` for a
/// scripting-on test (Makiri parses with scripting disabled).
pub fn run(t: &DatTest) -> Option<String> {
    if t.scripting == Scripting::On {
        return None;
    }
    let tree = match &t.fragment {
        Some(ctx) => parse_fragment(&t.data, ctx),
        None => parse_document(&t.data),
    };
    Some(tree::html5lib(&tree))
}

#[test]
fn reads_namespaces_attributes_and_template_contents() {
    let t = parse_document(b"<!DOCTYPE html><svg viewBox='0 0 1 1'><a xlink:href=x></a></svg><template><b>t</b></template><?pi data?>");
    assert_eq!(t.quirks, Some(Quirks::NoQuirks));
    let dump = tree::full(&t);
    assert!(dump.contains("<svg svg>"), "{dump}");
    assert!(dump.contains("@- viewBox=\"0 0 1 1\""), "{dump}");
    assert!(dump.contains("@xlink xlink:href=\"x\""), "{dump}");
    assert!(dump.contains("#contents\n|         <html b>"), "{dump}");
    assert!(dump.contains("#pi \"pi\" \"data\""), "{dump}");
    let Node::Element(html) = &t.children[1] else {
        panic!("{dump}")
    };
    assert_eq!(html.ns.as_deref(), Some(crate::conformance::tree::HTML_NS));
}

#[test]
fn reads_quirks_mode_and_foreign_fragment_contexts() {
    assert_eq!(parse_document(b"<p>").quirks, Some(Quirks::Quirks));
    assert_eq!(
        parse_document(b"<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Transitional//EN\">").quirks,
        Some(Quirks::LimitedQuirks)
    );
    let f = parse_fragment(b"<circle></circle>x", "svg desc");
    /* `desc` is an HTML integration point: a tag in it is HTML again. */
    assert_eq!(tree::html5lib(&f), "| <circle>\n| \"x\"");
    let f = parse_fragment(b"<circle/>", "svg g");
    let Node::Element(e) = &f.children[0] else {
        panic!()
    };
    assert_eq!(e.ns.as_deref(), Some(SVG_NS));
    let f = parse_fragment(b"<mi>", "math math");
    let Node::Element(e) = &f.children[0] else {
        panic!()
    };
    assert_eq!(e.ns.as_deref(), Some(MATHML_NS));
}

/// Every WPT tree-construction test, through Lexbor. Needs the fetched data
/// (`rake conformance:parser` fetches it and runs this).
#[test]
#[ignore = "needs the fetched WPT data; run by `rake conformance:parser`"]
fn wpt_tree_construction() {
    let dir = tree_construction_dir();
    let tests = dat::read_dir(&dir).unwrap_or_else(|| {
        panic!(
            "no .dat files in {} - run `rake conformance:parser`",
            dir.display()
        )
    });
    let known = known::load();
    let (mut pass, mut skipped, mut known_hit) = (0, 0, 0);
    let mut failures = Vec::new();
    let mut now_passing = Vec::new();
    for t in &tests {
        let Some(actual) = run(t) else {
            skipped += 1;
            continue;
        };
        let listed = known.get(&t.key());
        match (actual == t.document, listed) {
            (true, None) => pass += 1,
            (true, Some(id)) => now_passing.push(format!("{} ({id})", t.key())),
            (false, Some(_)) => known_hit += 1,
            (false, None) => failures.push(format!(
                "{}{}\n--- input ---\n{}\n--- expected ---\n{}\n--- actual ---\n{}",
                t.key(),
                t.fragment
                    .as_ref()
                    .map(|c| format!(" (context {c})"))
                    .unwrap_or_default(),
                String::from_utf8_lossy(&t.data),
                t.document,
                actual
            )),
        }
    }
    eprintln!(
        "WPT tree construction through Lexbor: {} tests, {pass} pass, {known_hit} known, \
         {skipped} scripting-on skipped, {} fail, {} now passing",
        tests.len(),
        failures.len(),
        now_passing.len()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    assert!(
        now_passing.is_empty(),
        "listed as known but now passing - remove from the list and the table: {now_passing:?}"
    );
    assert_eq!(
        known_hit,
        known.len(),
        "a listed test was not found in the data"
    );
}

/* ------------------------------------------------------------------ *
 * the tokenizer on its own                                            *
 * ------------------------------------------------------------------ */

use crate::caught::PanicLatch;
use crate::conformance::tokenizer_dir;
use crate::conformance::tokens::{self as tok, State, Token, TokenizerTest};
use crate::lexbor::abi as lxb;
use core::ffi::c_void;

/// What the token callback fills.
struct Collect {
    tokens: Vec<Token>,
    positions: Vec<(Option<usize>, Option<usize>)>,
    /// The input, to turn Lexbor's pointers into offsets.
    base: *const u8,
    len: usize,
    panic: PanicLatch,
}

impl Collect {
    /// A Lexbor position as an offset into the input: one in the input (or at
    /// its end) as itself, one in Lexbor's end-of-file buffer as the input's
    /// length - where the port puts its virtual end-of-file byte.
    fn offset(&self, p: *const u8) -> Option<usize> {
        if p.is_null() {
            return None;
        }
        let (p, base) = (p as usize, self.base as usize);
        Some(if p >= base && p <= base + self.len {
            p - base
        } else {
            self.len
        })
    }
}

/// `len` bytes at `p`, or nothing for a null pointer.
///
/// # Safety
/// A non-null `p` points at `len` readable bytes for the returned lifetime.
unsafe fn slice<'a>(p: *const u8, len: usize) -> &'a [u8] {
    if p.is_null() || len == 0 {
        &[]
    } else {
        // SAFETY: the caller's contract.
        unsafe { core::slice::from_raw_parts(p, len) }
    }
}

/// `[start, end)` of a token's text.
///
/// # Safety
/// Both null, or both into one live buffer with `start <= end`.
unsafe fn span<'a>(start: *const u8, end: *const u8) -> &'a [u8] {
    if start.is_null() {
        return &[];
    }
    // SAFETY: the caller's contract.
    unsafe { slice(start, end.offset_from(start) as usize) }
}

/// A Lexbor token as an html5lib one; `None` for end-of-file.
///
/// # Safety
/// `token` is the tokenizer's current token, inside its token-done callback.
unsafe fn read_token(token: *const lxb::lxb_html_token_t) -> Option<Token> {
    use lxb::*;
    // SAFETY: the function's contract; every pointer read below is the
    // token's own, live for the callback.
    unsafe {
        let t = &*token;
        let text = span(t.text_start, t.text_end);
        let tag = t.tag_id;
        let tag_is = |id: lxb_tag_id_enum_t| tag == id as lxb_tag_id_t;
        if tag_is(lxb_tag_id_enum_t_LXB_TAG__END_OF_FILE) {
            return None;
        }
        if tag_is(lxb_tag_id_enum_t_LXB_TAG__TEXT) {
            return Some(Token::Character(text.to_vec()));
        }
        if tag_is(lxb_tag_id_enum_t_LXB_TAG__EM_COMMENT) {
            return Some(Token::Comment(text.to_vec()));
        }
        if tag_is(lxb_tag_id_enum_t_LXB_TAG__PROCESSINGINSTRUCTION) {
            /* `null_count` is the target's length here (see `lxb_html_token_t`). */
            let at = t.null_count.min(text.len());
            return Some(Token::Pi {
                target: text[..at].to_vec(),
                data: text[at..].to_vec(),
            });
        }
        let attr_name = |a: *mut lxb_html_token_attr_t| {
            let mut len = 0;
            let p = lxb_html_token_attr_name(a, &mut len);
            slice(p, len).to_vec()
        };
        let attr_value = |a: &lxb_html_token_attr_t| slice(a.value, a.value_size).to_vec();
        /* A DOCTYPE id Lexbor saw the keyword for but no identifier after
         * keeps a null value: missing, as Lexbor's own harness reads it. */
        let id_value = |a: &lxb_html_token_attr_t| (!a.value.is_null()).then(|| attr_value(a));
        if tag_is(lxb_tag_id_enum_t_LXB_TAG__EM_DOCTYPE) {
            /* As `lxb_html_token_doctype_parse` reads it: the name, then an
             * optional PUBLIC or SYSTEM attribute, then the system id. */
            let first = t.attr_first;
            let name =
                (!first.is_null() && !(*first).name_begin.is_null()).then(|| attr_name(first));
            let (mut public_id, mut system_id) = (None, None);
            let second = if first.is_null() {
                first
            } else {
                (*first).next
            };
            if !second.is_null() {
                let id = (*(*second).name).attr_id;
                if id == lxb_dom_attr_id_enum_t_LXB_DOM_ATTR_PUBLIC as usize {
                    public_id = id_value(&*second);
                    let third = (*second).next;
                    if !third.is_null() {
                        system_id = id_value(&*third);
                    }
                } else if id == lxb_dom_attr_id_enum_t_LXB_DOM_ATTR_SYSTEM as usize {
                    system_id = id_value(&*second);
                }
            }
            let force_quirks = t.type_
                & lxb_html_token_type_LXB_HTML_TOKEN_TYPE_FORCE_QUIRKS as lxb_html_token_type_t
                != 0;
            return Some(Token::Doctype {
                name,
                public_id,
                system_id,
                force_quirks,
            });
        }
        let mut len = 0;
        let name = slice(lxb_tag_name_by_id_noi(tag, &mut len), len).to_vec();
        if t.type_ & lxb_html_token_type_LXB_HTML_TOKEN_TYPE_CLOSE as lxb_html_token_type_t != 0 {
            return Some(Token::EndTag { name });
        }
        let mut attrs = Vec::new();
        let mut a = t.attr_first;
        while !a.is_null() {
            attrs.push((attr_name(a), attr_value(&*a)));
            a = (*a).next;
        }
        Some(Token::StartTag {
            name,
            attrs,
            self_closing: t.type_
                & lxb_html_token_type_LXB_HTML_TOKEN_TYPE_CLOSE_SELF as lxb_html_token_type_t
                != 0,
        })
    }
}

/// # Safety
/// Called by Lexbor's tokenizer only, with `ctx` the live [`Collect`]
/// [`tokenize`] registered.
unsafe extern "C" fn collect_cb(
    _tkz: *mut lxb::lxb_html_tokenizer_t,
    token: *mut lxb::lxb_html_token_t,
    ctx: *mut c_void,
) -> *mut lxb::lxb_html_token_t {
    // SAFETY: the function's contract.
    let c = unsafe { &mut *(ctx as *mut Collect) };
    if c.panic.caught() {
        return core::ptr::null_mut();
    }
    let (begin, end) = {
        // SAFETY: the tokenizer's current token, live for the callback.
        let tk = unsafe { &*token };
        (c.offset(tk.begin), c.offset(tk.end))
    };
    let Collect {
        tokens,
        positions,
        panic,
        ..
    } = c;
    panic.guard(core::ptr::null_mut(), || {
        // SAFETY: inside the callback, on the tokenizer's current token.
        if let Some(t) = unsafe { read_token(token) } {
            tokens.push(t);
            positions.push((begin, end));
        }
        /* The same token back: the tokenizer cleans and reuses it. */
        token
    })
}

/// Owns a standalone tokenizer and its name tables.
struct Tkz(*mut lxb::lxb_html_tokenizer_t);

impl Drop for Tkz {
    fn drop(&mut self) {
        // SAFETY: made by `tokenize`, destroyed once, in Lexbor's own order.
        unsafe {
            lxb::lxb_html_tokenizer_tags_destroy(self.0);
            lxb::lxb_html_tokenizer_attrs_destroy(self.0);
            lxb::lxb_html_tokenizer_destroy(self.0);
        }
    }
}

/// Run Lexbor's tokenizer alone over `input` from `state`, as Lexbor's own
/// html5lib harness sets it up, with `last_start_tag` as the tag an end tag
/// must match to be appropriate.
pub fn tokenize(input: &[u8], state: State, last_start_tag: Option<&[u8]>) -> Vec<Token> {
    tokenize_traced(input, state, last_start_tag).tokens
}

/// [`tokenize`], with positions and parse errors, in the port's `Trace`.
pub fn tokenize_traced(
    input: &[u8],
    state: State,
    last_start_tag: Option<&[u8]>,
) -> crate::html::tokenizer::tests::Trace {
    use lxb::*;
    let mut collect = Collect {
        tokens: Vec::new(),
        positions: Vec::new(),
        base: input.as_ptr(),
        len: input.len(),
        panic: PanicLatch::default(),
    };
    let mut errors = Vec::new();
    // SAFETY: a tokenizer this function owns (`Tkz`), driven start to end
    // here; the callback's context is `collect`, which outlives the run.
    unsafe {
        let tkz = Tkz(lxb_html_tokenizer_create());
        assert!(!tkz.0.is_null(), "Lexbor makes a tokenizer");
        assert_eq!(lxb_html_tokenizer_init(tkz.0), 0, "tokenizer init");
        assert_eq!(lxb_html_tokenizer_tags_make(tkz.0, 128), 0, "tag table");
        assert_eq!(lxb_html_tokenizer_attrs_make(tkz.0, 128), 0, "attr table");
        lxb_html_tokenizer_callback_token_done_set_noi(
            tkz.0,
            Some(collect_cb),
            &mut collect as *mut Collect as *mut c_void,
        );
        let last = last_start_tag
            .map(|n| lxb_tag_id_by_name_noi((*tkz.0).tags, n.as_ptr(), n.len()))
            .filter(|&id| id != lxb_tag_id_enum_t_LXB_TAG__UNDEF as lxb_tag_id_t);
        let (f, wants_last): (lxb_html_tokenizer_state_f, bool) = match state {
            State::Data => (Some(lxb_html_tokenizer_state_data_before), false),
            State::Plaintext => (Some(lxb_html_tokenizer_state_plaintext_before), false),
            State::Rcdata => (Some(lxb_html_tokenizer_state_rcdata_before), true),
            State::Rawtext => (Some(lxb_html_tokenizer_state_rawtext_before), true),
            State::ScriptData => (Some(lxb_html_tokenizer_state_script_data_before), true),
            State::CdataSection => (Some(lxb_html_tokenizer_state_cdata_section_before), false),
        };
        (*tkz.0).state = f;
        if let (true, Some(id)) = (wants_last, last) {
            (*tkz.0).tmp_tag_id = id;
        }
        assert_eq!(lxb_html_tokenizer_begin(tkz.0), 0, "tokenizer begin");
        let st = lxb_html_tokenizer_chunk(tkz.0, input.as_ptr(), input.len());
        let end = lxb_html_tokenizer_end(tkz.0);
        let list = (*tkz.0).parse_errors;
        if !list.is_null() {
            let l = &*list;
            for i in 0..l.length {
                let e = &*(l.list.add(i * l.struct_size) as *const lxb_html_tokenizer_error_t);
                errors.push((e.id, collect.offset(e.pos)));
            }
        }
        drop(tkz);
        collect.panic.resume();
        assert_eq!((st, end), (0, 0), "tokenizer status");
    }
    crate::html::tokenizer::tests::Trace {
        tokens: collect.tokens,
        positions: collect.positions,
        errors,
    }
}

#[test]
fn tokenizes_from_each_initial_state() {
    let toks = tokenize(
        b"<a href='x&amp;y' b>t</a><!--c--><?p q?>",
        State::Data,
        None,
    );
    assert_eq!(
        toks,
        vec![
            Token::StartTag {
                name: b"a".to_vec(),
                attrs: vec![(b"href".to_vec(), b"x&y".to_vec()), (b"b".to_vec(), vec![])],
                self_closing: false
            },
            Token::Character(b"t".to_vec()),
            Token::EndTag {
                name: b"a".to_vec()
            },
            Token::Comment(b"c".to_vec()),
            Token::Pi {
                target: b"p".to_vec(),
                data: b"q".to_vec()
            },
        ]
    );
    let raw = tok::coalesce(&tokenize(
        b"<b>x</title></b>",
        State::Rcdata,
        Some(b"title"),
    ));
    assert_eq!(
        raw,
        vec![
            Token::Character(b"<b>x".to_vec()),
            Token::EndTag {
                name: b"title".to_vec()
            },
            /* An appropriate end tag returns the tokenizer to the data state. */
            Token::EndTag {
                name: b"b".to_vec()
            },
        ]
    );
}

/// Every html5lib tokenizer test, through Lexbor's tokenizer, from each of
/// the test's initial states.
#[test]
#[ignore = "needs the fetched html5lib-tests data; run by `rake conformance:parser`"]
fn html5lib_tokenizer() {
    let dir = tokenizer_dir();
    let tests: Vec<TokenizerTest> = tok::read_dir(&dir).unwrap_or_else(|| {
        panic!(
            "no .test files in {} - run `rake conformance:parser`",
            dir.display()
        )
    });
    let known = known::load();
    let (mut pass, mut known_hit, mut skipped) = (0, 0, 0);
    let mut failures = Vec::new();
    let mut now_passing = Vec::new();
    for t in &tests {
        if t.unrepresentable {
            skipped += 1;
            continue;
        }
        let mut bad = Vec::new();
        for &state in &t.states {
            let actual = tokenize(&t.input, state, t.last_start_tag.as_deref());
            if !tok::html5lib_eq(&actual, &t.output) {
                bad.push(format!(
                    "  {state:?}:\n    expected {:?}\n    actual   {:?}",
                    tok::coalesce(&t.output),
                    tok::coalesce(&actual)
                ));
            }
        }
        match (bad.is_empty(), known.get(&t.key())) {
            (true, None) => pass += 1,
            (true, Some(id)) => now_passing.push(format!("{} ({id})", t.key())),
            (false, Some(_)) => known_hit += 1,
            (false, None) => failures.push(format!(
                "{} {:?}\n  input {:?}\n{}",
                t.key(),
                t.description,
                String::from_utf8_lossy(&t.input),
                bad.join("\n")
            )),
        }
    }
    eprintln!(
        "html5lib tokenizer through Lexbor: {} tests, {pass} pass, {known_hit} known, \
         {skipped} unrepresentable skipped, {} fail, {} now passing",
        tests.len(),
        failures.len(),
        now_passing.len()
    );
    assert!(
        failures.is_empty(),
        "{} failing:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    assert!(
        now_passing.is_empty(),
        "listed as known but now passing - remove from the list and the table: {now_passing:?}"
    );
}

/* ------------------------------------------------------------------ *
 * the port's tokenizer against Lexbor's                               *
 * ------------------------------------------------------------------ */

/// Compare the port with Lexbor token for token - boundaries, attribute order
/// and all - on one input from one state; the first difference, if any.
fn tokenizer_difference(input: &[u8], state: State, last: Option<&[u8]>) -> Option<String> {
    let lexbor = tokenize_traced(input, state, last);
    let port = crate::html::tokenizer::tests::tokenize_traced(input, state, last);
    (lexbor != port).then(|| {
        format!(
            "input {:?} from {state:?} (last start tag {:?})\n  lexbor {lexbor:?}\n  port   {port:?}",
            String::from_utf8_lossy(input),
            last.map(String::from_utf8_lossy)
        )
    })
}

/// Every html5lib tokenizer input, from every state the test names.
#[test]
#[ignore = "needs the fetched html5lib-tests data; run by `rake conformance:parser`"]
fn port_tokenizer_matches_lexbor_on_html5lib_inputs() {
    let tests: Vec<TokenizerTest> =
        tok::read_dir(&tokenizer_dir()).expect("html5lib tokenizer tests");
    let mut diffs = Vec::new();
    for t in tests.iter().filter(|t| !t.unrepresentable) {
        for &state in &t.states {
            if let Some(d) = tokenizer_difference(&t.input, state, t.last_start_tag.as_deref()) {
                diffs.push(format!("{}: {d}", t.key()));
            }
        }
    }
    assert!(
        diffs.is_empty(),
        "{} differ:\n{}",
        diffs.len(),
        diffs
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Inputs assembled at random from fragments that reach every state's edge
/// cases (CR/LF pairs, NUL, references cut short, keywords cut short, end tags
/// in raw text), from every initial state.
#[test]
fn port_tokenizer_matches_lexbor_on_random_inputs() {
    const PIECES: &[&[u8]] = &[
        b"<",
        b">",
        b"/",
        b"</",
        b"<!",
        b"<!-",
        b"<!--",
        b"-->",
        b"--!>",
        b"-",
        b"!",
        b"?",
        b"<?",
        b"?>",
        b"<?xml",
        b"<?php",
        b" ",
        b"\t",
        b"\n",
        b"\r",
        b"\r\n",
        b"\x0c",
        b"\0",
        b"a",
        b"Z",
        b"x",
        b"1",
        b"=",
        b"\"",
        b"'",
        b"`",
        b"&",
        b"&amp",
        b"&amp;",
        b"&not",
        b"&noti;",
        b"&#",
        b"&#x",
        b"&#X41;",
        b"&#65",
        b"&#0;",
        b"&#x110000;",
        b"&#xD800;",
        b"&#128;",
        b";",
        b"<a",
        b"<a b",
        b"<a b=c",
        b"<a b='c'",
        b"<a b=\"c\"",
        b"<A B=C D>",
        b"</a>",
        b"</title>",
        b"</script>",
        b"</style>",
        b"<script>",
        b"<!--<script>",
        b"</scrIpt",
        b"<!DOCTYPE",
        b"<!doctype html",
        b" PUBLIC",
        b" SYSTEM",
        b" \"pub\"",
        b" 'sys'",
        b"<![CDATA[",
        b"]]>",
        b"]",
        b"\xc3\xa9",
        b"\xef\xbf\xbd",
        b"<b/>",
        b"<br/ >",
        b"<p a=1 a=2>",
        b"x=",
        b"title",
    ];
    let states = [
        State::Data,
        State::Plaintext,
        State::Rcdata,
        State::Rawtext,
        State::ScriptData,
        State::CdataSection,
    ];
    let lasts: [Option<&[u8]>; 4] = [None, Some(b"title"), Some(b"script"), Some(b"a")];
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut diffs = Vec::new();
    for _ in 0..20_000 {
        let len = (next() % 12) as usize;
        let mut input = Vec::new();
        for _ in 0..len {
            input.extend_from_slice(PIECES[(next() % PIECES.len() as u64) as usize]);
        }
        let state = states[(next() % states.len() as u64) as usize];
        let last = lasts[(next() % lasts.len() as u64) as usize];
        if let Some(d) = tokenizer_difference(&input, state, last) {
            diffs.push(d);
            if diffs.len() >= 10 {
                break;
            }
        }
    }
    assert!(diffs.is_empty(), "{}", diffs.join("\n"));
}

/// The traces the differential compares do carry errors and positions.
#[test]
fn tokenizer_traces_carry_errors_and_positions() {
    let input = b"<a b=1 b=2>x\0&noti;</a/>";
    let lexbor = tokenize_traced(input, State::Data, None);
    assert!(lexbor.errors.len() >= 3, "{lexbor:?}");
    assert_eq!(lexbor.positions[0], (Some(1), Some(2)), "{lexbor:?}");
    assert_eq!(
        lexbor,
        crate::html::tokenizer::tests::tokenize_traced(input, State::Data, None)
    );
}

/// The document `bench/bench.rb` builds, with `items` list items.
fn bench_html(items: usize) -> Vec<u8> {
    let mut rows = String::new();
    for i in 1..=items {
        rows.push_str(&format!(
            "<li class=\"item r{}\" data-id=\"{i}\" data-rank=\"{}\"><a href=\"/p/{i}\" class=\"link\">item {i}</a><span class=\"meta\">tag{} &amp; more</span></li>\n",
            i % 7,
            i % 100,
            i % 13
        ));
    }
    format!(
        "<!doctype html><html><head><title>bench</title>\n<meta charset=\"utf-8\"></head>\n<body>\n<header id=\"top\"><nav><a href=\"/\">home</a><a href=\"/about\">about</a></nav></header>\n<main id=\"main\" class=\"container\">\n<section class=\"list\"><ul>{rows}</ul></section>\n</main>\n<footer id=\"bot\"><p>&copy; bench</p></footer>\n</body></html>\n"
    )
    .into_bytes()
}

/// Lexbor's tokenizer and the port's over the bench document, timed. Run with
/// `cargo test --release ... tokenizer_speed -- --ignored --nocapture`.
#[test]
#[ignore = "a measurement, not a check"]
fn tokenizer_speed() {
    let inputs: Vec<(&str, Vec<u8>)> = vec![
        ("bench document", bench_html(20_000)),
        (
            "plain text",
            "lorem ipsum dolor sit amet ".repeat(100_000).into_bytes(),
        ),
        ("bare tags", "<p><b></b></p>".repeat(200_000).into_bytes()),
        (
            "static attrs",
            "<a href=x class=y id=z>".repeat(100_000).into_bytes(),
        ),
        (
            "custom names",
            "<my-el data-a=x data-b=y>".repeat(100_000).into_bytes(),
        ),
        (
            "references",
            "a&amp;b&lt;c&#65;".repeat(200_000).into_bytes(),
        ),
        ("named refs", "a&amp;b&lt;".repeat(300_000).into_bytes()),
        ("numeric refs", "a&#65;b&#x42;".repeat(300_000).into_bytes()),
        (
            "long named",
            "&CounterClockwiseContourIntegral;"
                .repeat(100_000)
                .into_bytes(),
        ),
        (
            "long named in tags",
            "<b>&CounterClockwiseContourIntegral;</b>"
                .repeat(80_000)
                .into_bytes(),
        ),
        ("one big text", "abcdefgh&lt;".repeat(300_000).into_bytes()),
    ];
    for (label, input) in &inputs {
        tokenizer_speed_on(label, input);
    }
}

fn tokenizer_speed_on(label: &str, input: &[u8]) {
    use std::time::Instant;

    unsafe extern "C" fn count_cb(
        _tkz: *mut lxb::lxb_html_tokenizer_t,
        token: *mut lxb::lxb_html_token_t,
        ctx: *mut c_void,
    ) -> *mut lxb::lxb_html_token_t {
        // SAFETY: `ctx` is the counter the caller registered.
        unsafe { *(ctx as *mut usize) += 1 };
        token
    }
    struct Count(usize);
    impl crate::html::tokenizer::TokenSink for Count {
        fn token(
            &mut self,
            _t: &mut crate::html::tokenizer::Tokenizer<'_, '_>,
        ) -> Result<(), crate::html::tokenizer::Stop> {
            self.0 += 1;
            Ok(())
        }
    }
    let rounds = 15;
    let lexbor = || {
        let mut n = 0usize;
        // SAFETY: as `tokenize_traced`, with a counting callback.
        unsafe {
            let tkz = Tkz(lxb::lxb_html_tokenizer_create());
            lxb::lxb_html_tokenizer_init(tkz.0);
            lxb::lxb_html_tokenizer_tags_make(tkz.0, 128);
            lxb::lxb_html_tokenizer_attrs_make(tkz.0, 128);
            lxb::lxb_html_tokenizer_callback_token_done_set_noi(
                tkz.0,
                Some(count_cb),
                &mut n as *mut usize as *mut c_void,
            );
            lxb::lxb_html_tokenizer_begin(tkz.0);
            lxb::lxb_html_tokenizer_chunk(tkz.0, input.as_ptr(), input.len());
            lxb::lxb_html_tokenizer_end(tkz.0);
        }
        n
    };
    let port = || {
        let mut names = crate::html::names::Names::default();
        let mut sink = Count(0);
        {
            let mut t = crate::html::tokenizer::Tokenizer::new(input, &mut names, &mut sink);
            t.chunk();
            t.end();
        }
        sink.0
    };
    assert_eq!(lexbor(), port(), "the two count the same tokens");
    let time = |f: &dyn Fn() -> usize| {
        let mut best = f64::MAX;
        for _ in 0..rounds {
            let t0 = Instant::now();
            std::hint::black_box(f());
            best = best.min(t0.elapsed().as_secs_f64());
        }
        best
    };
    let (l, p) = (time(&lexbor), time(&port));
    let mb = input.len() as f64 / 1e6;
    eprintln!(
        "{label:>15} {:.1} MB: lexbor {:.2} ms ({:.0} MB/s), port {:.2} ms ({:.0} MB/s), port/lexbor {:.2}",
        mb,
        l * 1e3,
        mb / l,
        p * 1e3,
        mb / p,
        p / l
    );
}

/// A long loop of the port alone, for a sampling profiler to attach to.
#[test]
#[ignore = "a profiling target"]
fn tokenizer_profile_target() {
    struct Count(usize);
    impl crate::html::tokenizer::TokenSink for Count {
        fn token(
            &mut self,
            _t: &mut crate::html::tokenizer::Tokenizer<'_, '_>,
        ) -> Result<(), crate::html::tokenizer::Stop> {
            self.0 += 1;
            Ok(())
        }
    }
    let input = std::env::var("PROFILE_INPUT")
        .unwrap_or_else(|_| "&CounterClockwiseContourIntegral;".to_string())
        .repeat(100_000);
    let until = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while std::time::Instant::now() < until {
        let mut names = crate::html::names::Names::default();
        let mut sink = Count(0);
        let mut t = crate::html::tokenizer::Tokenizer::new(input.as_bytes(), &mut names, &mut sink);
        t.chunk();
        t.end();
    }
}

/// Lexbor's tokenizer in a long loop, for a sampling profiler.
#[test]
#[ignore = "a profiling target"]
fn tokenizer_profile_target_lexbor() {
    unsafe extern "C" fn count_cb(
        _tkz: *mut lxb::lxb_html_tokenizer_t,
        token: *mut lxb::lxb_html_token_t,
        _ctx: *mut c_void,
    ) -> *mut lxb::lxb_html_token_t {
        token
    }
    let input = std::env::var("PROFILE_INPUT")
        .unwrap_or_else(|_| "&CounterClockwiseContourIntegral;".to_string())
        .repeat(100_000);
    let until = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while std::time::Instant::now() < until {
        // SAFETY: as `tokenize_traced`, with a callback that keeps nothing.
        unsafe {
            let tkz = Tkz(lxb::lxb_html_tokenizer_create());
            lxb::lxb_html_tokenizer_init(tkz.0);
            lxb::lxb_html_tokenizer_tags_make(tkz.0, 128);
            lxb::lxb_html_tokenizer_attrs_make(tkz.0, 128);
            lxb::lxb_html_tokenizer_callback_token_done_set_noi(
                tkz.0,
                Some(count_cb),
                core::ptr::null_mut(),
            );
            lxb::lxb_html_tokenizer_begin(tkz.0);
            lxb::lxb_html_tokenizer_chunk(tkz.0, input.as_ptr(), input.len());
            lxb::lxb_html_tokenizer_end(tkz.0);
        }
    }
}

/* ------------------------------------------------------------------ *
 * the arena, filled from Lexbor                                       *
 * ------------------------------------------------------------------ */

/// Lexbor's tree and its copy in the arena, read the same way.
fn lexbor_and_arena(input: &[u8]) -> (Tree, Tree) {
    let parsed = parse_html(input, false, DepthLimit::UNLIMITED).expect("Lexbor parses a document");
    // SAFETY: `parsed` owns the document and outlives every handle read here.
    let doc = unsafe { parsed.raw_doc().as_doc() };
    let arena = crate::lexbor::adapter::arena_copy::copy_document(doc).expect("the copy");
    (
        parse_document(input),
        crate::html::dom::conformance::read_document(&arena),
    )
}

#[test]
fn the_arena_copy_is_the_same_tree() {
    let (l, a) = lexbor_and_arena(
        b"<!DOCTYPE html PUBLIC \"-//W3C//DTD HTML 4.01//EN\"><title>t</title>\
          <svg viewBox='0 0 1 1'><foreignObject><p id=x class=y>p</p></foreignObject>\
          <a xlink:href=z></a></svg><math><mi>x</mi></math><template><b>t</b></template>\
          <!--c--><?pi d?>",
    );
    assert_eq!(l, a, "\n{}\n---\n{}", tree::full(&l), tree::full(&a));
}

/// Every WPT document input, copied into the arena, reads back as Lexbor's.
#[test]
#[ignore = "needs the fetched WPT data; run by `rake conformance:parser`"]
fn the_arena_copy_matches_lexbor_on_wpt_inputs() {
    let tests = dat::read_dir(&tree_construction_dir()).expect("WPT data");
    let mut diffs = Vec::new();
    for t in tests
        .iter()
        .filter(|t| t.fragment.is_none() && t.scripting != Scripting::On)
    {
        let (l, a) = lexbor_and_arena(&t.data);
        if l != a {
            diffs.push(format!(
                "{}\n{}\n---\n{}",
                t.key(),
                tree::full(&l),
                tree::full(&a)
            ));
        }
    }
    assert!(
        diffs.is_empty(),
        "{} differ:\n{}",
        diffs.len(),
        diffs
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n\n")
    );
}

/// The bench document's footprint in Lexbor and in the arena copy.
#[test]
#[ignore = "a measurement, not a check"]
fn arena_footprint() {
    let input = bench_html(20_000);
    let parsed = parse_html(&input, false, DepthLimit::UNLIMITED).expect("parses");
    // SAFETY: `parsed` owns the document for the reads below.
    let doc = unsafe { parsed.raw_doc().as_doc() };
    let lexbor = crate::lexbor::adapter::arena_bytes::document_size(doc);
    let arena = crate::lexbor::adapter::arena_copy::copy_document(doc).expect("copy");
    let nodes = arena.node_count();
    eprintln!(
        "{} bytes in, {nodes} nodes (attributes included): lexbor {} B ({:.0} B/node), arena {} B ({:.0} B/node)",
        input.len(),
        lexbor.live,
        lexbor.live as f64 / nodes as f64,
        arena.owned_bytes(),
        arena.owned_bytes() as f64 / nodes as f64
    );
}
