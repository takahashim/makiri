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
    `Nokogiri::HTML`/libxml2-HTML4 behaviour). Lax means "as Nokogiri does", so
    on an XML document it changes nothing: `Nokogiri::XML` is namespace-strict too.
* `namespace-uri()` of an HTML element returns the XHTML URI (DOM-correct, as browsers report)
  * `Nokogiri::HTML5` returns `""`.
* Name tests fold ASCII case on HTML elements, like browsers (WPT `domxpath`).
  The HTML Standard's XPath section does not ask for this - it only sets the
  default element namespace - so here Makiri follows the browsers over the text.
  * `//DiV` matches `<div>` and `//div[@Id]` its `id`; SVG / MathML names compare
    exactly (`//*[@refX]`, not `@refx`). Only ASCII folds: `Ø` still differs from `ø`.
    This holds in `namespace_matching: :lax` too.
  * `Nokogiri::HTML5` is case-sensitive there.
* `Node#path` names a node by expanded name where a bare name would not reach it
  * An SVG, MathML or namespaced-XML node, and an HTML name that is no plain
    XPath name (`<o:p>`, `xml:lang`, Vue's `@click`), give
    `*[local-name()='path' and namespace-uri()='http://www.w3.org/2000/svg']`,
    which `#at_xpath` evaluates with no prefix registered. Nokogiri writes
    `svg:svg` or `/*/*[2]` for the first kinds; the paths are equivalent, the
    strings are not.
  * A node not attached to its document answers `"?"`, as a doctype does.
    Nokogiri answers `"/div/p"` for a detached `<div><p>`, which is the path of
    the document's own `/div/p` when it has one.
* A foreign element's namespace declarations are not attributes
  * `<svg xmlns="...">` has no `@xmlns` for `//*[@xmlns]` or `@*`, as in browsers
    and in XPath's data model. An `xmlns` on an HTML element is an ordinary
    attribute and stays visible.

* A number literal is read as the nearest double; libxml2's own reader is not
  correctly rounded for a literal with more digits than a double holds, so
  `string(0.72609133372266155)` is `0.726091333722662` in Makiri and
  `0.726091333722661` in Nokogiri (the digits differ in the last place). The
  number is then written by libxml2's rule in both (`string(1234567890.5)` is
  `1.2345678905e+09`); only the value read differs.

## XML

* `Makiri::XML` is XML 1.0 (Fifth Edition) only and non-validating.
  * A `version="1.x"` document is read as XML 1.0, as §2.8 says a 1.0 processor
    does; a construct only XML 1.1 allows still fails.
  * The internal DTD subset is checked but never applied. §5.1 requires even
    a non-validating parser to apply its attribute defaults and entities, so a
    document whose DTD would change the tree is refused
    (`Makiri::XML::SyntaxError`, "unsupported DTD construct") rather than parsed
    without them: an attribute default (`"d"` or `#FIXED "d"`), a non-CDATA
    attribute type (`ID`, `NMTOKEN`, an enumeration, ... - they normalize the
    value), a parameter-entity reference, or a reference to an entity the DTD
    declares. Declarations that change nothing (`<!ELEMENT>`, `CDATA #IMPLIED` /
    `#REQUIRED`, unused entity declarations, notations) are accepted.
    Nokogiri/libxml2 by default parses such documents and leaves the defaults and
    entities out (its `DTDATTR` / `NOENT` options apply them).
  * External entities and the external subset are never fetched (no I/O), as
    §5.1 allows a non-validating parser.
  * Mutation supports in-place edits, the node factories, fragments
    (`Document#fragment` / `DocumentFragment.parse`), node insertion, and building
    a document from scratch (`XML::Document.new` + `#root=`); only handing a raw
    markup string straight to `#add_child` is unsupported (parse it into a fragment
    first). (`#to_xml` serialization is supported; HTML serialization - `to_html`
    / `inner_html` / `outer_html` - is not.)
* A processing-instruction target with a colon can be created but not written.
  * The parser rejects `<?a:b ...?>`, as Nokogiri does: Namespaces in XML §7
    requires every Name other than element and attribute names to be an NCName.
  * `create_processing_instruction("a:b", ...)` succeeds, as DOM
    `createProcessingInstruction` does, but `#to_xml` / `#canonicalize` then raise,
    as DOM Parsing's well-formed serializer does. Nokogiri writes `<?a:b ...?>`.
