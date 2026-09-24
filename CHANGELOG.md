# Changelog

## [Unreleased]

### Security

* `content=` on an HTML element and `delete(name)` no longer free the nodes they
  remove. Both went through Lexbor calls that destroy them, while a Ruby
  wrapper may still hold one: the wrapper then read freed memory, and the next
  node allocated there came back under it - a text node answering as an
  Element or an Attr. They detach now, as every other mutator does.
* `el[name] = value` on an attribute the element already has no longer goes
  through `lxb_dom_element_set_attribute`, which destroys the attribute - still
  linked into the element - when storing the new value runs out of memory. The
  value is set directly, and a failure leaves the attribute as it was.
* A checked argument String is locked while its bytes are borrowed. A call
  converts its arguments one at a time, and a later argument's `#to_s` could
  rewrite an earlier one - putting a NUL into a name that had passed its
  check, or growing it so the view read freed memory into the DOM. Such a
  `#to_s` now raises "can't modify string; temporarily locked".
* A receiver frozen by its own argument's `#to_s` is no longer edited.
* HTML element and attribute names follow the WHATWG DOM's rules: `name=`,
  `create_element`, `[]=` and `set_attribute_ns` raise `ArgumentError` for a
  name holding whitespace, `/`, `>` (or `=`), which was written into the markup
  as it stood - `name = "img src=x onerror=alert(1)"` serialized as that tag.
  See NOKOGIRI_DIFFERENCES.md.
* More inputs whose cost outgrew their size: a single-context reverse-axis
  step (`preceding-sibling`, `ancestor`) is reversed rather than merge-sorted
  (4000 siblings: 3.9 s); XML CSS `:nth-child` / `:nth-of-type` /
  `:first-of-type` and kin read a per-parent memo of positions (a 10k-entry
  sitemap took seconds, or hit the budget); a CDATA value full of `]]>` and a
  stylesheet full of rewritten `:lexbor-contains()` rules are linear again.

* A mutator's argument can no longer rebuild the document's indexes in the
  middle of the edit. Arguments are converted with `#to_s`, which is arbitrary
  Ruby; a query made there rebuilt the indexes from the tree the edit was about
  to change, after which `#text` read text storage the edit had released (a
  read of freed memory) and `//p` found removed nodes. Both representations;
  every mutator.
* Inputs whose cost grew faster than their size, with no budget to stop them,
  are linear or budgeted now: the `preceding` axis (depth 2000 took 8.8 s),
  `Makiri::Lexbor::CSS.parse_stylesheet` after one rejected
  `:lexbor-contains()` (186 KB took 4.6 s), the XML parser's duplicate-attribute
  check (100 elements of 4096 attributes took 16.6 s), `contains` /
  `substring-before` / `substring-after` and `translate` over long strings, and
  - charged to the op budget, so they raise `XPath::LimitExceeded` - a node's
  string-value, `lang()` and CSS `:nth-of-type` over XML.
* A panic below mutators, factories, `clone_node` / `import_node`,
  `XPathContext.new` and its setters, `Node#line`, `Attr#parent` and `#<=>` -
  all of which walk a tree built from input - raises `Makiri::InternalError`
  rather than `fatal`, as parsing and querying already did.

### Fixed

* A namespace Hash given to a query is read as a Hash, not through a `to_a`
  a subclass may redefine (a non-pair raised `Makiri::InternalError`), and
  each prefix and URI is read with `String()`, preferring `to_str`, as other
  arguments are. `XPathContext#register_namespace` refuses inside a handler
  before converting its arguments.
* An XML attribute compares equal to itself with `<=>`, as an HTML one does.
* XPath `string()` of a number follows libxml2's rule, as Nokogiri does:
  exponential notation above 1e9 and below 1e-5 (`1234567890.5` is
  `1.2345678905e+09`, `0.00001` stays `0.00001`), and integer form only inside
  C's `int` (`2147483647` is `2.147483647e+09`). It was C's `%.15g`, which
  disagreed with Nokogiri outside `[1e-4, 1e15)`.
* HTML-to-XML `import_node` no longer moves an element into another namespace
  when one of its attributes uses the element's prefix for a different URI
  (`p:e` in `urn:p` with an attribute `p:x` in `urn:other` came out in
  `urn:other`). Namespaced attributes cross with their namespace given
  directly, and a prefixed element is declared once, not on every descendant.
  A malformed attribute name (`:class`) is refused with `ArgumentError` again.
* A namespace given with `XML::Node#set_attribute_ns` on a detached element
  survives the element's insertion. The insertion re-derived it from the
  prefix, so `set_attribute_ns("urn:a", "x", v)` ended up in no namespace.
* `Makiri::XML` CSS reads `[|a]` as the no-namespace attribute, as the
  Selectors spec does; it was refused as the unsupported `[*|a]`. The `s`
  attribute modifier is accepted (XML values compare case-sensitively anyway);
  `i` is still refused.
* XPath resolves the `xml` prefix to its fixed namespace (`//@xml:lang`), with
  no registration and whatever one says, as Namespaces in XML binds it and
  Nokogiri answers. It raised "unknown namespace prefix".
* `Makiri::XML` nodes compare by document order with `<=>`, as HTML nodes
  do, so they sort; `<=>` returned nil for every pair.
* `XPathContext#register_namespace` reads its arguments as a namespace Hash
  does: both converted with `to_s` before either is checked, the same
  string-length cap, and the same "invalid namespace mapping" message. It
  had no cap, and worded a refusal differently.
* Every Ruby method Makiri defines turns an internal panic into
  `Makiri::InternalError`. Readers such as `children`, `[]`, `keys`,
  `NodeSet#each` and `Document#title` still raised `fatal`, which cannot be
  rescued in the frame that called them. `rake unsafe:boundaries` now fails on
  a method whose body does not go through `entry`.
