//! The compiled-selector cache for `css_match`'s queries, HTML and XML: a
//! selector string is parsed once by the ported parser (`crate::selectors`)
//! and its AST kept for the next query that uses the same string.
//!
//! # The cache adapts
//!
//! Compiled selectors are kept in a map keyed by the selector bytes (a Rust
//! map rather than a Ruby Hash, which saves a `VALUE` round-trip on every
//! lookup). A flood of one-off selectors - `getElementById` on unique React
//! `useId` ids, never requeried - would make every query pay for a key copy
//! and an insert that never pays back, so the hit rate is tracked over a
//! window ([`CachePolicy`]); below a floor, the cache is BYPASSED (parse per
//! call), and caching is periodically re-tested so a workload that starts
//! repeating selectors regains it.
//!
//! The policy was written for Lexbor's parser, whose lists shared one arena
//! that grew with every cached list and slowed each later parse; the port's
//! lists are owned, one allocation set per selector, so a cached entry costs
//! only its memory. The same constants are kept.

//!
//! # One per thread
//!
//! The cache and the matcher's kept stacks are thread-local: a query never
//! releases the GVL, so a per-thread engine is reached by one query at a
//! time - from whichever Ractor the thread is in, each Ractor's threads under
//! a GVL of their own - and being the thread's alone it needs to be
//! neither `Send` nor `Sync` - the stacks hold DOM node handles, which are
//! neither. (It was one process-global behind a GVL-keyed cell, which needed
//! the stacks' node vectors to be `Send` and a type that made them so.) Each
//! Ruby thread warms its own cache; a re-entrant query on one thread is
//! refused as [`ParseError::Busy`], as it was.

#![forbid(unsafe_code)]
// Its one entry point (`with_compiled`) is the Ruby glue's
// (`glue::html_node::css`, `glue::xml_node::css`) and the `html_css` fuzz
// harness's.

use core::cell::RefCell;
use std::collections::HashMap;

use crate::css_match::dom::Host;
use crate::css_match::Scratch;
use crate::falloc::{try_to_boxed_slice, MapInsert};
use crate::html::dom::css::HtmlHost;
use crate::selectors::ast::Lists;
use crate::selectors::{Parsed, Status};
use crate::xml::css::XmlHost;

/// Why a parse did not produce a selector list.
#[derive(Debug)]
pub enum ParseError {
    /// The parser could not be created.
    NotReady,
    /// The selector is malformed.
    Syntax,
    /// A parse on this thread is still borrowing the parser.
    Busy,
    /// Out of memory parsing - not a verdict on the selector.
    Oom,
}

/// Flush the whole cache once it holds this many distinct selectors.
const CACHE_CAP: usize = 256;
/// Re-evaluate the hit rate every N lookups.
const WIN: usize = 1024;
/// Below this hit rate (percent), bypass the cache.
const MIN_HIT_PCT: usize = 15;
/// Re-test caching every N bypass windows.
const RETEST_GAP: usize = 32;

/// Whether the compiled-selector cache is paying for itself.
///
/// Counts lookups over a window of [`WIN`]; at a window's end, a hit rate
/// under [`MIN_HIT_PCT`] switches caching off, and after [`RETEST_GAP`] such
/// windows it is switched back on to be measured again.
struct CachePolicy {
    win: usize,
    win_hits: usize,
    bypass: bool,
    bypass_runs: usize,
}

impl CachePolicy {
    const fn new() -> Self {
        CachePolicy {
            win: 0,
            win_hits: 0,
            bypass: false,
            bypass_runs: 0,
        }
    }

    /// Count one lookup. `true` when this lookup ends a window in which caching
    /// did not pay, so caching is now off and the cached lists must go.
    fn tick(&mut self) -> bool {
        self.win += 1;
        if self.win <= WIN {
            return false;
        }
        let mut switched_off = false;
        if !self.bypass {
            if self.win_hits * 100 < WIN * MIN_HIT_PCT {
                self.bypass = true;
                self.bypass_runs = 0;
                switched_off = true;
            }
        } else {
            self.bypass_runs += 1;
            if self.bypass_runs >= RETEST_GAP {
                self.bypass = false; /* re-test caching over the next window */
            }
        }
        self.win = 1;
        self.win_hits = 0;
        switched_off
    }

    fn hit(&mut self) {
        self.win_hits += 1;
    }

    fn bypassing(&self) -> bool {
        self.bypass
    }
}

/// Selector bytes -> the parsed selector.
struct Cache {
    map: Option<HashMap<Box<[u8]>, Parsed>>,
}

impl Cache {
    const fn new() -> Self {
        Cache { map: None }
    }

    fn map(&mut self) -> &mut HashMap<Box<[u8]>, Parsed> {
        self.map.get_or_insert_with(HashMap::new)
    }

