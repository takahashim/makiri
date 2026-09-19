# Differences from Nokogiri

Makiri targets a Nokogiri-compatible API, but a few behaviours differ. Where
they do, Makiri usually follows the web platform - the WHATWG standards and
what browsers do - rather than libxml2. Detailed, test-backed notes live in
`spec/conformance/README.md`.

## XPath

* The `namespace::` axis is not implemented
  * It raises `Makiri::Error` rather than returning a silently-empty result.
  * Nokogiri (libxml2) supports it (for `<svg>` in HTML it yields the `xml` and `svg` namespace nodes).
    For an element's namespace use `namespace-uri()` / `local-name()`, which are implemented.
* Unprefixed name tests are namespace-strict by default (HTML5/WHATWG-faithful, like browsers' `document.evaluate` and `Nokogiri::HTML5`)
  * `//div` matches, but foreign elements need a registered prefix (`//svg:path`).
    Pass `namespace_matching: :lax` to `Node#xpath` / `XPathContext.new` for the
    namespace-agnostic match where `//path` finds an SVG element (the
    `Nokogiri::HTML`/libxml2-HTML4 behaviour).
* `namespace-uri()` of an HTML element returns the XHTML URI (DOM-correct, as browsers report)
  * `Nokogiri::HTML5` returns `""`.
* Name tests fold ASCII case on HTML elements, like browsers (WPT `domxpath`)
  * `//DiV` matches `<div>` and `//div[@Id]` its `id`; SVG / MathML names compare
    exactly (`//*[@refX]`, not `@refx`). Only ASCII folds: `Ø` still differs from `ø`.
    This holds in `namespace_matching: :lax` too.
  * `Nokogiri::HTML5` is case-sensitive there.
* A foreign element's namespace declarations are not attributes
  * `<svg xmlns="...">` has no `@xmlns` for `//*[@xmlns]` or `@*`, as in browsers
    and in XPath's data model. An `xmlns` on an HTML element is an ordinary
    attribute and stays visible.

## XML

* `Makiri::XML` is XML 1.0 only and non-validating.
  * A `version="1.1"` declaration is rejected; Nokogiri parses XML 1.1.
  * The DTD is recognized but not processed: DTD-defined entities are not
    expanded and DTD default attributes are not applied (Nokogiri/libxml2 can do
    both). External entities/subsets are never fetched (no I/O).
  * Mutation supports in-place edits, the node factories, fragments
    (`Document#fragment` / `DocumentFragment.parse`), node insertion, and building
    a document from scratch (`XML::Document.new` + `#root=`); only handing a raw
    markup string straight to `#add_child` is unsupported (parse it into a fragment
    first). (`#to_xml` serialization is supported; HTML serialization - `to_html`
    / `inner_html` / `outer_html` - is not.)
* A colon in a processing-instruction target is well-formed (`<?a:b ...?>` parses).
  * XML 1.0 §2.6: a `PITarget` is a `Name`, not an NCName, and Namespaces in XML
    1.0's normative conformance section constrains only element/attribute names
    (QNames), never PI targets. Nokogiri/libxml2 rejects it (`colons are forbidden
    from PI names`); Makiri follows the normative text. Only the reserved `xml`
    (any case) target is rejected.
* A node's namespace URI is its identity, not something re-derived from the
  declarations around it - the WHATWG DOM model, measured against Chrome 152
  (`DOMParser` + `XMLSerializer`).
  * Moving a node under an element that binds its prefix to a different URI does
    not change `namespace_uri`; the serializer emits the declaration the
    output needs (`<p:x xmlns:p="urn:a"/>`), and nothing when the destination
    already agrees. libxml2 keeps the URI on an in-document move but does *not*
    emit the declaration, so Nokogiri's tree and its own output disagree there.
  * `#to_xml` on a node below the root is self-contained: it declares the
    prefixes its subtree uses, so the output re-parses to the same namespaces
    standing alone. Nokogiri omits them, and its subtree output does not
    round-trip.
  * Where one prefix would have to mean two things at once, the serializer
    invents one (`ns1`, `ns2`, ...) rather than shadow the other, as browsers do.
  * An element in no namespace stays that way under a default namespace,
    serialized as `xmlns=""`.
  * Nodes from the factories (`create_element` and friends) still take their
    namespace from the context they are first inserted into, so a subtree can be
    built detached and attached afterwards. Only later moves carry.
* Inserting a node from another document adopts it (`add_child` / `before` /
  `after` / `replace`): it is brought over and taken out of the document it came
  from, as `appendChild` does in the DOM and in both Chrome and Nokogiri.
  * Each arena owns its own nodes, so the node cannot be relinked across them: it
    is copied here and removed there. The one visible difference from Nokogiri
    and browsers is that the node handed back is a different object than the
    one passed in - use the return value afterwards, not the argument.
  * `Document#import_node` is the copy: it leaves the source alone, like DOM
    `importNode`.
* Otherwise the parsed tree is byte-identical to `Nokogiri::XML`'s (verified by
  the property-based differential), including namespaces, prolog/epilog comments
  and PIs, and adjacent-CDATA coalescing.