* Namespaces across `import_node` between HTML and XML:
  * HTML to XML reads each attribute's own namespace, as `Attr#namespace_uri`
    does. A parsed `q:y` inside `<svg>` (no namespace) was put in SVG.
  * An HTML attribute in no namespace whose name has a prefix other than `xml`
    (`fb:like`, a parsed `xlink:href` on an HTML element) has no XML form, and
    the import now refuses it. The copy used to be made with a prefix bound
    to nothing, and then could be neither inserted nor serialized.
  * An HTML element named with a colon (`<fb:like>`) crosses as a DOM-loose
    name, like other names XML cannot write: it can be inserted, and
    `to_xml` refuses it.
  * XML to HTML makes a prefixed element with its prefix, so `p:e` has the
    local name `e` and `//q:e` finds it. It used to have the local name `p:e`.
  * An attribute set by `set_attribute_ns` in its element's own namespace
    (`set_attribute_ns(SVG, "q:x")` on an SVG element) reports that namespace;
    it read as none.
* An XPath comparison over node string-values no longer raises `LimitExceeded`
  because the values it compared added up past 64 MB. That total was the
  per-string cap reused for the evaluation's string-value cache, so
  `//*[. = "x"]` raised on a page where `//*[string(.) = "x"]` answered. The
  cache has its own cap now and stops keeping values past it. Building a
  string-value is charged to the op budget by its size (one op per 64 bytes),
  so a comparison that rebuilds large uncached values fails fast rather than
  copying gigabytes.
* A refused XML namespace declaration says which rule it broke - declaring
  `xmlns`, binding `xml` elsewhere, binding a reserved namespace to another
  prefix or as the default, or binding a prefix to the empty namespace -
  instead of one message listing all of them.
* A rejected stylesheet rule's `selector_text` is sliced by Lexbor's own
  offsets, so it can no longer come from an identical piece elsewhere in the
  sheet, and a declaration value no longer shows the `:lexbor-contains()`
  guard's `zzzz` rewrite.
* An HTML attribute's parent is Lexbor's `attr->owner`, read live: a detached
  element's attribute had a parent or not depending on whether the document
  had been queried first, and a fragment's attributes had none.
* XML namespaces: an attribute whose prefix was unbound on a detached element
  is resolved when the element is inserted (it was written as `xmlns:ns1=""`),
  insertion refuses two attributes that end up with one (namespace, local
  name), a rename while detached is resolved on insertion, both writers refuse
  a prefix bound to nothing, and `set_attribute_ns` refuses a namespace that
  does not fit the name (a prefix with none, the XML namespace under another
  prefix, ...).

* `Makiri::XML#to_xml` output re-parses in cases it did not: a CDATA value
  holding `]]>` (adjacent sections merge, as in libxml2) is split across two
  sections as libxml2 writes it; a SYSTEM id holding `"` is single-quoted; an
  attribute with a namespace but no prefix gets a declared prefix instead of
  losing its namespace; an `xmlns` attribute contradicting a no-namespace
  element is left out rather than inventing `xmlns:ns1=""` (see
  NOKOGIRI_DIFFERENCES.md); and an element copied in but not yet inserted keeps
  its own declaration.
* The XML mutators enforce the rules the parser does. `[]=`,
  `set_attribute_ns` and `name=` refuse a namespace declaration Namespaces in
  XML §3 forbids (`xmlns:xml` to another URI, `xmlns:xmlns`, a reserved URI
  under another prefix) and a second attribute with the same namespace and
  local name; `create_document_type` refuses a name that is no QName and a
  PUBLIC id outside PubidChar.
* `Makiri::XML::Node#canonicalize` raises when the document's declarations no
  longer give a name its namespace (a node moved from under its declaration,
  one removed), where it rendered a different namespace or an unbound prefix.
* Importing HTML into XML no longer writes `xmlns:xmlns` for a foreign
  element's `xmlns:xlink`, which made the output unreadable, and no longer turns
  an HTML element's `xmlns` attribute into a declaration that moved it out of
  XHTML.
* An HTML document refuses a second root element and a text child, as the DOM
  requires and the XML side already did.
* XPath: `substring()` rounds each argument by `round()`'s rule, as §4.2 says
  (`substring("12345", 1.5, 2.6)` is `"234"`, was `"23"`); `<`, `>`, `<=`, `>=`
  between a node-set and a boolean compare the node-set's boolean (§3.4), as
  `=` did; and a string beginning with U+0000 is true.
* CSS over XML agrees with the HTML matcher on an empty attribute value
  (`[a^=""]` matched every element, attribute or not) and whitespace in `~=`
  (both match nothing), on `$=` with a non-ASCII value, and on `:empty` beside
  a comment.
* `Node#line` of a node copied from another document is nil, where it
  answered with a line of this document the node was never on.

* `Node#path` round-trips through `#at_xpath` for CDATA sections and processing
  instructions, and for text next to a CDATA section. A CDATA section is a
  `text()` step counted among its text siblings, and a PI is
  `processing-instruction('target')`, as Nokogiri writes them; before, the path
  was `#cdata-section` (a syntax error) or the PI's target as an element name.
  A node XPath cannot reach - a doctype, a namespace declaration, or a node
  inside a `DocumentFragment` - answers `"?"`, as Nokogiri does, instead of
  `/#document-fragment/...`. So does a node not attached to its document, which
  Nokogiri does not do: its `/div/p` could name the document's own `/div/p`
  (see NOKOGIRI_DIFFERENCES.md).
* `Node#path` round-trips for namespaced nodes too: SVG and MathML in HTML,
  default-namespace and prefixed XML, and namespaced attributes (`xlink:href`).
  These are named by expanded name -
  `*[local-name()='path' and namespace-uri()='http://www.w3.org/2000/svg']` -
  which needs no prefix registered; before, the path found nothing, or raised
  `unknown namespace prefix`. An SVG `<a>` no longer shares a position count
  with the HTML `<a>`s beside it. The same holds for HTML names that are no
  plain XPath name: Word's `<o:p>`, `xml:lang`, `xmlns:v` on an HTML element,
  and Vue's `@click` / `:href` / `v-on:x`, whose paths raised
  `unknown namespace prefix` or `XPath::SyntaxError`.
* Copying an XML doctype (`import_node`, `clone_node`, and so `Document#dup`)
  keeps its PUBLIC id. The copy read the id's length as a name-prefix length:
  the PUBLIC id came back as the name's bytes and whatever followed them, an
  absent one as `PUBLIC ""` - and where that ran past the end of the new
  document's store, the first `#public_id` raised `fatal`.