* `#freeze` on a node is ENFORCED: a frozen node's mutators raise `FrozenError`,
  and so does passing a frozen node as the argument of an insertion, which
  relinks it. Nokogiri reports `frozen?` but every mutator still mutates. The
  check reaches the nodes the caller named; a fragment argument splices its
  children, and those cannot be checked, because frozen-ness is a property of a
  Ruby object and the arena keeps no map from a node back to its wrapper.

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

* `to_xml` keeps an element's namespace when an `xmlns` attribute on it says
  otherwise
  * `root["xmlns"] = "urn:x"` on an element in no namespace is not written: the
    element stays in no namespace when the output is re-read, as the DOM Parsing
    and Serialization spec asks. Nokogiri writes `<r xmlns="urn:x">`, which moves
    the element into `urn:x` on re-parse. Create the element in the namespace
    instead (`create_element("r", "xmlns" => "urn:x")`, or parse it so).

## HTML parsing

* `<?php ... ?>` in HTML input is a **ProcessingInstruction** node; `#to_html`
  writes it back as `<?php ... ?>`, and `<?` at end of input is ignored.
  * The HTML Standard added processing-instruction tokens and tree-construction
    rules for them; Makiri follows them through Lexbor. `Nokogiri::HTML5` (gumbo)
    still produces the older bogus comment (`<!--?php ... ?-->`), and
    `Nokogiri::HTML` (libxml2) its own comment.

* A tree deeper than `max_tree_depth` raises **`Makiri::Error`**
  ("document tree depth limit exceeded (400)"), where `Nokogiri::HTML5` raises
  `ArgumentError` ("Document tree depth limit exceeded"). The default (400),
  the "negative disables it" rule and the boundaries are Nokogiri's: a document
  holds 398 nested elements inside `<html><body>`, a fragment 400.
  * `Makiri::HTML` / `Makiri.parse` / `HTML::Document.parse`,
    `HTML::DocumentFragment.parse` and `Document#fragment` take the keyword;
    `inner_html=`, `outer_html=` and `Node#parse` always use the default.
    Nokogiri's `Document#fragment` takes no options.
  * The other Gumbo limits (`max_attributes`, `max_errors`) do not exist; an
    unknown keyword is an `ArgumentError`.
  * `Nokogiri::HTML` (libxml2) stops at depth 256 instead, and returns the
    TRUNCATED document with the error in `#errors`; Makiri never returns a
    partial tree.

