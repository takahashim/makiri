//! The engine end to end, without Ruby: an XML document, an expression or a
//! selector, and the value that comes back.
//!
//! The Ruby specs reach the same paths through `Node#xpath` and `#css`; these
//! run under `cargo test` (and under ASan in CI) with nothing between the test
//! and the engine's front door. The expected values are what `Makiri::XML`
//! answered for the same document and expressions when these were written.

#![forbid(unsafe_code)]

use crate::engine_error::ErrorKind;

use crate::xpath::limits::Budget;

use crate::text::VerifiedText;
use crate::token::Token;
use crate::xml::tree::parse as xml_parse;
use crate::xml::{ArenaKind, NodeId};
use crate::xpath::ast::Ast;
use crate::xpath::ctx::{Context, Resolver, ResolverCall, XPathValue};
use crate::xpath::parse::parse_owned;

const DOC: &[u8] = br#"<r xmlns:d="urn:d"><a k="1">x</a><a k="2"> y  z </a><b><c/><c n="3"/><d:e>ne</d:e></b><!--cm--><?pi data?></r>"#;

#[derive(Debug)]
enum Answer {
    Nodes(Vec<String>),
    Str(String),
    Num(f64),
    Bool(bool),
    Err(ErrorKind),
}

impl PartialEq for Answer {
    fn eq(&self, other: &Answer) -> bool {
        match (self, other) {
            (Answer::Nodes(a), Answer::Nodes(b)) => a == b,
            (Answer::Str(a), Answer::Str(b)) => a == b,
            (Answer::Num(a), Answer::Num(b)) => (a.is_nan() && b.is_nan()) || a == b,
            (Answer::Bool(a), Answer::Bool(b)) => a == b,
            (Answer::Err(a), Answer::Err(b)) => a == b,
            _ => false,
        }
    }
}

fn nodes(names: &[&str]) -> Answer {
    Answer::Nodes(names.iter().map(|s| s.to_string()).collect())
}

fn text(s: &str) -> Answer {
    Answer::Str(s.to_string())
}

enum Query {
    XPath,
    #[cfg(feature = "lexbor")]
    Css,
}

/// Run `expr` against [`DOC`] from its document node, with `d` bound to
/// `urn:d`, and describe what came back. `tighten` adjusts the budgets first.
/// A node-set is also asked for through `evaluate_first`, which must agree on
/// its first node.
fn run(
    query: Query,
    expr: &str,
    tighten: impl FnOnce(&mut crate::xpath::limits::Limits),
) -> Answer {
    let doc = xml_parse(DOC).expect("the fixture parses");
    let mut ctx = crate::xml::xpath::context(&doc, doc.doc_node());
    ctx.register_ns(b"d", b"urn:d").expect("registered");
    tighten(ctx.limits_mut());

    let source = VerifiedText::from_bytes(expr.as_bytes()).expect("verified");
    let mut parse_budget = Budget::with_limits(ctx.limits());
    /* `source` borrows `expr`, which outlives the parse. */
    let compiled: Result<Box<Ast>, _> = match query {
        Query::XPath => parse_owned(source, &mut parse_budget),
        #[cfg(feature = "lexbor")]
        Query::Css => {
            let ns = crate::css::CssNs {
                default_namespace: false,
            };
            let gvl = crate::gvl::Gvl::exclusive();
            crate::css::compile_owned(
                &gvl,
                source,
                &ns,
                crate::css::Form::Select,
                &mut parse_budget,
            )
        }
    };
    let ast = match compiled {
        Ok(ast) => ast,
        Err(_) => return Answer::Err(parse_budget.take_error().status),
    };
    {
        let describe = |node: &Token| {
            let id = NodeId::from_token(node.as_ptr() as usize).expect("a node token");
            match doc.type_(id) {
                Some(ArenaKind::Text) | Some(ArenaKind::CDataSection) => "text".to_string(),
                Some(ArenaKind::Comment) => "comment".to_string(),
                Some(ArenaKind::Pi) => String::from_utf8_lossy(doc.local(id)).into_owned(),
                _ => String::from_utf8_lossy(doc.qname(id)).into_owned(),
            }
        };
        match ctx.evaluate(&ast, None) {
            Err(e) => Answer::Err(e.status),
            Ok(XPathValue::NodeSet(set)) => {
                let all: Vec<String> = set.as_slice().iter().map(describe).collect();
                if let Ok(XPathValue::NodeSet(one)) = ctx.evaluate_first(&ast, None) {
                    assert_eq!(
                        one.as_slice().first().map(describe),
                        all.first().cloned(),
                        "evaluate_first disagrees with evaluate for {expr}"
                    );
                }
                Answer::Nodes(all)
            }
            Ok(XPathValue::String(t)) => text(&String::from_utf8_lossy(t.as_slice())),
            Ok(XPathValue::Number(d)) => Answer::Num(d),
            Ok(XPathValue::Boolean(b)) => Answer::Bool(b),
        }
    }
}