* `Makiri::XML::Document#dup` / `#clone` return a copy. They returned the
  document itself, so `xml.clone(freeze: true)` froze the original. `#dup`
  re-parses the serialisation, as `Makiri::HTML::Document#dup` does (a document
  with no root yet is copied node by node), with the default `max_bytes`.
* `Node#clone_node` on a Document raises `Makiri::Error` in both
  representations, instead of returning the document itself as its "copy"
  (and, in XML, leaving a stray node in the arena).
* `NodeSet#xpath` / `#at_xpath` / `#search` with an expression that evaluates
  to a string, number or boolean raise `ArgumentError`, as Nokogiri does,
  instead of `NoMethodError` from inside the union.
* `Makiri::XML::Builder` and its `NodeBuilder` no longer claim Ruby's implicit
  conversions (`to_ary`, `to_str`, ...) through `respond_to?`, so `Array(builder)`,
  `puts` and splats no longer build a `<to_ary>` element or add a `to_ary` class.
* `NodeSet#css` / `#xpath` / `#at_css` / `#at_xpath` pass their further
  arguments (a namespace map, a handler, `namespace_matching:`) to each node's
  query, as `Node`'s take them; they used to accept the expression alone.

### Performance

* `NodeSet#at_css` / `#at_xpath` stop at the first node with a match instead of
  querying every node and building the union.
* `XML::Node#canonicalize` no longer walks up the ancestors for each namespace
  declaration it renders; it reads the scope it already keeps, and that scope
  is indexed by prefix, so a lookup no longer costs the depth of the
  declarations in scope. A 1000-deep document of declarations went from 0.7 s
  to 0.02 s, and documents with thousands of declarations in scope, which ran
  both `to_xml` and `canonicalize` out of their namespace step budget, now
  serialize.

## [0.10.0] - 2026-09-22

### Fixed

* A frozen node now raises `FrozenError` when it is the ARGUMENT of a tree
  mutation, not only the receiver: `b.add_child(a)` relinks `a` exactly as
  `a.remove` does. Both `Makiri::XML` and `Makiri::HTML`. A fragment argument
  splices its children, which have no wrapper of their own to check.
* A `Makiri::XML` mutation that exceeds the document's own `max_bytes` /
  `max_nodes` now raises `Makiri::XML::LimitExceeded` instead of reporting the
  refusal as out of memory.
* Inserting a `DocumentFragment` is all or nothing on `add_child`, `before` and
  `after`, as it already was on `replace`: a child the rules refuse no longer
  leaves the earlier ones linked in a document the caller was told had not
  changed.
* A rejected `Makiri::XML::Document#fragment` no longer charges the document for
  the nodes it discarded. 100k rejected fragments grew a `<r/>` document to
  77 MB; it is now 516 bytes.
* A failed `Makiri::XML` parse reports the FIRST failure for all four kinds
  (`syntax` was sticky while `limit` and `unsupported` overwrote each other).
* `Makiri::XML#to_xml`'s serializer allocates fallibly again, so running out of
  memory raises instead of aborting the process.
* `:lexbor-contains()` now rejects an argument the bundled CSS parser does not
  take, the way any unknown pseudo-class is rejected: `Makiri::CSS::SyntaxError`
  from `#css` / `#at_css` / `#matches?`, and a `:bad_style` rule from
  `Makiri::Lexbor::CSS.parse_stylesheet`. Well-formed uses are unchanged.

### Performance