## CSS

* Most jQuery/Nokogiri CSS extensions are not supported (`:gt`, `:lt`, `:eq`, `:first`, ...)
  * Makiri uses Lexbor's selector engine, which is standards-based apart from one
    text-containment extension. Use XPath (`xpath("//p[contains(., 'x')]")`) or
    Enumerable (`css('li')[1]`) for the rest.
    Standard Level-4 selectors (`:is` / `:where` / `:has`) are supported; some of which Nokogiri rejects.
  * `:lexbor-contains("text")` is supported (on both HTML and XML) - Lexbor's
    spelling of the jQuery `:contains()` substring filter, matching an element
    whose text contains the string; append ` i` (`:lexbor-contains("text" i)`)
    for an ASCII case-insensitive match. (Nokogiri's name `:contains` is not an
    alias.) Like Lexbor's matcher, it tests the element's immediate child text
    nodes (not the deep string-value), so HTML and XML agree; on XML it lowers
    to XPath `child::text()[contains(., "text")]`.
* Untyped `:*-of-type` (`:first-of-type`, `:nth-of-type(an+b)`, ... with no type
  selector) is supported and correct on both HTML and XML - the "type" is the
  element's own expanded name.
  * Nokogiri (XML and HTML5) mistranslates these to first-/only-child
    (`//*[position()=1]` / `//*[last()=1]`), so it under-matches; Makiri matches
    Lexbor's HTML matcher.
* Type selectors are ASCII case-insensitive (CSS-correct for HTML; `LI` matches `<li>`)
  * `Nokogiri::HTML5` is case-sensitive there.

## Serialization

* Comment data is written literally, as the WHATWG serialization algorithm
  says and as browsers do: `comment.content = "a-->b"` serializes to
  `<!--a-->b-->`, which re-parses as the comment `"a"` followed by text.
  * `Nokogiri::HTML5` escapes it to `<!--a--&gt;b-->` instead. That does not
    round-trip either - comments do not decode entities, so the data comes back
    as `"a--&gt;b"`. Neither library round-trips this; Makiri matches Chrome.
* The same applies to the children of `style` / `script` / `xmp` / `iframe` /
  `noembed` / `noframes` / `plaintext`, which the algorithm also writes
  literally. `noscript` is escaped, because Makiri parses it with scripting
  disabled (its children are elements, not raw text) and escaping is what makes
  that round-trip; `Nokogiri::HTML5` writes it literally and contradicts its own
  parser there.

## Text input (mutation APIs)

* Programmatic string arguments must be valid UTF-8 (invalid bytes raise
  `Makiri::Error`, never silently repaired - unlike HTML *parsing*, which decodes
  leniently to U+FFFD).
* An embedded NUL (U+0000) is accepted in HTML data-family content -
  text/comment node content (`create_text_node`, `create_comment`, `content=`)
  and attribute values (`[]=`, `set_attribute_ns`) - and stored/read back
  verbatim, matching the WHATWG DOM / browsers (`document.createTextNode("\0")`).
  It is still rejected in names, tag names, namespaces, PI target/data, CSS
  selectors, and XPath expressions/variable names (a NUL there raises).
  * On re-parse, the HTML tokenizer replaces a U+0000 in text/attributes with
    U+FFFD (WHATWG), so a serialized-then-reparsed round-trip is not byte-identical.
  * `Makiri::XML` rejects NUL everywhere: XML 1.0 has no legal U+0000 character,
    so admitting it would produce non-well-formed XML.