    fn flush(&mut self) {
        self.map().clear();
    }

    /// The cached parse of `selector` - which counts as a hit in `policy` -
    /// or one parsed and cached now.
    fn get(&mut self, policy: &mut CachePolicy, selector: &[u8]) -> Result<&Parsed, ParseError> {
        if self.map().contains_key(selector) {
            policy.hit();
        } else {
            /* Bound the cache: when it is full, drop every entry at once. */
            if self.map().len() >= CACHE_CAP {
                self.flush();
            }
            /* The key is copied: the borrow points into a Ruby String that may
             * be collected or mutated, while the entry outlives the call. */
            let key = try_to_boxed_slice(selector).ok_or(ParseError::Oom)?;
            let parsed = parse(selector)?;
            if self.map().falloc_insert(key, parsed).is_err() {
                return Err(ParseError::Oom);
            }
        }
        self.map().get(selector).ok_or(ParseError::Oom)
    }
}

struct Globals {
    policy: CachePolicy,
    /// Parsed selectors, which name no DOM: one cache serves both.
    cache: Cache,
    /// The matcher's stacks, kept from one query to the next (its doc) - one
    /// per DOM, since they hold its nodes.
    scratches: Scratches,
}

/// [`Globals`]' kept [`Scratch`]es.
pub struct Scratches {
    html: Scratch<HtmlHost>,
    xml: Scratch<XmlHost>,
}

/// A DOM whose [`Scratch`] the cache keeps.
pub trait Kept: Host {
    fn kept(s: &mut Scratches) -> &mut Scratch<Self>;
}

impl Kept for HtmlHost {
    fn kept(s: &mut Scratches) -> &mut Scratch<Self> {
        &mut s.html
    }
}

impl Kept for XmlHost {
    fn kept(s: &mut Scratches) -> &mut Scratch<Self> {
        &mut s.xml
    }
}

thread_local! {
    /// This thread's engine, borrowed once per query.
    static G: RefCell<Globals> = const {
        RefCell::new(Globals {
            policy: CachePolicy::new(),
            cache: Cache::new(),
            scratches: Scratches {
                html: Scratch::new(),
                xml: Scratch::new(),
            },
        })
    };
}

/// Parse `selector` with the ported parser.
fn parse(selector: &[u8]) -> Result<Parsed, ParseError> {
    crate::selectors::parse(selector).map_err(|st| match st {
        Status::ErrorMemoryAllocation => ParseError::Oom,
        _ => ParseError::Syntax,
    })
}