* `Makiri::XML#to_xml` plans namespaces from a binding stack instead of
  re-walking each ancestor's attribute list, which cost O(depth^2 x attributes).
  403 KB of nested prefixed attributes took 4.88s and now takes 0.001s. A
  crafted document fails closed with `Makiri::Error` ("namespace planning
  exceeded its step budget") rather than running on.
* Setting an attribute on a `Makiri::XML` element walks the attribute list once
  instead of twice (4096 attributes: 47ms -> 27ms).

## [0.10.0.rc2] - 2026-09-20

### Added

* `XPathContext.new` now registers namespace bindings passed via
  `prefix: uri` (previously ignored).
* `#css`, `#at_css`, and `#matches?` accept `(selector, namespaces = nil)`
  on HTML documents for consistency with XML.
* Standardized error message format for rejected CSS selectors across XML
  and HTML: `"<reason>: <selector>"`.
* Unified argument handling for `#xpath` and `#at_xpath` between HTML and XML.
  * HTML supports per-query namespace bindings and keyword prefixes
    (e.g., `s: uri`).
  * XML supports custom-function handlers.
* XML nodes now support HTML-style node readers and aliases
  (`#first_element_child`, `#next_element`, `#tag_name`, `#attr`,
  `#node_name`, etc.).

### Changed

* Updated vendored Lexbor to `v3.0.0-66`.
* Parsed `<?php ... ?>` tags in HTML input as `ProcessingInstruction` nodes
  instead of comments, adhering to the HTML Standard.
* Vendored Lexbor updated from `v3.0.0-25` to `v3.0.0-66` (`05b5d37`), for an
  out-of-bounds write in `lxb_dom_character_data_replace` reachable through
  `#content=`, UTF-16 surrogate rejection, exact `meta` charset matching and
  buffer-capacity fixes. (v3.0.1 is not usable: it lacks the two CSS-selector
  fixes Makiri upstreamed.)
* `<?php ... ?>` in HTML input is now a `ProcessingInstruction` node rather
  than a Comment, serializes as `<?target data?>`, and `<?` at end of input is
  ignored - the HTML Standard's processing-instruction tokens, which Lexbor now
  implements. `Nokogiri::HTML5` still answers with a comment.
* `Makiri::XML#matches?` tests the node locally via tree walking instead of
  performing a full-document search.
* `Makiri::XML::Namespace` is now an immutable `Data` object defined in Ruby.
* `XML::Node#value` returns its text content (or attribute value for
  attributes), aligning with HTML node behavior.
* XPath over HTML now handles element/attribute names case-insensitively
  and properly handles `xmlns` attributes according to browser specs.
* `Makiri::XML` checks the internal DTD subset for well-formedness and
  raises `Makiri::XML::SyntaxError` if unsupported constructs (e.g., attribute
  defaults, entity references) would alter the document tree.
* Documents with `version="1.x"` are now accepted and parsed as XML 1.0.
* Processing instruction (PI) targets containing colons (e.g., `<?a:b?>`)
  are now rejected per Namespaces in XML specs.

### Fixed

* `Makiri::XML::DocumentType#prefix` now returns `nil` instead of
  the PUBLIC ID.
* `Makiri::XML#last_element_child` now correctly returns an `Element`
  instead of arbitrary child nodes (like comments or text).
* `Element#local_name` and `local-name()` in XPath preserve camelCase
  for SVG/MathML elements (e.g., `foreignObject`).
* `inner_html=` and `outer_html=` are now atomic operations to prevent
  leaving the DOM in a corrupted state on failure.
* XPath over HTML assigns empty/correct namespaces to attributes instead of
  inheriting from their parent element.
* Corrected `lang()` evaluation to check the nearest language attribute
  hierarchically.
* Fixed `namespace_matching: :lax` on XML to be strict, aligning with
  `Nokogiri::XML` / `libxml2` behavior.
* Moving an HTML node to another document now correctly removes it
  from the source document's indexes.
* Prevented XPath custom handlers from creating new nodes on the document
  being evaluated.
* Fixed CSS universal selectors (`p|*`, `|*`) on XML to respect namespace
  boundaries.
* CSS column combinator (`a || b`) on XML now raises
  `Makiri::CSS::SyntaxError` instead of being misparsed as descendant selector.

## [0.10.0.rc1] - 2026-09-19

### Changed

* The native extension is rewritten in Rust. The C glue, the XPath engine,
  the XML reader and the CSS lowering are now one Rust crate; the only C left is
  the vendored Lexbor, still unpatched. The Ruby API is unchanged, and answers
  were checked against the C build's recorded output as well as the existing
  differential suites against Nokogiri.

  * Installing from source needs a Rust toolchain. `cargo` (stable) and
    libclang are required alongside CMake, and `rb_sys` becomes a runtime
    dependency of the source gem, because its `extconf.rb` runs at install time.
    The precompiled platform gems need none of this and do not depend on
    `rb_sys`.
  * Faster. Makiri now beats both Nokogiri and nokolexbor on every
    `rake bench` row, parse included - previously ~1.25x slower than nokolexbor.
    Source locations are stamped on the first `#line` or mutation rather than
    during every parse, and the vendored Lexbor is built with link-time
    optimization where the linker supports it (macOS; Linux with clang,
    llvm-ar and lld; not Windows).
  * An internal failure is an exception, not a crash. It used to end the
    host process with SIGABRT. Now it unwinds on the thread that ran the call:
    `ensure` blocks run and the process keeps working. On entry points that
    handle input (parse, XPath, CSS, serialization, text) it is the new
    `Makiri::InternalError`, which descends from `Exception`, not
    `StandardError`, so a bare `rescue => e` does not swallow it; elsewhere it
    is Ruby's `fatal`.
  * An XPath handler may not modify the document being evaluated. Every
    mutator on that document raises `Makiri::Error` while an evaluation with a
    handler runs, because the evaluator holds names and values from it for the
    whole walk.

### Fixed

* Reading text no longer crashes when a comment, processing instruction or text
  node stands before the root element of a document without `<html>`.
* A handler that evaluates again on its own `XPathContext` no longer leaves the
  outer walk without its handler or with reset operation budgets.

## [0.9.0] - 2026-09-11

### Added

* `Node#attribute_by_qualified_name(name)` and
  `Node#attribute_value_by_qualified_name(name)`: the attribute whose
  qualified name is exactly `name` — its node, and its value — or nil.
  `#[]` cannot answer this: it also finds a prefixed attribute by its local
  name (`svg_a["href"]` returns `xlink:href`), and it lower-cases what it looks
  up (`el["DATA-X"]` finds `data-x`). The new match is byte-exact.

### Changed

* `Makiri::XML` follows the DOM namespace model. A node's namespace URI is
  decided once — by the parser, or by the context it is first inserted into —
  and does not change afterwards; the serializer emits whatever xmlns
  declarations the output needs:

  - moving a node under an element that binds its prefix to a different URI
    keeps `namespace_uri`, and the output declares the prefix at the moved node;
  - inserting a node whose prefix is bound nowhere in the destination now
    succeeds instead of raising;
  - `import_node` keeps the namespace of what it copied;
  - an element in no namespace stays in no namespace under a default namespace,
    serialized as `xmlns=""`;
  - `#to_xml` on a node below the root declares the prefixes its subtree uses,
    so the output re-parses to the same namespaces standing alone.

  Nodes from the factories (`create_element` and friends) still take their
  namespace from the context they are first inserted into.

* Inserting a node from another document adopts it. `add_child` / `before` /
  `after` / `replace` bring the node over and remove it from the document it
  came from, instead of copying it (`Makiri::XML`) or raising (`Makiri::HTML`).
  A spliced fragment is left empty; a rejected insert leaves the source
  document untouched.

  The node handed back is a different object than the one passed in, so use
  the return value afterwards rather than the argument. `Document#import_node`
  is unchanged: it copies and leaves the source alone.

* XML serialization is capped at the nesting depth the parser accepts.
  `#to_xml` and `#canonicalize` raise `Makiri::Error` past it, rather than
  emitting XML that Makiri could not read back. A deeper tree is still fine to
  hold, walk and query.

### Fixed

* XPath axes from an attribute context node on the XML backend now follow
  XPath 1.0 §2.2: `following-sibling` and `preceding-sibling` are empty, and
  `following` / `preceding` exclude attribute nodes. They used to return the
  element's later attributes.

* A rejected insert no longer leaves part of the moved subtree carrying
  namespace URIs resolved against the scope it was refused from.

## [0.8.0] - 2026-07-12

### Fixed

* `Document#import_node` (DOM `importNode` / `adoptNode`) now imports an HTML
  element whose name is a valid DOM name but not a well-formed XML QName (e.g.
  `":good:times:"`, `"x<"`, `"0:a"`) instead of raising. Such an element is not
  XML-serializable (`#to_xml` raises). An element carrying a similarly lenient
  attribute name is still rejected.

## [0.7.0] - 2026-07-11

### Added

* `Document#create_document_type(name, public_id = "", system_id = "")` on both
  `Makiri::HTML::Document` and `Makiri::XML::Document` (DOM
  `DOMImplementation.createDocumentType`): creates a detached `DocumentType`
  node, to be placed before the document element (e.g. with
  `root.add_previous_sibling(doctype)`). The name is case-preserving; an omitted
  or empty public/system id is treated as absent; an invalid name (or a `"` in an
  id) raises. Inserting a doctype is validated against the WHATWG rules — it may
  only be a child of the document, must precede the document element, and a
  document holds at most one — so a misplaced or duplicate doctype raises rather
  than producing a malformed tree.

* The XML `DocumentType` is now part of the tree: it appears in
  `Document#children` (before the root, like a browser DOM) and carries
  `parent`/sibling links, for both a parsed `<!DOCTYPE>` and the factory above.
  `Document#internal_subset` still returns it. XPath is unaffected — its 1.0 data
  model excludes the doctype (`//node()` / `/node()` never surface it), as in
  Nokogiri/libxml2. This supports building a document with
  `DOMImplementation.createDocument(namespace, qualifiedName, doctype)`.

* `Makiri::XML::Document#create_loose_dom_element(qualified_name, prefix,
  local_name, namespace_uri)`, an escape hatch for browser-DOM interop that
  creates an element whose name follows WHATWG DOM element-name rules rather
  than the stricter XML QName rules (e.g. `"f:o:o"`, `":foo"`, `"0:a"`). The
  regular factories stay XML-strict. Such an element is intentionally not
  XML-serializable: `#to_xml` and `#canonicalize` raise `Makiri::Error` if the
  tree contains one. Renaming it to a valid XML name (`#name=`) returns it to
  strict mode.

### Fixed

* Replacing an XML node with a `DocumentFragment` (`node.replace(fragment)`) is
  now all-or-nothing: if the replacement is rejected (e.g. it would leave the
  document with two root elements), the target node and the fragment are left
  untouched, instead of dropping the target and splicing in only part of the
  fragment. Replacing the root element with a single-element fragment now works.

## [0.6.0] - 2026-07-05

### Changed

* Text and comment node content (`create_text_node`, `create_comment`,
  `content=`) and attribute values (`[]=`, `set_attribute_ns`) now accept an
  embedded NUL (U+0000) and store it verbatim, matching the WHATWG DOM and
  browsers, instead of raising. Names, tag names, namespaces, CSS selectors, and
  XPath expressions stay NUL-strict, and invalid UTF-8 is still rejected. XML
  documents (`Makiri::XML`) are unchanged and still reject NUL everywhere, since
  XML 1.0 has no legal U+0000 character.

## [0.5.1] - 2026-06-22

### Changed

* Faster CSS queries that reuse the same selector: compiled selectors are now
  cached and reused across queries instead of being re-parsed each time.

## [0.5.0] - 2026-06-14

### Fixed

* Use-after-free when an XPath custom-function handler mutated the same
  `XPathContext` (`register_*` / `node=`) mid-`evaluate`: such re-entrant context
  mutation is now refused instead of invalidating the running evaluation's state.

* `Node#name=` now invalidates the element-name index, so a later `//tag` query
  reflects the rename instead of seeing a stale bucket.

* XML processing-instruction targets now follow XML 1.0 §2.6: a PITarget is a
  `Name`, not an NCName, so a colon is permitted (`<?a:b ...?>` parses, and
  `create_processing_instruction("a:b", ...)` succeeds). Only the reserved `xml`
  (any case) is still rejected. Previously a colon in a PI target was rejected as
  not-well-formed, which was stricter than the spec (a PI target is not subject to
  namespace processing).

* Memory leaks of the internal XPath evaluation context on error / edge paths: a
  `Makiri::XML` `#css` / `#xpath` / `#at_xpath` whose selector or expression failed
  the text-input contract leaked the context (it is now verified BEFORE the context
  is allocated), and a context could leak if building the Ruby result raised (it is
  now freed before conversion).

### Added

* `ProcessingInstruction#target` on the XML node (the PI's target name).

* Cross-kind `Document#import_node(node, deep = false)`. `import_node` now
  translates a subtree across representations: `Makiri::XML::Document#import_node`
  (newly added) imports an HTML (Lexbor) node by translating it to the XML node
  representation, and `Makiri::HTML::Document#import_node` likewise translates an
  XML node to HTML. Same-representation imports keep working (HTML to HTML via
  Lexbor, XML to XML via the arena deep/shallow copy). The result is a detached
  copy owned by the target document; the source is untouched. Elements (with
  attributes), text, comment, and processing-instruction nodes translate both
  ways, and an HTML `<template>`'s contents (which HTML keeps in a separate
  fragment) are carried across rather than silently dropped; an XML CDATA section
  has no HTML counterpart, so translating one into an HTML document fails closed
  (`Makiri::Error`). Namespaces are preserved across the translation: HTML->XML
  synthesizes the xmlns declarations needed to reproduce each node's namespace
  (so e.g. an inline `<svg>` stays in the SVG namespace and HTML elements in the
  XHTML namespace), and XML->HTML maps the namespace URI back to a Lexbor
  namespace id, interning any URI (not only the ones Lexbor knows by default) so
  custom namespaces survive too. An HTML-namespaced `<template>`'s content is
  placed in its content fragment (HTMLTemplateElement.content), like a parsed
  template. The other node-argument mutators
  (`add_child`/`before`/`after`/`replace`/`fragment`) still reject a foreign-kind
  node; `import_node` is the one sanctioned crossing point.

* `set_attribute_ns(namespace, qualified_name, value)` and
  `remove_attribute_ns(namespace, local_name)` on `Makiri::XML` elements - the DOM
  setAttributeNS / removeAttributeNS, keyed on the (explicit namespace, local name)
  pair so two attributes with the same qualified name in different namespaces
  coexist (a null/"" namespace is the null namespace).

* `Makiri::Lexbor::CSS.parse_stylesheet(text)`, a thin binding over Lexbor's
  CSS stylesheet parser that returns the parsed rules as plain Ruby primitives
  (`{type: :style, selectors: [{text:, specificity: [a,b,c]}, ...],
  declarations: [{name:, value:, important:}, ...]}` and nested
  `{type: :media, condition:, rules: [...]}`, in source order). Selector
  specificity and value normalization come from Lexbor; `css-syntax-3` error
  recovery means a broken stylesheet yields its valid rules instead of raising.
  Hosts the new `Makiri::Lexbor::*` namespace (the unabstracted lexbor-native
  surface, distinct from the Nokogiri-compatible `Makiri::*`).

## [0.4.0] - 2026-06-12

### Added

* CSS selectors on `Makiri::XML`. `#css` / `#at_css` / `#matches?`, lowered
  to the native XPath engine (case-sensitive, namespace-aware). Covers the
  standard selector set including combinator arguments to `:is`/`:where`/`:not`/
  `:has`, untyped `:*-of-type`, and `:lexbor-contains`. Verified by a differential
  against `Nokogiri::XML` plus property-based tests.

* `Makiri::XML::Builder`, a Nokogiri-compatible DSL for building an XML
  document or subtree from scratch (block / `instance_eval` forms, namespaced
  elements via `xml["prefix"]`, the `tag.class.id!` attribute short-cuts, raw-XML
  `<<`, and `.with`). Verified by a differential against `Nokogiri::XML::Builder`.

### Changed

* The XML declaration emits `encoding="UTF-8"` only when the source declared
  one (or `#to_xml(encoding:)` is passed); built or declaration-less documents
  now serialize to a bare `<?xml version="1.0"?>`, like Nokogiri (the output is
  UTF-8 either way).

* Faster XML queries. A document-rooted `//name` / `css("name")` is served
  from a lazily-built element-name index instead of a full-tree walk (~11x
  Nokogiri on the benchmark feed); name tests resolve their prefix once per step,
  and `at_css` / `at_xpath` short-circuit on prefixed name tests.

* CSS class/ID selectors now match case-sensitively in no-quirks documents
  (case-insensitively only in quirks mode), like browsers and `Nokogiri::HTML5` -
  via an upstreamed Lexbor fix (see below).

* XPath number parsing now follows the XPath 1.0 `Number` grammar exactly and
  is locale-independent, matching libxml2/Nokogiri and browsers. C `strtod`'s
  superset forms are no longer accepted: `1e3` / `0x1A` lex as a Number followed
  by a name (a syntax error as a full expression, where they previously parsed
  as 1000 / 26), `number()` returns NaN for exponent/hex/`+`-signed strings, and
  only XPath whitespace (space/tab/CR/LF, not `\v`/`\f`) is trimmed around the
  coerced value. Valid literals (`5.`, `.5`, `1.5`) are unchanged.

### Security

* Updated the vendored Lexbor (v3.0.0 -> `3a2d595`), which includes two
  CSS-selector fixes we upstreamed - class/ID case-sensitivity follows quirks
  mode, and a prefix-less type selector no longer defaults to the universal
  namespace - plus a heap-overflow fix in its `:lexbor-contains()` parser
  (reached from `Node#css`) and other post-v3.0.0 bugfixes. (An untagged master
  commit, taken deliberately; see CLAUDE.md.)

* Hardened native memory safety. The XML arena is ASan-red-zoned to catch
  intra-arena overflows, the engines are fuzzed under ASan/UBSan, and buffer
  growth is bounded by a hard ceiling.

* Extended the lint-enforced bounded-reader (`mkr_span`) discipline to the
  remaining byte-scanning code: the source-location line table, the XPath
  string-function scanners (now explicitly length-bounded instead of relying on
  the NUL contract), and the number parse above. Fixed a borrowed-RSTRING
  pointer held across a potential GC point in the XML encoding sniffer, and a
  missing NUL-termination guarantee in the libFuzzer XPath harness.

## [0.3.0] - 2026-06-06

### Added

* Native XML 1.0 reader + in-place editor - `Makiri::XML::Document.parse(source)`
  / `Makiri::XML(source)`. No libxml2: a strict, fail-closed parser builds its own
  node arena (case- and namespace-preserving), queried by the native XPath engine.
  * Strict & secure: fail-closed decode (bad UTF-8 / NUL -> `XML::SyntaxError`),
    duplicate attributes rejected, XML 1.0 only; verified against the W3C XML
    Conformance Test Suite.
  * Encoding autodetected (BOM / `<?xml encoding?>`); a contradicting String
    encoding is a fatal error, not a silent mis-decode.
  * DoS-bounded by a single arena byte ceiling (default 256 MiB; raise per parse
    with `max_bytes:`).
  * `<!DOCTYPE>` recognized but not processed (`#internal_subset` ->
    `XML::DocumentType`); zero entity/DTD I/O, so XXE and billion-laughs are
    structurally impossible. Kept off the tree, as in libxml2.
  * Read API mirrors Nokogiri: `#xpath` / `#at_xpath` (`{prefix => uri}`),
    name/namespace readers, `#text`, `#[]`, traversal, and namespace introspection
    (`Makiri::XML::Namespace`); `XPathContext` works over XML nodes too.
  * Prolog/epilog comments & PIs kept on the document node; adjacent same-type
    character data coalesced - byte-identical to Nokogiri (property-based diff).
  * `#to_xml` / `#to_s` (`pretty:` / `indent:` / `encoding:`) and `#canonicalize`
    (Inclusive C14N 1.0, byte-identical to libxml2); buffers fail closed.
  * Unsupported surface raises `NotImplementedError`: `#css` / `#at_css` and HTML
    serialization.
  * Tree mutation - fully fail-closed, detach-never-destroy:
    * in-place: `#[]=` / `#delete`, `#content=`, `#name=`, `#remove` / `#unlink`;
    * factories: `Document#create_{element,text_node,comment,cdata,processing_instruction}`
      (+ Nokogiri-style `.new` constructors);
    * insertion: `#add_child` / `<<`, `#before` / `#after`, `#replace` - namespaces
      resolved at the insertion point; a cross-document insert deep-copies;
    * fragments: `XML::DocumentFragment.parse` / `XML::Document#fragment`;
    * from scratch: `XML::Document.new` + `#root=`.
* `XML::Element#element_children` and `Node#clone_node` for XML nodes (also enabling
  `Node#dup` / `#clone`); a clone keeps name case, namespace and the CDATA type.
* `Node` includes `Enumerable` over its child nodes (`each` / `map` / `select` / ...).
* `Node#<=>` + `Comparable` - sort by document position (`nil` across documents or
  for attributes).
* `NodeSet.new(document_or_node, list = [])` - foreign / cross-representation nodes
  are rejected.
* `NodeSet#[]` accepts a `Range` or `start, length` (like `Array#[]`).
* `Node` / `NodeSet` / `Document` `#dup` / `#clone` now return real independent
  copies (`#dup(0)` shallow; `#clone(freeze:)` honoured).
* A frozen node is genuinely immutable - every mutator raises `FrozenError`.

### Changed

* CSS queries reuse one shared Lexbor engine (GVL-safe) and `at_css` wraps the match
  directly: `at_css('#id')` ~5x faster than nokolexbor (was ~1.16x slower).
* HTML serialization pre-reserves its buffer - `to_html` now at parity with nokolexbor.
* Node-class names are the WHATWG DOM interface names (`CDATASection`, `Attr`,
  `DocumentType`, ...), with the Nokogiri spellings (`CDATA`, `DTD`) kept as aliases;
  added `Node#cdata?`.
* Text-index range table uses `uint32` bounds (24 -> 16 B/entry; ~27% less retained
  index, byte-identical text).
* Parsing honours the input String's encoding - Shift_JIS / EUC-JP / ... are now
  transcoded to UTF-8 instead of mangled.
* Parsing skips its UTF-8 validation scan when the String's coderange already proves
  it valid.
* Faster HTML parse/serialize: `memchr` line table + validate-only UTF-8 scan (~7%),
  and a single-copy serializer buffer (~1.2-1.3x).

### Fixed

* Hardened the HTML/XML representation boundary. HTML (Lexbor) and XML (arena)
  nodes are now distinct TypedData types, so the wrong representation raises
  `TypeError` instead of corrupting memory:
  * `Node#==` / `XPathContext#node=` with an XML `Document` no longer aborts the
    process;
  * `NodeSet#|` / `+` / `&` / `-` across different documents raise `Makiri::Error`
    (was a silent mis-wrap);
  * HTML-only APIs (`import_node`, `add_child` / `before` / `after` / `replace`,
    `fragment(context:)`) reject an XML node argument (was a segfault).
* The bundle exported the entire vendored Lexbor symbol table (~1700 `lxb_*`); now
  only `Init_makiri` is exported, so loading alongside another Lexbor gem (e.g.
  nokolexbor) no longer segfaults. (Precompiled gems: rebuild required.)

## [0.2.0] - 2026-06-04

### Added

* `Element#tag_name` (DOM `tagName`) - the qualified name uppercased for an
  HTML element in an HTML document (`"DIV"`), keeping the original case for
  SVG/MathML; `nil` for non-elements. Complements `#name`, which stays the
  lowercase qualified name.
* `ProcessingInstruction#target` (DOM `target`) - a PI's target name; `nil` for
  other node kinds. Its data is read via `#content`/`#text`.
* `Document#create_processing_instruction(target, data)` (DOM
  `createProcessingInstruction`) and `Document#create_document_fragment` (DOM
  `createDocumentFragment`, an empty fragment to build up programmatically -
  unlike `#fragment` / `DocumentFragment.parse`, which parse HTML). Both produce
  a detached node owned by the document; PI creation fails closed when the data
  contains the `?>` terminator (matching the DOM constraint). (DOM
  `createCDATASection` is intentionally not provided: per WHATWG DOM it throws on
  an HTML document, which is the only kind Makiri produces.)
* `Node#{namespace_uri, prefix, local_name}` - the WHATWG DOM per-node
  namespace accessors on `Element` and `Attribute` (`nil` on other node kinds).
  `namespace_uri` resolves an element's namespace from its node (so an HTML
  element is the XHTML namespace `http://www.w3.org/1999/xhtml`, not `nil` - the
  DOM-faithful value browsers and `namespace-uri()` return; SVG/MathML get their
  own URI), and agrees byte-for-byte with the `namespace-uri()` XPath function.
  For attributes it is `nil` unless prefixed, where it returns the parser-assigned
  foreign-content namespace (`xlink`/`xml`/`xmlns`). `prefix` is the prefix
  segment of the qualified name (`nil` for the usual unprefixed HTML5 case), and
  `local_name` is the name without that prefix. Previously a node's namespace was
  reachable only through XPath (`namespace-uri()`/`local-name()`).
