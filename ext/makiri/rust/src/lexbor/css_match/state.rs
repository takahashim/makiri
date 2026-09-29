//! What an element IS, for the pseudo-classes that ask about its state
//! rather than its place in a selector: `:empty`/`:blank`/`:root`, the link
//! pseudo-classes, and the form states (`:disabled`, `:checked`, ...) - by
//! the HTML Standard or by Lexbor, as each function's doc says. None of it
//! knows about selectors.

use crate::lexbor::adapter::html::{HtmlNode, NodeType, NsId};

use super::tree::parent_element;

/// Lexbor's own whitespace set for tokenizing an attribute value (`class`, or
/// any `~=` operand) - `lexbor_utils_whitespace`: space, tab, LF, FF, CR.
/// Deliberately NOT `u8::is_ascii_whitespace`, which also matches vertical tab
/// (0x0B) - a real behavioural difference, which `lexbor::tests::css_match`
/// pins.
pub(super) fn is_lexbor_whitespace(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0C | b'\r')
}

pub(super) fn is_html_namespace(node: HtmlNode<'_>) -> bool {
    node.ns_id() == Some(NsId::HTML)
}

fn get_attr<'doc>(node: HtmlNode<'doc>, name: &[u8]) -> Option<&'doc [u8]> {
    node.element().and_then(|el| el.get_attribute(name))
}

pub(super) fn has_attr(node: HtmlNode<'_>, name: &[u8]) -> bool {
    node.element()
        .is_some_and(|el| el.get_attribute(name).is_some())
}

/// `:empty` (`lxb_selectors_pseudo_class`'s `EMPTY` case): a child of ANY type
/// OTHER than Comment disqualifies it, not just Element/non-empty-text - a
/// processing-instruction child does too (found by `spec/xml_css_spec.rb`'s
/// HTML/XML agreement check: `<i><?pi x?></i>` was wrongly treated as `:empty`,
/// since neither `element()` nor `char_data()` sees a PI, and it fell through
/// unnoticed).
pub(super) fn is_empty(node: HtmlNode<'_>) -> bool {
    !node.children().any(|c| c.node_type() != NodeType::Comment)
}

/// `:blank` (`lxb_dom_node_is_empty`, in Lexbor's `dom/interfaces/node.c`): as
/// `:empty`, but a Text child only disqualifies it when it holds a
/// non-whitespace byte - still stricter than "ignore text entirely", and a PI
/// (or anything else that is neither Text nor Comment) disqualifies it
/// unconditionally, same bug/fix as `is_empty` above.
pub(super) fn is_blank(node: HtmlNode<'_>) -> bool {
    !node.children().any(|c| match c.char_data() {
        Some(t) => t.iter().any(|&b| !is_lexbor_whitespace(b)),
        None => c.node_type() != NodeType::Comment,
    })
}

pub(super) fn is_root(node: HtmlNode<'_>) -> bool {
    node.owner_document().as_node().document_root() == Some(node)
}

/// `:any-link` / `:link`, exactly as `lxb_selectors_pseudo_class` has them: an
/// element whose tag is `a`, `area` or `map` (`:any-link`) / `a`, `area` or
/// `link` (`:link`), in any namespace (Lexbor compares the tag id alone, so an
/// SVG `<a>` counts), with an attribute whose local name is `href`, in any
/// namespace (`lxb_dom_element_attr_by_id`, so `xlink:href` counts).
pub(super) fn is_any_link(node: HtmlNode<'_>, link_tag: bool) -> bool {
    let Some(el) = node.element() else {
        return false;
    };
    let tags: [&[u8]; 3] = if link_tag {
        [b"a", b"area", b"link"]
    } else {
        [b"a", b"area", b"map"]
    };
    tags.contains(&el.local_name()) && el.attrs().any(|a| a.local_name() == b"href")
}

/// An HTML element named one of `names` (the stored, lower-cased local name).
fn html_named(node: HtmlNode<'_>, names: &[&[u8]]) -> bool {
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
fn in_disabled_fieldset(node: HtmlNode<'_>) -> bool {
    let mut child = node;
    let mut ancestor = parent_element(node);
    while let Some(a) = ancestor {
        if html_named(a, &[b"fieldset"]) && has_attr(a, b"disabled") {
            let first_legend = a.children().find(|c| html_named(*c, &[b"legend"]));
            if first_legend != Some(child) {
                return true;
            }
        }
        child = a;
        ancestor = parent_element(a);
    }
    false
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
pub(super) fn is_disabled(node: HtmlNode<'_>) -> bool {
    if html_named(
        node,
        &[b"button", b"input", b"select", b"textarea", b"fieldset"],
    ) {
        return has_attr(node, b"disabled") || in_disabled_fieldset(node);
    }
    if html_named(node, &[b"optgroup"]) {
        return has_attr(node, b"disabled");
    }
    if html_named(node, &[b"option"]) {
        return has_attr(node, b"disabled")
            || parent_element(node)
                .is_some_and(|p| html_named(p, &[b"optgroup"]) && has_attr(p, b"disabled"));
    }
    false
}

/// `:enabled`: the elements `:disabled` is defined for, when not disabled -
/// not every other element, as Lexbor's unconditional `!disabled` has it.
pub(super) fn is_enabled(node: HtmlNode<'_>) -> bool {
    html_named(
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
    ) && !is_disabled(node)
}

/// `:checked`, as the HTML Standard defines it: an `input` whose type is
/// Checkbox or Radio and which is checked, or an `option` that is
/// selected - by the `checked` / `selected` attribute, the parsed state.
/// HTML elements only. Lexbor also takes an element with a custom tag and a
/// `checked` attribute; the Standard does not.
pub(super) fn is_checked(node: HtmlNode<'_>) -> bool {
    if html_named(node, &[b"option"]) {
        return has_attr(node, b"selected");
    }
    if html_named(node, &[b"input"]) {
        let checkable = get_attr(node, b"type").is_some_and(|t| {
            t.eq_ignore_ascii_case(b"checkbox") || t.eq_ignore_ascii_case(b"radio")
        });
        return checkable && has_attr(node, b"checked");
    }
    false
}

/// `:optional`/`:required` (`lxb_selectors_pseudo_class`): `input`/`select`/
/// `textarea` only.
pub(super) fn is_form_field(node: HtmlNode<'_>) -> bool {
    node.element()
        .is_some_and(|el| matches!(el.dom_local_name(), b"input" | b"select" | b"textarea"))
}

pub(super) fn is_read_write(node: HtmlNode<'_>) -> bool {
    node.element()
        .is_some_and(|el| matches!(el.dom_local_name(), b"input" | b"textarea"))
        && !has_attr(node, b"readonly")
        && !is_disabled(node)
}
