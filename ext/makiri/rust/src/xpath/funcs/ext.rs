//! The functions beyond XPath 1.0's own library: Nokogiri's two builtins, in
//! its builtin namespace. `lookup` in the parent is still the one place that
//! knows which names exist.

#![forbid(unsafe_code)]

use super::*;

/* ---------- the Nokogiri builtins ---------- */

/// css-class(haystack, needle): true iff `needle` is a whitespace-separated
/// token of `haystack`. Kept behaviour-identical to libxml2's builtin_css_class.
fn ws_token_match(hay: &[u8], val: &[u8]) -> bool {
    if val.is_empty() {
        return true; /* libxml2 returns non-NULL for an empty val */
    }
    hay.split(|&b| crate::xpath::lex::is_ws(b))
        .any(|t| t == val)
}

pub(super) fn fn_css_class<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    _focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    two::<D, _>(ev, args, |hay, needle| boolean(ws_token_match(hay, needle)))
}

/// local-name-is(name): true iff the context node's qualified name (for HTML the
/// lowercase local name) equals the argument.
pub(super) fn fn_local_name_is<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let doc = ev.doc;
    let want = to_text::<D>(&args[0], ev)?;
    boolean(
        focus
            .node
            .is_some_and(|n| doc.qualified_name(n) == want.as_slice()),
    )
}