* `Node#clone_node(deep = false)` - a copy of the node, owned by the same
  document and detached from any parent (the DOM `cloneNode`, whose `deep`
  defaults to `false` - a missing/`nil`/`false` argument is a shallow clone; a
  truthy one copies the subtree). Built on the same `import_node` +
  `<template>`-content fixup the fragment parser uses, so a deep-cloned
  `<template>` keeps its contents. Fails closed: a failed import raises rather
  than returning a partial node.
* `Document#import_node(node, deep = false)` - a copy of `node` owned by the
  receiver document (the DOM `importNode`, whose `deep` likewise defaults to
  `false`). Unlike `Node#clone_node`, the copy is owned by the target rather
  than the node's own document, so it is the way to bring a node across
  documents (Makiri never moves a node between arenas); the source is left
  untouched. Same import + `<template>`-content fixup as `clone_node`, and fails
  closed on a failed import.
* `Node#pointer_id` - the underlying `lxb_dom_node_t` pointer as an Integer,
  matching `Nokogiri::XML::Node#pointer_id`. Shares the value `#hash`/`#eql?`
  are built on, so it is a stable, Nokogiri-compatible identity key for
  consumers (e.g. wrapper caches) that key nodes by pointer. Stable for a
  node's lifetime; an address may be reused after a node is freed (same caveat
  as Nokogiri).

