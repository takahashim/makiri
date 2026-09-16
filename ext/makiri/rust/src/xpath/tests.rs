//! The engine end to end, without Ruby: an XML document, an expression or a
//! selector, and the value that comes back.
//!
//! The Ruby specs reach the same paths through `Node#xpath` and `#css`; these
//! run under `cargo test` (and under ASan in CI) with nothing between the test
//! and the engine's front door. The expected values are what `Makiri::XML`
//! answered for the same document and expressions when these were written.

#![allow(unsafe_code)]

use core::ffi::{c_int, c_void};

use crate::xpath::limits::Budget;

use crate::text::VerifiedText;
use crate::xml::parse::xml_parse;
use crate::xml::{NodeId, NodeType};
use crate::xpath::ast::Ast;
use crate::xpath::ctx::{Context, Resolver, ResolverCall, XPathValue};
use crate::xpath::msg::{XP_ERR_LIMIT, XP_ERR_RUNTIME, XP_ERR_SYNTAX};
use crate::xpath::parse::parse_owned;

const DOC: &[u8] = br#"<r xmlns:d="urn:d"><a k="1">x</a><a k="2"> y  z </a><b><c/><c n="3"/><d:e>ne</d:e></b><!--cm--><?pi data?></r>"#;

#[derive(Debug)]
enum Answer {
    Nodes(Vec<String>),
    Str(String),
    Num(f64),
    Bool(bool),
    Err(c_int),
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
    let mut ctx = Context::xml(&doc, doc.doc_node());
    ctx.register_ns(b"d", b"urn:d").expect("registered");
    tighten(ctx.limits_mut());

    let source = VerifiedText::from_bytes(expr.as_bytes()).expect("verified");
    let mut parse_budget = Budget::with_limits(ctx.limits());
    // SAFETY: `source` borrows `expr`, which outlives the parse.
    let compiled: Result<Box<Ast>, _> = unsafe {
        match query {
            Query::XPath => parse_owned(source, &mut parse_budget),
            #[cfg(feature = "lexbor")]
            Query::Css => {
                let ns = crate::css::CssNs {
                    default_namespace: false,
                };
                crate::css::compile_owned(source, &ns, &mut parse_budget)
            }
        }
    };
    let ast = match compiled {
        Ok(ast) => ast,
        Err(_) => return Answer::Err(parse_budget.take_error().status),
    };
    {
        let describe = |node: &*mut c_void| {
            let id = NodeId::from_token(*node as usize);
            match doc.type_(id) {
                Some(NodeType::Text) | Some(NodeType::CData) => "text".to_string(),
                Some(NodeType::Comment) => "comment".to_string(),
                Some(NodeType::Pi) => String::from_utf8_lossy(doc.local(id)).into_owned(),
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
    assert_eq!(xpath(r#"substring("12345", 1.5, 2.6)"#), text("23"));
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
    assert_eq!(xpath("//a["), Answer::Err(XP_ERR_SYNTAX));
    assert_eq!(xpath("foo()"), Answer::Err(XP_ERR_RUNTIME));
    let capped = run(Query::XPath, "//c | //a", |l| l.max_nodeset_size = 2);
    assert_eq!(capped, Answer::Err(XP_ERR_LIMIT));
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
fn parse_status(expr: &str) -> Result<(), c_int> {
    let doc = xml_parse(DOC).expect("the fixture parses");
    let ctx = Context::xml(&doc, doc.doc_node());
    let mut budget = Budget::with_limits(ctx.limits());
    let source = VerifiedText::from_bytes(expr.as_bytes()).expect("verified");
    // SAFETY: `source` borrows `expr`, which outlives the parse.
    match unsafe { parse_owned(source, &mut budget) } {
        Ok(_) => Ok(()),
        Err(_) => Err(budget.take_error().status),
    }
}

#[test]
fn nesting_depth_is_bounded_where_the_tree_is_built() {
    // At the cap the tree is built; one level past it the parse refuses.
    assert_eq!(parse_status(&chain(1023)), Ok(()));
    assert_eq!(parse_status(&chain(1024)), Err(XP_ERR_LIMIT));
    // Under the cap the parse succeeds and the evaluation limit decides, as before.
    assert_eq!(parse_status(&chain(300)), Ok(()));
    // A chain that used to build tens of thousands of levels stops at the cap
    // instead of taking the stack with it.
    assert_eq!(parse_status(&chain(30_000)), Err(XP_ERR_LIMIT));
}

/// `f()` answers true, first running `inner` on the same context when `nest`
/// is set - the shape of a Ruby handler that evaluates again mid-walk.
struct Nesting<'a> {
    ctx: &'a Context<'a>,
    inner: Box<Ast>,
    nest: bool,
}

// SAFETY: it answers no nodes, and changes nothing.
unsafe impl Resolver for Nesting<'_> {
    fn resolve(
        &self,
        _budget: &mut Budget,
        call: &ResolverCall<'_>,
    ) -> Result<Option<crate::xpath::value::Val>, crate::xpath::msg::Reported> {
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
    let mut ctx = Context::xml(&doc, doc.doc_node());
    ctx.limits_mut().max_eval_ops = max_eval_ops;
    let parse = |text: &str| {
        let mut budget = Budget::new();
        let source = VerifiedText::from_bytes(text.as_bytes()).unwrap();
        // SAFETY: `source` borrows `text`, which outlives the parse.
        match unsafe { parse_owned(source, &mut budget) } {
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
        Ok(_) => Answer::Err(-1),
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
    assert_eq!(walk_with_handler(false, 30), Answer::Err(XP_ERR_LIMIT));
    assert_eq!(walk_with_handler(true, 30), Answer::Err(XP_ERR_LIMIT));
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
    assert_eq!(css("a["), Answer::Err(XP_ERR_SYNTAX));
    // A selector list lowers to a chain of unions, held to the same depth cap.
    assert_eq!(css(&vec!["a"; 1100].join(",")), Answer::Err(XP_ERR_LIMIT));
}
