//! What an element IS, for the pseudo-classes that ask about its state
//! rather than its place in a selector: `:empty`/`:blank`/`:root`, the link
//! pseudo-classes, `:defined`, and the form states (`:disabled`, `:checked`, ...) - by
//! the HTML Standard or by Lexbor, as each function's doc says. None of it
//! knows about selectors; the form states that walk the tree charge the
//! query's work budget as they go.

use crate::compat::{self, Fix};
use crate::node_type::NodeType;

use super::dom::{CssAttr, CssElement, CssNode};
use super::tree::{children, parent_element};
use super::{Budget, MatchFailure};

/// Lexbor's own whitespace set for tokenizing an attribute value (`class`, or
/// any `~=` operand) - `lexbor_utils_whitespace`: space, tab, LF, FF, CR.
/// Deliberately NOT `u8::is_ascii_whitespace`, which also matches vertical tab
/// (0x0B) - a real behavioural difference, which `lexbor::tests::css_match`
/// pins.
pub(super) fn is_lexbor_whitespace(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0C | b'\r')
}

pub(super) fn is_html_namespace<'d, N: CssNode<'d>>(node: N) -> bool {
    node.is_html()
}

/// DOM `hasAttribute`, false for a non-element: the Standard's form states
/// ask for an element's own (no-namespace) attributes this way.
fn has_attr<'d, N: CssNode<'d>>(node: N, name: &[u8]) -> bool {
    node.element().is_some_and(|el| el.has_attribute(name))
}

/// Lexbor's tag test (`node->local_name == LXB_TAG_*`): the tag id, so the
/// stored lower-cased local name, in any namespace.
fn lexbor_tag<'d, E: CssElement<'d>>(el: E, tags: &[&[u8]]) -> bool {
    tags.contains(&el.local_name())
}

/// `lxb_dom_element_attr_by_id`: an attribute whose (lower-cased) local name
/// is `local`, in any namespace - so `xlink:href` counts as `href`.
fn lexbor_attr<'d, E: CssElement<'d>>(el: E, local: &[u8]) -> bool {
    el.attrs().any(|a| a.local_name() == local)
}

/// `:empty`: no element child, and no text child with data (a CDATA
/// section is text) - a comment or a processing instruction does not count,
/// as in Chromium, Firefox and WebKit (M10 in docs/parser_differences.ja.md).
/// Run as Lexbor ran, any child but a comment disqualifies
/// (`lxb_selectors_pseudo_class`'s `EMPTY` case), a processing instruction
/// and an empty text included.
pub(super) fn is_empty<'d, N: CssNode<'d>>(node: N) -> bool {
    if !compat::fixed(Fix::M10) {
        return !children(node).any(|c| c.node_type() != NodeType::Comment);
    }
    !children(node).any(|c| match c.node_type() {
        NodeType::Element => true,
        NodeType::Text | NodeType::CDataSection => c.char_data().is_some_and(|d| !d.is_empty()),
        _ => false,
    })
}

/// `:blank` (`lxb_dom_node_is_empty`, in Lexbor's `dom/interfaces/node.c`): as
/// Lexbor's `:empty`, but a Text child only disqualifies it when it holds a
/// non-whitespace byte - and a PI (or anything else that is neither Text nor
/// Comment) disqualifies it unconditionally.
pub(super) fn is_blank<'d, N: CssNode<'d>>(node: N) -> bool {
    !children(node).any(|c| match c.char_data() {
        Some(t) => t.iter().any(|&b| !is_lexbor_whitespace(b)),
        None => c.node_type() != NodeType::Comment,
    })
}

pub(super) fn is_root<'d, N: CssNode<'d>>(node: N) -> bool {
    node.is_root()
}

const SVG_NS: &[u8] = b"http://www.w3.org/2000/svg";
const XLINK_NS: &[u8] = b"http://www.w3.org/1999/xlink";