### Changed

* Source gem: drop the Lexbor trees the build never compiles
  (`test`/`utils`/`examples`/`benchmarks`/`wasm`/`packaging`; each is behind an
  `IF(LEXBOR_BUILD_*)` guard and we build with them OFF), roughly halving the
  packaged file count (~1115 → ~566). Precompiled gems are unaffected.

### Internal

* XPath: build the per-context compiled-AST cache key with `mkr_strndup`
  (the expression is a `verified_text`, so its length is known) instead of
  `mkr_strdup`, avoiding a `strlen` over already-length-bounded bytes.

## [0.1.0] - 2026-06-02

First public release. An HTML5 parser, a native XPath 1.0 query engine, and CSS
selectors for Ruby - built on vendored [Lexbor](https://lexbor.com/) with no
libxml2 / libxslt dependency at any layer.

### Added

Parsing & DOM

* `Makiri::HTML` / `Makiri.parse` - HTML5 parsing via vendored, unpatched Lexbor,
  with browser-compatible UTF-8 decoding (invalid bytes → U+FFFD; parsing never
  fails on bad bytes). Read-only navigation and attribute/text readers across
  `Document`, `Element`, `Attribute`, `Text`, `CData`, `Comment`,
  `ProcessingInstruction`, `DocumentType`, and `DocumentFragment`.
* `Node#line` - 1-based source line of an element, reconstructed from the
  tokenizer without patching Lexbor (nil when the location is unknown).
* `Element#attribute_nodes` and `Attribute#{name,value,parent,element}`, backed
  by a lazily-built attribute→owner index in the Lexbor compat layer.
* `Document#{root,title,body,head,encoding,meta_encoding,meta_encoding=,
  quirks_mode,internal_subset,errors}` and `Makiri::DocumentType#{public_id,
  system_id,external_id}`.

XPath

* Native XPath 1.0 query engine (no libxml2/libxslt): `Node#{xpath,at_xpath}`
  and `Makiri::XPathContext` (`evaluate`, namespace/variable binding, custom
  function handlers that dispatch unknown functions to a Ruby object). 26
  built-in functions with spec-faithful semantics (XML NCNames including
  non-ASCII, node-set vs node-set comparisons per §3.4, document order per §5.1,
  Unicode-aware `translate`/`substring`).
* Namespace matching is strict by default (HTML5/WHATWG-faithful, like
  browsers' `document.evaluate` and `Nokogiri::HTML5`); pass
  `namespace_matching: :lax` for the namespace-agnostic, `Nokogiri::HTML`-style
  match.
* Per-context compiled-AST cache, and fail-closed per-evaluate budgets
  (operation / recursion-depth / node-set / string-byte caps) that raise
  `Makiri::XPath::LimitExceeded` on overrun.

CSS

* `Node#{css,at_css,matches?}` via Lexbor's selector engine (descendant-only,
  document order). Malformed selectors raise `Makiri::CSS::SyntaxError`.

Mutation & serialization

* DOM mutation: `add_child`/`<<`, `add_previous_sibling`/`before`,
  `add_next_sibling`/`after`, `remove`/`unlink`, `replace`; attribute `[]=` and
  `delete`; `content=`, `name=` (in-place rename); `Document#create_element` /
  `create_text_node` / `create_comment`; `inner_html=` / `outer_html=`. Inserts
  validate same-document, reject cycles, and use move semantics.
* Context-sensitive fragment parsing: `DocumentFragment.parse` /
  `Document#fragment` / `Node#parse` with a `context:` element, and a
  `<template>`'s contents via `Element#content_fragment` (preserved through
  import). Passes the html5lib-tests fragment suite.
