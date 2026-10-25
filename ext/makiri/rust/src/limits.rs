//! Bounds shared across layers, defined once.

#![forbid(unsafe_code)]

/// The most nodes one result set may hold. Every node-collecting path - the
/// XPath evaluator, the CSS selector engine, `NodeSet` and its set operations -
/// fails closed at this same bound rather than growing without limit. It lived
/// as four literal copies once, and a drift between them would have made CSS
/// and XPath disagree about where "too many" starts.
pub const NODE_SET_MAX: usize = 10 * 1000 * 1000;

/// A work counter's next value once `n` more are charged against `cap`, or
/// `None` to refuse - and a refusal leaves the counter as it was. The one rule
/// every work budget keeps: XPath's evaluator ops, AST nodes and recursion
/// depth, and the CSS matcher's steps. An admitted charge never takes the
/// counter past `cap`, and so never overflows; Verus proves it, and that from
/// zero no sequence of charges passes `cap` (`verus/limits.rs`).
#[inline]
pub fn admit(count: usize, n: usize, cap: usize) -> Option<usize> {
    if count > cap || cap - count < n {
        return None;
    }
    Some(count + n)
}