fn xpath(expr: &str) -> Answer {
    run(Query::XPath, expr, |_| {})
}

#[test]
fn location_paths_select_in_document_order() {
    assert_eq!(xpath("/r/a"), nodes(&["a", "a"]));
    assert_eq!(xpath(r#"//a[@k="2"]"#), nodes(&["a"]));
    assert_eq!(xpath("//a[2]"), nodes(&["a"]));
    assert_eq!(xpath("//a[last()]"), nodes(&["a"]));
    assert_eq!(xpath("//c/.."), nodes(&["b"]));
    assert_eq!(xpath("//a/following-sibling::*"), nodes(&["a", "b"]));
    assert_eq!(xpath("//c[@n]/ancestor::*"), nodes(&["r", "b"]));
    assert_eq!(xpath("//@k"), nodes(&["k", "k"]));
    assert_eq!(xpath("//b/*"), nodes(&["c", "c", "d:e"]));
    assert_eq!(xpath("//d:e"), nodes(&["d:e"]));
    assert_eq!(xpath("//c | //a"), nodes(&["a", "a", "c", "c"]));
    assert_eq!(xpath("//comment()"), nodes(&["comment"]));
    assert_eq!(xpath(r#"//processing-instruction("pi")"#), nodes(&["pi"]));
    assert_eq!(xpath(r#"//text()[.="x"]"#), nodes(&["text"]));
    assert_eq!(xpath("(//a)[1]/@k"), nodes(&["k"]));
}

#[test]
fn string_functions_follow_the_xpath_rules() {
    assert_eq!(xpath("name(/r/*[3])"), text("b"));
    assert_eq!(xpath("string(//a[2])"), text(" y  z "));
    assert_eq!(xpath("normalize-space(//a[2])"), text("y z"));
    assert_eq!(xpath(r#"concat("a", 1, true())"#), text("a1true"));
    assert_eq!(xpath(r#"contains("abc","bc")"#), Answer::Bool(true));
    assert_eq!(xpath(r#"starts-with("abc","ab")"#), Answer::Bool(true));
    /* XPath 1.0 §4.2's own example: round(1.5) = 2, round(2.6) = 3, so
     * positions 2 <= p < 5. This line used to pin "23", the answer of rounding
     * the SUM 1.5 + 2.6. */
    assert_eq!(xpath(r#"substring("12345", 1.5, 2.6)"#), text("234"));
    assert_eq!(xpath(r#"substring-before("a/b","/")"#), text("a"));
    assert_eq!(xpath(r#"substring-after("a/b","/")"#), text("b"));
    assert_eq!(xpath(r#"translate("abc","abc","AB")"#), text("AB"));
    assert_eq!(xpath("local-name(//d:e)"), text("e"));
    assert_eq!(xpath("namespace-uri(//d:e)"), text("urn:d"));
}

#[test]
fn numbers_and_booleans_follow_the_xpath_rules() {
    assert_eq!(xpath("count(//*)"), Answer::Num(7.0));
    assert_eq!(xpath("string-length(//a[1])"), Answer::Num(1.0));
    assert_eq!(xpath(r#"number(" 12 ")"#), Answer::Num(12.0));
    assert_eq!(xpath("sum(//@k)"), Answer::Num(3.0));
    assert_eq!(xpath("floor(-1.5)"), Answer::Num(-2.0));
    assert_eq!(xpath("ceiling(-1.5)"), Answer::Num(-1.0));
    assert_eq!(xpath("round(2.5)"), Answer::Num(3.0));
    assert_eq!(xpath("round(-2.5)"), Answer::Num(-2.0));
    assert_eq!(xpath("round(0.49999999999999994)"), Answer::Num(0.0));
    // Negative zero compares equal to zero, so its sign shows through a division.
    assert_eq!(xpath("1 div round(-0.5)"), Answer::Num(f64::NEG_INFINITY));
    assert_eq!(xpath("1 div round(-0)"), Answer::Num(f64::NEG_INFINITY));
    assert_eq!(xpath("1 div 0"), Answer::Num(f64::INFINITY));
    assert_eq!(xpath("0 div 0"), Answer::Num(f64::NAN));
    assert_eq!(xpath("5 mod -2"), Answer::Num(1.0));
    assert_eq!(xpath(r#"//a = "x""#), Answer::Bool(true));
    assert_eq!(xpath("//@k > 1"), Answer::Bool(true));
    assert_eq!(xpath("boolean(//zz)"), Answer::Bool(false));
    assert_eq!(xpath("not(//a)"), Answer::Bool(false));
}

#[test]
fn failures_come_back_with_their_status() {
    assert_eq!(xpath("//a["), Answer::Err(ErrorKind::Syntax));
    assert_eq!(xpath("foo()"), Answer::Err(ErrorKind::Runtime));
    let capped = run(Query::XPath, "//c | //a", |l| l.max_nodeset_size = 2);
    assert_eq!(capped, Answer::Err(ErrorKind::Limit));
}

/// `1+1+...+1` with `ops` operators: a left-leaning tree `ops + 1` levels deep.
fn chain(ops: usize) -> String {
    let mut e = String::from("1");
    e.push_str(&"+1".repeat(ops));
    e
}

/// Whether `expr` parses under the default limits, or the status it fails with.
///
/// Parse only: evaluating a tree this deep takes more stack than a debug build's
/// test thread has, and what is being tested is where the tree stops being built.
fn parse_status(expr: &str) -> Result<(), ErrorKind> {
    let doc = xml_parse(DOC).expect("the fixture parses");
    let ctx = crate::xml::xpath::context(&doc, doc.doc_node());
    let mut budget = Budget::with_limits(ctx.limits());
    let source = VerifiedText::from_bytes(expr.as_bytes()).expect("verified");
    match parse_owned(source, &mut budget) {
        Ok(_) => Ok(()),
        Err(_) => Err(budget.take_error().status),
    }
}

#[test]
fn nesting_depth_is_bounded_where_the_tree_is_built() {
    // At the cap the tree is built; one level past it the parse refuses.
    assert_eq!(parse_status(&chain(1023)), Ok(()));
    assert_eq!(parse_status(&chain(1024)), Err(ErrorKind::Limit));
    // Under the cap the parse succeeds and the evaluation limit decides, as before.
    assert_eq!(parse_status(&chain(300)), Ok(()));
    // A chain that used to build tens of thousands of levels stops at the cap
    // instead of taking the stack with it.
    assert_eq!(parse_status(&chain(30_000)), Err(ErrorKind::Limit));
}

/// `f()` answers true, first running `inner` on the same context when `nest`
/// is set - the shape of a Ruby handler that evaluates again mid-walk.
struct Nesting<'a> {
    ctx: &'a Context<'a, &'a crate::xml::model::Document>,
    inner: Box<Ast>,
    nest: bool,
}

impl Resolver for Nesting<'_> {
    fn resolve(
        &self,
        _budget: &mut Budget,
        call: &ResolverCall<'_>,
    ) -> Result<Option<crate::xpath::value::Val>, crate::engine_error::Reported> {
        if call.local != b"f" {
            return Ok(None);
        }
        if self.nest {
            let _ = self.ctx.evaluate(&self.inner, None);
        }
        Ok(Some(crate::xpath::value::Val::boolean(true)))
    }
}

/// `//node()[f()]` against [`DOC`] under `max_eval_ops`, with or without the
/// nested evaluate inside `f()`.
fn walk_with_handler(nest: bool, max_eval_ops: usize) -> Answer {
    let doc = xml_parse(DOC).expect("the fixture parses");
    let mut ctx = crate::xml::xpath::context(&doc, doc.doc_node());
    ctx.limits_mut().max_eval_ops = max_eval_ops;
    let parse = |text: &str| {
        let mut budget = Budget::new();
        let source = VerifiedText::from_bytes(text.as_bytes()).unwrap();
        /* `source` borrows `text`, which outlives the parse. */
        match parse_owned(source, &mut budget) {
            Ok(ast) => ast,
            Err(_) => panic!("{text} parses"),
        }
    };
    let outer = parse("//node()[f()]");
    let nesting = Nesting {
        ctx: &ctx,
        inner: parse("true()"),
        nest,
    };
    match ctx.evaluate(&outer, Some(&nesting)) {
        Ok(XPathValue::NodeSet(set)) => Answer::Num(set.len() as f64),
        Ok(_) => panic!("the outer evaluate answers a node-set"),
        Err(e) => Answer::Err(e.status),
    }
}

#[test]
fn a_nested_evaluate_does_not_refill_the_outer_budget() {
    /* With room to spare, the handler's nested evaluate changes nothing. */
    let all = walk_with_handler(false, 1000);
    assert!(matches!(all, Answer::Num(n) if n > 0.0), "{all:?}");
    assert_eq!(walk_with_handler(true, 1000), all);
    /* A budget the walk overruns stays overrun when every predicate call
     * evaluates again - the nested run must not reset the outer's count. */
    assert_eq!(walk_with_handler(false, 30), Answer::Err(ErrorKind::Limit));
    assert_eq!(walk_with_handler(true, 30), Answer::Err(ErrorKind::Limit));
}

#[cfg(feature = "lexbor")]
fn css(selector: &str) -> Answer {
    run(Query::Css, selector, |_| {})
}

#[cfg(feature = "lexbor")]
#[test]
fn css_selectors_lower_to_the_same_answers_as_xml_css() {
    assert_eq!(css("a"), nodes(&["a", "a"]));
    assert_eq!(css(r#"a[k="2"]"#), nodes(&["a"]));
    assert_eq!(css("b > c"), nodes(&["c", "c"]));
    assert_eq!(css("a + b"), nodes(&["b"]));
    assert_eq!(css("c:nth-child(2)"), nodes(&["c"]));
    assert_eq!(css("b > :not(c)"), nodes(&["d:e"]));
    assert_eq!(css("d|e"), nodes(&["d:e"]));
    assert_eq!(css("b > *:first-child"), nodes(&["c"]));
    assert_eq!(css("a:last-of-type"), nodes(&["a"]));
    assert_eq!(css("c[n]"), nodes(&["c"]));
    assert_eq!(css("a["), Answer::Err(ErrorKind::Syntax));
    // A selector list lowers to a chain of unions, held to the same depth cap.
    assert_eq!(
        css(&vec!["a"; 1100].join(",")),
        Answer::Err(ErrorKind::Limit)
    );
}

/// A selector list nested past the lowering's cap is refused with LIMIT on the
/// way down, not after the native stack has run out: a stack overflow longjmps
/// past `Parsed`'s `Drop`, leaving the process-global parser's borrow held, so
/// every later XML `css`/`at_css`/`matches?` would fail `Busy` for the life of
/// the process. The second assertion is what tells an ordinary refusal from a
/// lost borrow.
#[cfg(feature = "lexbor")]
#[test]
fn a_deeply_nested_selector_is_refused_and_leaves_the_parser_usable() {
    let depth = crate::css::MAX_SELECTOR_NESTING as usize + 8;
    let nested = format!("{}a{}", ":not(".repeat(depth), ")".repeat(depth));
    assert_eq!(css(&nested), Answer::Err(ErrorKind::Limit));
    assert_eq!(css("a"), nodes(&["a", "a"]));
}

/// With no room in the string-value cache, comparisons build their values
/// uncached and still answer as with the cache.
#[test]
fn comparisons_answer_the_same_with_no_cache() {
    let exprs = [
        "//a = //a",
        "//a != //a",
        r#"count(//a[. = //a])"#,
        "//a < //c",
        r#"count(//*[. = "x"])"#,
        "sum(//@k) = 3",
    ];
    for e in exprs {
        let cached = xpath(e);
        let uncached = run(Query::XPath, e, |l| l.max_cache_bytes = 0);
        assert_eq!(cached, uncached, "{e}");
    }
}

/// A document whose string-values come in every shape the reader takes: one
/// text (borrowed), several (built), none, whitespace only, CDATA, a lone text
/// among empty elements, and an attribute.
const SHAPES: &[u8] = br#"<r><li>item 5</li><m>a<!--c-->b<?p q?></m><e/><w> </w><s><![CDATA[cd]]></s><n>x<![CDATA[y]]>z</n><o><i></i>only<i/></o><x a="val"/></r>"#;

/// `expr` against [`SHAPES`] from its document node: the answer, or the
/// failure's status and message.
fn shapes(
    expr: &str,
    tighten: impl FnOnce(&mut crate::xpath::limits::Limits),
) -> Result<Answer, (ErrorKind, String)> {
    let doc = xml_parse(SHAPES).expect("the fixture parses");
    let mut ctx = crate::xml::xpath::context(&doc, doc.doc_node());
    tighten(ctx.limits_mut());
    let mut budget = Budget::with_limits(ctx.limits());
    let source = VerifiedText::from_bytes(expr.as_bytes()).expect("verified");
    let Ok(ast) = parse_owned(source, &mut budget) else {
        panic!("{expr} parses");
    };
    match ctx.evaluate(&ast, None) {
        Ok(XPathValue::NodeSet(set)) => Ok(Answer::Num(set.len() as f64)),
        Ok(XPathValue::String(t)) => Ok(text(&String::from_utf8_lossy(t.as_slice()))),
        Ok(XPathValue::Number(d)) => Ok(Answer::Num(d)),
        Ok(XPathValue::Boolean(b)) => Ok(Answer::Bool(b)),
        Err(e) => Err((e.status, e.message().unwrap_or("").to_string())),
    }
}

fn shape(expr: &str) -> Answer {
    shapes(expr, |_| {}).unwrap_or_else(|e| panic!("{expr}: {e:?}"))
}

#[test]
fn string_values_read_in_place_and_built_agree_with_section_5() {
    /* One text: the element's value is that text node's slice. */
    assert_eq!(shape("string(//li)"), text("item 5"));
    assert_eq!(shape("//li = 'item 5'"), Answer::Bool(true));
    assert_eq!(shape("count(//*[. = 'item 5'])"), Answer::Num(1.0));
    assert_eq!(shape("string-length(//li)"), Answer::Num(6.0));
    assert_eq!(shape("contains(//li, 'm 5')"), Answer::Bool(true));
    assert_eq!(shape("normalize-space(//li)"), text("item 5"));
    /* Several: joined in document order, comments and PIs left out. */
    assert_eq!(shape("string(//m)"), text("ab"));
    assert_eq!(shape("//m = 'ab'"), Answer::Bool(true));
    assert_eq!(shape("string(//n)"), text("xyz"));
    assert_eq!(shape("//n = 'xyz'"), Answer::Bool(true));
    /* None: the empty string. Whitespace only is text. */
    assert_eq!(shape("string(//e)"), text(""));
    assert_eq!(shape("//e = ''"), Answer::Bool(true));
    assert_eq!(shape("string-length(//w)"), Answer::Num(1.0));
    /* CDATA is text, alone or among other texts. */
    assert_eq!(shape("string(//s)"), text("cd"));
    assert_eq!(shape("//s = 'cd'"), Answer::Bool(true));
    /* Empty elements around the one text change nothing. */
    assert_eq!(shape("string(//o)"), text("only"));
    assert_eq!(shape("//o = 'only'"), Answer::Bool(true));
    /* The document: every text, joined. */
    assert_eq!(shape("string(/)"), text("item 5ab cdxyzonly"));
    /* Attributes and leaves read their own value. */
    assert_eq!(shape("string(//x/@a)"), text("val"));
    assert_eq!(shape("//x/@a = 'val'"), Answer::Bool(true));
    assert_eq!(shape("starts-with(//x/@a, 'va')"), Answer::Bool(true));
    assert_eq!(shape("string(//m/comment())"), text("c"));
    assert_eq!(shape("//li/text() = //li"), Answer::Bool(true));
    /* Node-set against node-set, read in place on both sides and cached. */
    assert_eq!(shape("count(//*[. = //li/text()])"), Answer::Num(1.0));
    assert_eq!(shape("//s = //n"), Answer::Bool(false));
}

#[test]
fn a_borrowed_string_value_is_held_to_the_byte_cap_like_a_built_one() {
    let cap = |bytes| move |l: &mut crate::xpath::limits::Limits| l.max_string_bytes = bytes;
    /* At the cap a borrowed value is read. */
    assert_eq!(shapes("string(//li)", cap(6)), Ok(text("item 5")));
    assert_eq!(shapes("string(//x/@a)", cap(3)), Ok(text("val")));
    /* Past it every shape fails the same way: element, text node, attribute,
     * a comparison's cached read, and a value that has to be built. */
    let built = shapes("string(//m)", cap(1)).expect_err("over the cap");
    assert_eq!(built.0, ErrorKind::Limit);
    assert!(built.1.contains("string size limit exceeded"), "{built:?}");
    for e in [
        "string(//li)",
        "string(//li/text())",
        "string(//x/@a)",
        "//li = 'x'",
        "//x/@a = 'x'",
        "contains(//li, 'x')",
        "number(//li)",
        "sum(//li)",
    ] {
        assert_eq!(shapes(e, cap(1)), Err(built.clone()), "{e}");
    }
    /* A lone text below the cap in a value built from several still trips it. */
    assert_eq!(
        shapes("string(//n)", cap(2)).map_err(|e| e.0),
        Err(ErrorKind::Limit)
    );
    assert_eq!(shapes("string(//n)", cap(3)), Ok(text("xyz")));
}

/// The smallest `max_eval_ops` under which `expr` answers on [`SHAPES`].
fn ops_needed(expr: &str) -> usize {
    (1..10_000)
        .find(|&n| shapes(expr, |l| l.max_eval_ops = n).is_ok())
        .expect("fits some budget")
}

#[test]
fn a_borrowed_string_value_still_charges_its_walk() {
    /* `//o`'s value is one borrowed slice, but finding it walks three nodes
     * (i, the text, i) - one op each, exactly as a built value's walk. */
    assert_eq!(ops_needed("string(//o)"), ops_needed("boolean(//o)") + 3);
    assert_eq!(ops_needed("string(//m)"), ops_needed("boolean(//m)") + 4);
    /* A leaf walks nothing. */
    assert_eq!(
        ops_needed("string(//li/text())"),
        ops_needed("boolean(//li/text())")
    );
}