/// `:any-link` / `:link` (no history, so the same): an HTML `a` or `area`
/// with an `href` attribute (the HTML Standard's "Pseudo-classes"), or an
/// SVG `a` with an `href` or an XLink `href` (SVG 2), as Chromium, Firefox
/// and WebKit answer (M2 in docs/parser_differences.ja.md).
///
/// Run as Lexbor ran, `lxb_selectors_pseudo_class` has them: an element
/// whose tag is `a`, `area` or `map` (`:any-link`) / `a`, `area` or `link`
/// (`:link`), in any namespace, with an attribute whose local name is
/// `href`, in any namespace.
pub(super) fn is_any_link<'d, N: CssNode<'d>>(node: N, link_tag: bool) -> bool {
    let Some(el) = node.element() else {
        return false;
    };
    if !compat::fixed(Fix::M2) {
        let tags: [&[u8]; 3] = if link_tag {
            [b"a", b"area", b"link"]
        } else {
            [b"a", b"area", b"map"]
        };
        return lexbor_tag(el, &tags) && lexbor_attr(el, b"href");
    }
    let href = |xlink: bool| {
        el.attrs().any(|a| {
            a.local_name() == b"href"
                && match a.ns_uri() {
                    None => true,
                    Some(uri) => xlink && uri == XLINK_NS,
                }
        })
    };
    if html_named(node, &[b"a", b"area"]) {
        return href(false);
    }
    node.ns_uri() == Some(SVG_NS) && el.local_name() == b"a" && href(true)
}

/// `:required` (`required == true`) / `:optional`, exactly as
/// `lxb_selectors_pseudo_class` has them: an `input`, `select` or
/// `textarea` ([`lexbor_tag`]) with / without a `required` attribute
/// ([`lexbor_attr`]).
pub(super) fn is_required<'d, N: CssNode<'d>>(node: N, required: bool) -> bool {
    node.element().is_some_and(|el| {
        lexbor_tag(el, &[b"input", b"select", b"textarea"])
            && lexbor_attr(el, b"required") == required
    })
}

/// `:placeholder-shown`, exactly as `lxb_selectors_pseudo_class` has it: an
/// `input` or `textarea` with a `placeholder` attribute - not `select`, and
/// whether the field is empty is not looked at.
pub(super) fn is_placeholder_shown<'d, N: CssNode<'d>>(node: N) -> bool {
    node.element().is_some_and(|el| {
        lexbor_tag(el, &[b"input", b"textarea"]) && lexbor_attr(el, b"placeholder")
    })
}

/// `:active` / `:focus` / `:hover` as Lexbor answers them for a document
/// nobody interacts with: an element carrying an attribute of that name.
pub(super) fn has_state_attr<'d, N: CssNode<'d>>(node: N, local: &[u8]) -> bool {
    node.element().is_some_and(|el| lexbor_attr(el, local))
}

/// An HTML element named one of `names` (the stored, lower-cased local name).
fn html_named<'d, N: CssNode<'d>>(node: N, names: &[&[u8]]) -> bool {
    is_html_namespace(node)
        && node
            .element()
            .is_some_and(|el| names.contains(&el.local_name()))
}

/// Whether `node` is inside a `<fieldset disabled>` without being inside
/// that fieldset's first `legend` element child - the HTML Standard's
/// inheritance for form controls and fieldsets. The first `legend` CHILD,
/// not the first child: whitespace or another element may come before it.
/// Every disabled fieldset on the way up counts, so a legend exempts only
/// from its own fieldset.
///
/// Asked from the child's side - is the child on the way up a `legend` with
/// no `legend` before it - rather than by finding the fieldset's first
/// legend: that scanned the fieldset's children for every control in it,
/// quadratic in a wide fieldset. Each ancestor and each sibling looked at
/// charges `budget`, so a shape that is still wide (many legends, each
/// holding controls) fails closed instead of running on.
fn in_disabled_fieldset<'d, N: CssNode<'d>>(
    node: N,
    budget: &Budget,
) -> Result<bool, MatchFailure> {
    let mut child = node;
    let mut ancestor = parent_element(node);
    while let Some(a) = ancestor {
        budget.charge()?;
        if html_named(a, &[b"fieldset"]) && has_attr(a, b"disabled") {
            let mut exempt = html_named(child, &[b"legend"]);
            let mut prev = child.prev();
            while exempt {
                let Some(p) = prev else { break };
                budget.charge()?;
                exempt = !html_named(p, &[b"legend"]);
                prev = p.prev();
            }
            if !exempt {
                return Ok(true);
            }
        }
        child = a;
        ancestor = parent_element(a);
    }
    Ok(false)
}