* Serialization: `Node#{to_html,to_s,outer_html,inner_html}` (with `pretty:`)
  and `NodeSet#{to_html,text}`.
* Nokogiri-compatible conveniences: `Node#{attr,get_attribute,set_attribute,
  attribute,has_attribute?,node_name,type,classes,add_class,append_class,
  remove_class,traverse,root,ancestors,path,search,at,to_h}`,
  `NodeSet#{|,+,&,-,css,xpath,search,at,last,remove}`, and `Element.new` /
  `Text.new`.

Safety & concurrency

* UTF-8 text-input contract: HTML and fragment parsing are lenient (invalid
  bytes → U+FFFD, never reject), while strings passed to the XPath / CSS /
  DOM-mutation APIs must be valid UTF-8 with no NUL byte, otherwise they raise
  `Makiri::Error` - never silently truncated, repaired, or reinterpreted.
* Thread-safe by construction: parsing releases the GVL (concurrent parse scales
  ~2× on 8 cores), while XPath evaluation holds the GVL so sharing a document or
  context across threads cannot corrupt memory. Fail-closed string caps and
  iterative (non-recursive) tree walks resist stack-exhaustion DoS.

Performance (`rake bench`, vs Nokogiri/libxml2)

* Meets or beats Nokogiri on every benchmarked operation: parse ~3×, css ~12×,
  at_css ~1000×, serialize ~4×, `//tag` ~3.4×, `[@attr='v']` predicate ~1.5×,
  attribute axis ~1.3×, traverse ~1.2×, full-text extraction ~parity. Backed by
  a document element index (for `//tag`), a direct-attribute predicate fast
  path, and a hashed per-evaluate string-value cache.

Tooling

* Vendored Lexbor as a git submodule (pinned v3.0.0, applied without patches).
  Build hardening flags; AddressSanitizer+UBSan build (`rake sanitize`);
  grammar-aware robustness fuzzer (`rake fuzz` / `rake fuzz:sanitize`);
  benchmark harness (`rake bench`); conformance harnesses (html5lib-tests, WPT
  domxpath, CSS differential vs `Nokogiri::HTML5`). GitHub Actions CI across
  Ruby 3.2–4.0 × Ubuntu/macOS plus a sanitizer job.

[0.9.0]: https://github.com/takahashim/makiri/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/takahashim/makiri/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/takahashim/makiri/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/takahashim/makiri/compare/v0.5.1...v0.6.0
[0.5.1]: https://github.com/takahashim/makiri/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/takahashim/makiri/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/takahashim/makiri/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/takahashim/makiri/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/takahashim/makiri/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/takahashim/makiri/releases/tag/v0.1.0
