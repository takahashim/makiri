//! Bounds shared across layers, defined once.

#![forbid(unsafe_code)]

/// The most nodes one result set may hold. Every node-collecting path - the
/// XPath evaluator, the CSS selector engine, `NodeSet` and its set operations -
/// fails closed at this same bound rather than growing without limit. It lived
/// as four literal copies once, and a drift between them would have made CSS
/// and XPath disagree about where "too many" starts.
pub const NODE_SET_MAX: usize = 10 * 1000 * 1000;