/// Parse `selector` (or serve it from the cache) and hand its comma groups to
/// `f` as a [`Lists`] view - with the matcher's kept [`Scratch`].
///
/// Three ways: a window that ended with caching off flushes the cache;
/// bypassing parses per call; otherwise the selector comes from the cache,
/// parsed into it on a miss.
pub fn with_compiled<H: Kept, R>(
    selector: &[u8],
    f: impl FnOnce(Lists<'_>, &mut Scratch<H>) -> R,
) -> Result<R, ParseError> {
    /* `try_with`: a query from a thread-local destructor, after this one is
     * gone, is refused rather than a panic. */
    G.try_with(|cell| {
        let mut guard = cell.try_borrow_mut().map_err(|_| ParseError::Busy)?;
        let g = &mut *guard;

        if g.policy.tick() {
            g.cache.flush();
        }

        if g.policy.bypassing() {
            let parsed = parse(selector)?;
            return Ok(f(parsed.groups(), H::kept(&mut g.scratches)));
        }

        let parsed = g.cache.get(&mut g.policy, selector)?;
        Ok(f(parsed.groups(), H::kept(&mut g.scratches)))
    })
    .map_err(|_| ParseError::NotReady)?
}

/// The cache's own checks, each on its test thread's cache.
///
/// The adaptive bypass/retest window (40,000-iteration territory in
/// `spec/css_selector_cache_spec.rb`) is NOT re-tested here - that spec
/// already covers it end to end through the real glue, and duplicating a
/// slow, iteration-heavy test at this level buys little; this module checks
/// the parts a fast, ASan-friendly Rust test is good for: a hit answers the
/// same as a miss would, filling the cache past its cap doesn't lose
/// correctness, and a rejected selector does not disturb what is already
/// cached.
#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_methods)] /* test inputs */
    use crate::css_cache::with_compiled;
    use crate::css_match::select_all;
    use crate::html_dom::html::{HtmlNode, RawNode};
    use crate::html_dom::post_parse::{parse_html, HtmlParsed};
    use crate::html_dom::tree_guard::ParseLimits;

    fn parsed(html: &[u8]) -> Box<HtmlParsed> {
        parse_html(html, true, ParseLimits::DEFAULT).expect("a document parses")
    }

    fn root(doc: &HtmlParsed) -> HtmlNode<'_> {
        doc.doc().as_node()
    }

    /// Every matching descendant's node identity, through the cache.
    fn select_all_cached(doc: &HtmlParsed, selector: &str) -> Vec<RawNode> {
        with_compiled(selector.as_bytes(), |groups, scratch| {
            select_all(
                scratch,
                root(doc),
                groups,
                &crate::css_match::Namespaces::NONE,
            )
        })
        .unwrap_or_else(|_| panic!("{selector:?} fails to parse"))
        .unwrap_or_else(|e| panic!("{selector:?} failed: {e:?}"))
        .into_iter()
        .map(RawNode::from)
        .collect()
    }

    #[test]
    fn a_cache_hit_answers_the_same_as_the_first_miss_did() {
        let doc = parsed(b"<html><body><p class=x>a</p><p>b</p><p class=x>c</p></body></html>");
        let first = select_all_cached(&doc, "p.x");
        let second = select_all_cached(&doc, "p.x"); // same bytes: a cache hit
        assert_eq!(first.len(), 2);
        assert!(
            first == second,
            "a hit must answer exactly what the miss did"
        );
    }

    #[test]
    fn filling_the_cache_past_its_cap_does_not_lose_correctness() {
        // 300 distinct selectors against the 256-entry cap (mirrors
        // spec/css_selector_cache_spec.rb): the 257th flushes it, and every
        // one - before and after the flush - must still answer right.
        let mut html = String::from("<html><body>");
        for i in 1..=300 {
            html.push_str(&format!("<p id=n{i}>{i}</p>"));
        }
        html.push_str("</body></html>");
        let doc = parsed(html.as_bytes());

        for i in 1..=300 {
            let sel = format!("#n{i}");
            let found = select_all_cached(&doc, &sel);
            assert_eq!(found.len(), 1, "selector {sel:?} should match exactly one");
        }
        // A second pass re-hits everything, including the ones the cap
        // already flushed out and back in once.
        for i in 1..=300 {
            let sel = format!("#n{i}");
            assert_eq!(select_all_cached(&doc, &sel).len(), 1);
        }
    }

    #[test]
    fn a_rejected_selector_does_not_disturb_what_is_already_cached() {
        let doc = parsed(b"<html><body><p class=x>a</p><p class=x>b</p></body></html>");
        assert_eq!(select_all_cached(&doc, "p.x").len(), 2);

        let rejected = with_compiled(b"p[", |groups, scratch| {
            select_all(
                scratch,
                root(&doc),
                groups,
                &crate::css_match::Namespaces::NONE,
            )
        });
        assert!(rejected.is_err(), "a malformed selector must not parse");

        // The previously cached entry must still answer correctly.
        assert_eq!(select_all_cached(&doc, "p.x").len(), 2);
    }

    /// The kept `Scratch` carries capacity and nothing else: a query on a
    /// new document - which may reuse a dropped one's addresses and stamp
    /// space - answers what a fresh `Scratch` does, for the selectors that
    /// keep per-node memos (`:nth-*`, `of S`) and resolve names per document.
    #[test]
    fn the_kept_scratch_carries_nothing_from_one_document_to_the_next() {
        use crate::css_match::Scratch;
        use crate::html::dom::css::HtmlHost;
        const SELECTORS: &[&str] = &[
            "li:nth-child(2n+1)",
            "li:nth-last-of-type(2)",
            ":nth-child(2 of .x)",
            "[data-k=v]",
            "li[data-k]:nth-child(odd)",
        ];
        let docs: [&[u8]; 3] = [
            b"<ul><li class=x data-k=v>a<li>b<li class=x>c<li data-k=w>d</ul>",
            b"<ol><li>a<li class=x>b<li class=x data-k=v>c</ol><p data-k=v>",
            b"<ul><li data-k>a<li class=x>b<li class=x>c<li class=x>d<li>e</ul>",
        ];
        for round in 0..2 {
            for html in docs {
                let doc = parsed(html);
                for sel in SELECTORS {
                    let kept = select_all_cached(&doc, sel);
                    let fresh: Vec<RawNode> =
                        with_compiled(sel.as_bytes(), |groups, _: &mut Scratch<HtmlHost>| {
                            select_all(
                                &mut Scratch::new(),
                                root(&doc),
                                groups,
                                &crate::css_match::Namespaces::NONE,
                            )
                        })
                        .expect("parses")
                        .expect("matches")
                        .into_iter()
                        .map(RawNode::from)
                        .collect();
                    assert!(
                        kept == fresh,
                        "{sel:?} in round {round} differs from a fresh scratch"
                    );
                }
            }
        }
    }
}