* A `<select>` that receives more than 10,000 `<option>`s during a parse
  raises **`Makiri::Error`** ("too many option elements in one select element
  (limit 10000)"); Nokogiri has no such limit. Lexbor's option insertion is
  quadratic in the select's option count, so without it 40,000 options took
  4 seconds. Options that update no select (in a `<datalist>`, or outside any
  select) are not counted.

## HTML mutation

* There is no `Node#name=` / `#node_name=` (on HTML or XML nodes). Nokogiri
  renames a node in place and keeps its identity; the DOM has no rename, and
  Lexbor keeps elements such as `<template>`, `<option>` and `<style>` in
  structs of their own, so a node cannot change its tag safely. Create an
  element of the new name, move the attributes and children, and `replace`
  the old one (see CHANGELOG.md).
* Element and attribute names follow the WHATWG DOM's rules
  * `create_element`, `[]=` and `set_attribute_ns` raise `ArgumentError` for a
    name the DOM refuses - one holding whitespace, `/`, `>` (or `=` for an
    attribute) - where `Nokogiri::HTML5` accepts it and writes it into the
    markup as it stands: `create_element("img src=x onerror=alert(1)")`
    serializes as that tag.
    The names HTML actually uses (`data-*`, `aria-*`, `@click`, `:href`,
    `v-on:x`, custom elements) are accepted.
  * `set_attribute_ns(nil, "x:y")` raises, as the DOM's `setAttributeNS` does:
    a prefix needs a namespace.
* An HTML `<template>` follows the WHATWG content model, which Nokogiri does
  not: its parsed contents live in the separate fragment `Element#content_fragment`
  returns, `template.children` is empty, and `inner_html` / `inner_html=`
  special-case the contents (the WHATWG DOM special-cases `innerHTML` alone;
  `append_child`, `content=`, and `children` act on the element's own empty
  children, as the specification says). Nokogiri treats `<template>` as an
  ordinary element, with the parsed nodes as its children, so
  `template.inner_html` and `template.children` answer the other way round and
  there is no `content_fragment`.
* An HTML document has one root element and no text child, as the DOM requires;
  `doc << element` beside an existing root raises.
* Moving HTML into an XML document (`xml_doc.import_node(html_node)`, or
  inserting one) keeps every name's namespace, and refuses what XML cannot
  write that way. An attribute in no namespace whose name has a prefix other
  than `xml` - `v-on:click`, `fb:like`, an `xlink:href` on an HTML (not SVG)
  element - raises `Makiri::Error`: as XML it would be a prefix bound to
  nothing. Nokogiri copies it and writes `v-on:click="..."` into output that is
  not namespace-well-formed. An element named with a colon (`<fb:like>`)
  crosses as a DOM-loose name, which `to_xml` refuses.
* A known gap, in Lexbor's tag table: an HTML document that already holds a
  parsed element named with a colon (`<x:y>`, one local name) and then
  receives, by `import_node` from another document, a prefixed element
  written the same way (`x:y` from XML: prefix `x`, local name `y`) re-points
  the table's entry for that spelling - the parsed element then no longer
  matches CSS `x\:y`. Copies within one document do not touch the table.
* An XML element with a prefix, imported into HTML, keeps it (`h:div` in
  XHTML has the local name `div`). Two readers then disagree, as they do in
  browsers: CSS's `div` matches it (Lexbor matches the local name), XPath's
  `//div` does not (an HTML element's name test reads its qualified name). And
  Lexbor's HTML serializer writes the prefix (`<h:div>`), where the HTML
  standard writes the local name, so the HTML does not re-parse to the same
  element.

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
* HTML CSS takes a `{prefix => uri}` Hash (`css(selector, ns)`) but does not
  resolve prefixes against it: Lexbor's matcher matches a prefixed type selector
  loosely, so `svg|path`, `|path` and `path` all find the SVG element whatever
  is bound. `Nokogiri::HTML5` honours the binding (a wrong URI finds nothing).
  * The Hash is accepted and unused rather than refused, so the `css(selector,
    ns)` a caller writes for both representations works. Use `#xpath`, where a
    prefix IS resolved against the bindings, when the namespace matters.
  * `Makiri::XML` resolves CSS prefixes properly - it lowers the selector to the
    XPath engine, which registers the bindings.
  * The same holds for attribute selectors: HTML `[|href]` (no namespace) and
    plain `[href]` also find an SVG `xlink:href`, which `Nokogiri::HTML5`
    does not.
    `Makiri::XML` reads `[|a]` as the no-namespace attribute.
* A selector under a node matches the way `Element#querySelectorAll` does in a
  browser, not scoped to that node, on HTML: `at_css("#c").css("div p")` finds a
  `p` inside `#c` when `#c` is itself a `div`, since the selector is matched
  against the whole document and only the results are kept to descendants.
  `Nokogiri::HTML5` and `Nokogiri::XML` scope the selector to the node (`#c`
  cannot be the `div`), and so does `Makiri::XML`, which lowers the selector to
  an XPath from the node. Lexbor has no `:scope`; for a scoped match on HTML,
  use XPath from the node (`xpath(".//div//p")`).
* The attribute case modifiers (`[a="x" i]`, `[a="x" s]`): HTML supports both,
  through Lexbor's matcher. `Makiri::XML` accepts `s` (case-sensitive, which XML
  values are anyway) and refuses `i` with `Makiri::CSS::SyntaxError`. Nokogiri
  refuses both on either representation.
* `#matches?` answers for a DETACHED node (`document.create_element("p")
  .matches?("p")` is true, on both representations). Nokogiri raises
  `NoMethodError` there - it implements `#matches?` as a search from
  `ancestors.last`, which a detached node does not have.
* * Type selectors are ASCII case-insensitive (CSS-correct for HTML; `LI` matches `<li>`)
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