/// `:disabled`, as the HTML Standard defines it (§4.16.3 and "disabled" for
/// each element): a `button`/`input`/`select`/`textarea` or `fieldset` with
/// a `disabled` attribute or inside a disabled fieldset
/// ([`in_disabled_fieldset`]), an `optgroup` with the attribute, an `option`
/// with it or in an `optgroup` with it. HTML elements only.
///
/// Deliberately NOT Lexbor's `lxb_selectors_pseudo_class_disabled` (module
/// doc), which
/// needs the attribute on the element itself (so an `<input>` inside a
/// disabled fieldset is enabled), counts any element with a custom tag, and
/// decides the legend exemption from the fieldset's `first_child` - a
/// whitespace text node defeats it, and an empty fieldset is a NULL read.
///
/// Form-associated custom elements are left out: whether a custom element is
/// form-associated is decided by a script's class definition, which a
/// parsed document does not have. As everywhere else here, the content
/// attribute stands for the element's state: the document as parsed.
pub(super) fn is_disabled<'d, N: CssNode<'d>>(
    node: N,
    budget: &Budget,
) -> Result<bool, MatchFailure> {
    if html_named(
        node,
        &[b"button", b"input", b"select", b"textarea", b"fieldset"],
    ) {
        return Ok(has_attr(node, b"disabled") || in_disabled_fieldset(node, budget)?);
    }
    if html_named(node, &[b"optgroup"]) {
        return Ok(has_attr(node, b"disabled"));
    }
    if html_named(node, &[b"option"]) {
        return Ok(has_attr(node, b"disabled")
            || parent_element(node)
                .is_some_and(|p| html_named(p, &[b"optgroup"]) && has_attr(p, b"disabled")));
    }
    Ok(false)
}

/// `:enabled`: the elements `:disabled` is defined for, when not disabled -
/// not every other element, as Lexbor's unconditional `!disabled` has it.
pub(super) fn is_enabled<'d, N: CssNode<'d>>(
    node: N,
    budget: &Budget,
) -> Result<bool, MatchFailure> {
    Ok(html_named(
        node,
        &[
            b"button",
            b"input",
            b"select",
            b"textarea",
            b"fieldset",
            b"optgroup",
            b"option",
        ],
    ) && !is_disabled(node, budget)?)
}

/// `:checked`, as the HTML Standard defines it: an `input` whose type is
/// Checkbox or Radio and which is checked, or an `option` that is
/// selected - by the `checked` / `selected` attribute, the parsed state.
/// HTML elements only. Lexbor also takes an element with a custom tag and a
/// `checked` attribute; the Standard does not.
pub(super) fn is_checked<'d, N: CssNode<'d>>(node: N) -> bool {
    if html_named(node, &[b"option"]) {
        return has_attr(node, b"selected");
    }
    if html_named(node, &[b"input"]) {
        let checkable = node
            .element()
            .and_then(|el| el.get_attribute(b"type"))
            .is_some_and(|t| {
                t.eq_ignore_ascii_case(b"checkbox") || t.eq_ignore_ascii_case(b"radio")
            });
        return checkable && has_attr(node, b"checked");
    }
    false
}

/// The `input` types the `readonly` attribute applies to (HTML Standard,
/// "the readonly attribute"); a missing or unknown type is Text, which it
/// applies to.
const READONLY_TYPES_NOT: [&[u8]; 10] = [
    b"hidden",
    b"range",
    b"color",
    b"checkbox",
    b"radio",
    b"file",
    b"submit",
    b"image",
    b"reset",
    b"button",
];

