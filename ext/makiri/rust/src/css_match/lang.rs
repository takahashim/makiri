//! `:lang()`: an element's language - the nearest `lang` (in XML,
//! `xml:lang`) of it and its ancestors - matched by RFC 4647's extended
//! filtering, as Selectors 4 has it. What an element IS, like `state`'s,
//! apart only for its size.

use crate::selectors::ast::LangRanges;

use super::dom::{CssElement, CssNode, Host};

/// `:lang()`: the element's language (the nearest `lang` - or, in XML,
/// `xml:lang` - of it and its ancestors) matched by any of the ranges.
#[inline(never)]
#[cold]
pub(super) fn lang_matches<'d, N: CssNode<'d>>(node: N, ranges: LangRanges<'_>) -> bool {
    language_of(node).is_some_and(|tag| ranges.iter().any(|range| extended_filter(range, tag)))
}

/// The language of the element `node`: the value of the first `lang` on it
/// or an ancestor - in an XML document `xml:lang`, then `lang` on an XHTML
/// element. An empty value is a language of its own ("unknown"), which stops
/// the climb. `None` when no element up the tree gives one.
fn language_of<'d, N: CssNode<'d>>(node: N) -> Option<&'d [u8]> {
    let mut at = Some(node);
    while let Some(n) = at {
        if let Some(el) = n.element() {
            let lang = if <N::Host as Host>::RESOLVES_NAMES {
                el.get_attribute(b"lang")
            } else {
                el.get_attribute(b"xml:lang").or_else(|| {
                    if n.is_html() {
                        el.get_attribute(b"lang")
                    } else {
                        None
                    }
                })
            };
            if lang.is_some() {
                return lang;
            }
        }
        at = n.parent();
    }
    None
}

/// RFC 4647 section 3.3.2, extended filtering, as Selectors 4 uses it: does
/// the language range `range` match the language tag `tag`, ignoring ASCII
/// case? `*` as a subtag matches any (a tag's first one included), and a
/// range's subtags need only be found in order in the tag, never past a
/// single-character one. An empty range matches only an empty tag, and a
/// tag that is empty matches nothing else.
fn extended_filter(range: &[u8], tag: &[u8]) -> bool {
    if range.is_empty() || tag.is_empty() {
        return range.is_empty() && tag.is_empty();
    }
    let mut ranges = range.split(|&b| b == b'-');
    let mut tags = tag.split(|&b| b == b'-');
    let (Some(first_range), Some(first_tag)) = (ranges.next(), tags.next()) else {
        return false;
    };
    if first_range != b"*" && !first_range.eq_ignore_ascii_case(first_tag) {
        return false;
    }
    for subrange in ranges {
        if subrange == b"*" {
            continue;
        }
        loop {
            let Some(subtag) = tags.next() else {
                return false;
            };
            if subrange.eq_ignore_ascii_case(subtag) {
                break;
            }
            if subtag.len() == 1 {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::extended_filter as f;

    #[test]
    fn extended_filtering_follows_rfc_4647() {
        assert!(f(b"en", b"en-US"));
        assert!(f(b"EN", b"en-us"));
        assert!(f(b"en-US", b"en-US"));
        assert!(!f(b"en-GB", b"en-US"));
        assert!(!f(b"e", b"en"));
        assert!(f(b"*-US", b"en-US"));
        assert!(!f(b"*-GB", b"en-US"));
        assert!(f(b"*", b"en"));
        // A subtag of the range is found later in the tag, not past a singleton.
        assert!(f(b"zh-TW", b"zh-Hant-TW"));
        assert!(f(b"de-1996", b"de-DE-1996"));
        assert!(!f(b"en-US", b"en-a-US"));
        // Empty: only each other.
        assert!(f(b"", b""));
        assert!(!f(b"", b"en"));
        assert!(!f(b"en", b""));
        assert!(!f(b"*", b""));
    }
}