/// `:read-write`, as the HTML Standard defines it (M9 in
/// docs/parser_differences.ja.md): an `input` the `readonly` attribute
/// applies to, and a `textarea`, without that attribute and not disabled
/// ([`is_disabled`]); and any other element that is editable - inside an
/// editing host ([`is_editable`]). `:read-only` is every other element.
///
/// Run as Lexbor ran (`lxb_selectors_pseudo_class_read_write`): an `input`
/// or `textarea` ([`lexbor_tag`]) without a `readonly` attribute
/// ([`lexbor_attr`]) that is not disabled - disabled by the HTML Standard,
/// as `:disabled` is.
pub(super) fn is_read_write<'d, N: CssNode<'d>>(
    node: N,
    budget: &Budget,
) -> Result<bool, MatchFailure> {
    let Some(el) = node.element() else {
        return Ok(false);
    };
    if !compat::fixed(Fix::M9) {
        return Ok(lexbor_tag(el, &[b"input", b"textarea"])
            && !lexbor_attr(el, b"readonly")
            && !is_disabled(node, budget)?);
    }
    if html_named(node, &[b"input"]) {
        let applies = el.get_attribute(b"type").is_none_or(|t| {
            !READONLY_TYPES_NOT
                .iter()
                .any(|no| t.eq_ignore_ascii_case(no))
        });
        return Ok(applies && !has_attr(node, b"readonly") && !is_disabled(node, budget)?);
    }
    if html_named(node, &[b"textarea"]) {
        return Ok(!has_attr(node, b"readonly") && !is_disabled(node, budget)?);
    }
    is_editable(node, budget)
}

/// Whether `node` is editable: the nearest HTML element at or above it
/// whose `contenteditable` attribute has a valid value decides - `""`,
/// `true` or `plaintext-only` (an editing host) yes, `false` no; an invalid
/// value inherits, as does no attribute, and the top of the tree is not
/// editable. Every element climbed charges `budget`.
fn is_editable<'d, N: CssNode<'d>>(node: N, budget: &Budget) -> Result<bool, MatchFailure> {
    let mut at = Some(node);
    while let Some(n) = at {
        budget.charge()?;
        if is_html_namespace(n) {
            if let Some(v) = n
                .element()
                .and_then(|el| el.get_attribute(b"contenteditable"))
            {
                if v.is_empty()
                    || v.eq_ignore_ascii_case(b"true")
                    || v.eq_ignore_ascii_case(b"plaintext-only")
                {
                    return Ok(true);
                }
                if v.eq_ignore_ascii_case(b"false") {
                    return Ok(false);
                }
            }
        }
        at = parent_element(n);
    }
    Ok(false)
}

/// `:defined`: false for an HTML element whose name is a valid custom
/// element name - the document has no registry, so none is ever defined - and
/// true for every other element.
pub(super) fn is_defined<'d, N: CssNode<'d>>(node: N) -> bool {
    node.element()
        .is_some_and(|el| !(node.is_html() && is_custom_element_name(el.dom_local_name())))
}

/// HTML's "valid custom element name": a lower-case ASCII letter first, a
/// hyphen somewhere, only PCENChars, and not one of the eight names the
/// SVG and MathML vocabularies reserve.
fn is_custom_element_name(name: &[u8]) -> bool {
    const RESERVED: [&[u8]; 8] = [
        b"annotation-xml",
        b"color-profile",
        b"font-face",
        b"font-face-src",
        b"font-face-uri",
        b"font-face-format",
        b"font-face-name",
        b"missing-glyph",
    ];
    let Ok(text) = core::str::from_utf8(name) else {
        return false;
    };
    let pcen = |c: char| {
        matches!(c,
            '-' | '.' | '0'..='9' | '_' | 'a'..='z' | '\u{B7}'
            | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{37D}'
            | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}' | '\u{203F}'..='\u{2040}'
            | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}'
            | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}' | '\u{10000}'..='\u{EFFFF}')
    };
    text.starts_with(|c: char| c.is_ascii_lowercase())
        && text.contains('-')
        && text.chars().all(pcen)
        && !RESERVED.contains(&name)
}
