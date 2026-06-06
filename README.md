# Makiri

Standards-oriented HTML5/XML parsing, CSS selector querying, XPath 1.0 querying,
and a native XML 1.0 reader/editor for Ruby, powered by Lexbor and a native XPath
engine - with no libxml2 dependency.

> [!WARNING]
> Status: early release. APIs and behavior may change before v1.0.

## What / Why

Makiri uses Lexbor for HTML5 parsing and CSS selector support, and implements
XPath 1.0 evaluation in its own native engine, with no libxml2 dependency.

* HTML5 parsing via [Lexbor](https://lexbor.com)
  * Makiri uses Lexbor as the parsing backend and provides a Ruby-facing DOM/query layer.
  * Lexbor-specific behavior is isolated in a thin compatibility layer
    (`ext/makiri/lexbor_compat/`).
* CSS selector support via Lexbor
  * Supports Lexbor-backed standard CSS selector querying, including `:is`/`:where`/`:has`
* Native XPath 1.0 engine
  * XPath is parsed and evaluated by Makiri's own engine, written from scratch.
  * Makiri does not depend on libxml2 for parsing, DOM representation, or XPath evaluation.
* Native XML 1.0 reader + in-place editor (`Makiri::XML`)
  * A strict, non-validating, fail-closed parser with its own node arena (not
    Lexbor's HTML DOM), queried through the same native XPath engine, with
    in-place tree edits (attributes, content, rename, remove).
  * Conformance is held by the W3C XML Conformance Test Suite, an XPath
    differential, and property-based testing vs Nokogiri (see below).
* Bounded, fail-closed execution
  * XPath evaluation is bounded by per-evaluation limits on work, memory, and recursion.
  * Ownership and borrowing are kept explicit across layers, with owned/borrowed
    string types and verified text at engine boundaries.
  * Programmatic invalid input, limit violations, allocation failures, and unsupported constructs
    fail closed instead of producing partial or silently truncated results.

## Usage

```ruby
require "makiri"

doc = Makiri::HTML(<<~HTML)
  <html><body>
    <div id="main" class="container">
      <p class="lead">Hello</p>
      <a href="/a">one</a>
      <a href="/b">two</a>
    </div>
  </body></html>
HTML

# CSS selectors (Lexbor's selector engine)
doc.css("a").map { |a| a["href"] }      # => ["/a", "/b"]
doc.at_css("p.lead").text               # => "Hello"

# XPath 1.0 (native engine - no libxml2)
doc.xpath("//a").length                 # => 2
doc.xpath("count(//a)")                 # => 2.0
doc.at_xpath('//*[@id="main"]/p').text  # => "Hello"

# Attributes and navigation
link = doc.at_css("a")
link["href"]                            # => "/a"
link.parent.name                        # => "div"

# Source location (reconstructed from the tokenizer, no Lexbor patches)
doc.at_css("p").line                    # => 3

# Serialization
doc.at_css("#main").to_html             # => "<div id=\"main\" ...>...</div>"
doc.at_css("#main").inner_html          # => "\n    <p class=\"lead\">Hello</p>\n..."
```

### XPathContext (namespaces and variables)

```ruby
ctx = Makiri::XPathContext.new(doc)
ctx.register_variable("cls", "lead")
ctx.evaluate('//p[@class=$cls]').first.text   # => "Hello"
```

### XML (with in-place editing)

`Makiri::XML(source)` parses **XML 1.0** with a native, strict,
well-formedness-checking parser (no libxml2) and queries it through the same
native XPath 1.0 engine. `source` is a String or any object responding to
`#read` (an `IO` / `File` / `StringIO`); read a non-UTF-8 file in binary mode
(`File.binread`) so its encoding is autodetected. Element-name case and namespaces are preserved. It is
**fail-closed**: malformed input, a duplicate attribute, or a
non-`1.0` version declaration raises `Makiri::XML::SyntaxError`, and operations
XML does not support raise `NotImplementedError` rather than returning a wrong
result. The tree supports in-place edits and building new subtrees (see below).
A `<!DOCTYPE ...>` is recognized but its **DTD is not processed** (no
entity/element declarations are loaded, no external subset is fetched) - so a
DTD-defined entity reference stays an undefined-entity error and **XXE /
billion-laughs are structurally impossible**. The doctype's name and identifiers
are still readable:

```ruby
doc = Makiri::XML(<<~XML)
  <feed xmlns="http://www.w3.org/2005/Atom">
    <entry><title>Hello</title></entry>
    <entry><title>World</title></entry>
  </feed>
XML

# Namespace matching is strict, so a default namespace needs a registered prefix.
ns = { "a" => "http://www.w3.org/2005/Atom" }
doc.xpath("//entry").length                    # => 0  (default namespace)
doc.xpath("//a:entry", ns).length              # => 2
doc.at_xpath("//a:entry/a:title", ns).text     # => "Hello"

# Or reuse a context (caches registrations + compiled expressions):
ctx = Makiri::XPathContext.new(doc.root)
ctx.register_namespace("a", "http://www.w3.org/2005/Atom")
ctx.evaluate("//a:entry").length               # => 2

el = doc.at_xpath("//a:entry", ns)
el.local_name                                  # => "entry"
el.namespace_uri                               # => "http://www.w3.org/2005/Atom"

doc.css("entry")     # raises NotImplementedError (use #xpath)

# Serialize back to XML
doc.to_xml                                 # => "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<feed ...>...</feed>\n"
doc.at_xpath("//a:entry", ns).to_xml       # => "<entry><title>Hello</title></entry>" (no declaration)
doc.to_xml(pretty: true)                   # indented, element-only content

# DOCTYPE is recognized but the DTD is not processed (no entities, no I/O):
dtd = Makiri::XML(%(<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.0//EN" "x.dtd"><html/>))
        .internal_subset
dtd.name         # => "html"
dtd.external_id  # => "-//W3C//DTD XHTML 1.0//EN"  (alias: #public_id)
dtd.system_id    # => "x.dtd"
```

Comments and processing instructions in the prolog/epilog are document-node
children (reachable via `//comment()` / `//processing-instruction()` and
`#children`), and adjacent CDATA is coalesced - matching libxml2 and the XPath
data model. `#to_xml` / `#to_s` serialize the tree back to XML (`pretty: true`,
or `indent: n`, for indented element-only content; `encoding: "Shift_JIS"` to
transcode, with a hex character reference for anything the encoding can't hold);
a `Document#to_xml` adds the declaration and the DOCTYPE. `#canonicalize` emits
Inclusive Canonical XML 1.0 (for XML signatures; `comments: true` to keep
comments), byte-identical to libxml2. CSS is intentionally unavailable for XML
(Lexbor's selector engine lower-cases names, which breaks XML case/namespace
matching) - use XPath.

The tree supports in-place mutation - every edit validates its input (names as
XML 1.0 QNames, values as XML Char) so the tree stays serializable to
well-formed XML, and a removed node is detached, never freed, so a live wrapper
that aliases it stays usable:

```ruby
doc = Makiri::XML(%(<feed xmlns:dc="urn:dc"><entry id="1">Hi</entry><draft/></feed>))
e   = doc.at_xpath("//entry")

e["id"]   = "9"            # add or replace an attribute (value escaped on output)
e["dc:k"] = "v"           # a prefixed name resolves against the in-scope xmlns
e.content = "Bye"         # replace an element's children with text
e.name    = "post"        # rename in place (identity + namespace re-resolved)
e.delete("id")            # remove an attribute
doc.at_xpath("//draft").remove

doc.root.to_xml           # => "<feed xmlns:dc=\"urn:dc\"><post dc:k=\"v\">Bye</post></feed>"
```

New subtrees can be built too - `Document#create_element` (and
`#create_text_node` / `#create_comment` / `#create_cdata` /
`#create_processing_instruction`) make detached nodes, and `#add_child` / `<<`,
`#add_previous_sibling` / `#before`, `#add_next_sibling` / `#after`, `#replace`
link them. A node's namespace is resolved against its position **at insertion**
(a prefixed name binds to the in-scope `xmlns`, an unprefixed element to the
default namespace), so the same tree results whether you set names before or
after attaching; an unbound prefix in the live tree fails closed. A node from
another document is **deep-copied** into the target (the source is untouched):

```ruby
doc   = Makiri::XML(%(<feed xmlns="urn:a" xmlns:dc="urn:dc"/>))
entry = doc.create_element("entry")
entry["dc:id"] = "42"                       # prefixed attr resolves on insertion
entry.add_child(doc.create_element("title", "Hello"))
doc.root.add_child(entry)

doc.to_xml   # => "...<entry dc:id=\"42\"><title>Hello</title></entry>..."
```

Supported edits: `#[]=`, `#delete` / `#remove_attribute`, `#content=`, `#name=`,
`#remove` / `#unlink`, the factories above, and `#add_child` / `<<` /
`#before` / `#after` / `#replace`. Insertion takes a `Makiri::XML` node or a
`DocumentFragment` (its children are spliced in); a fragment is parsed by
`Document#fragment(str)` (bound to the document) or `DocumentFragment.parse(str)`
(standalone). A raw string handed straight to `#add_child` is **not** accepted -
parse it into a fragment first. A whole document can also be built from scratch
with `XML::Document.new` + `#root=` and the factories.

The character encoding is autodetected (XML 1.0 Appendix F): a byte-order mark or
the `<?xml encoding="..."?>` declaration selects it, so raw bytes (`File.binread`)
in UTF-16, Shift_JIS, etc. parse correctly and a leading BOM is stripped. A
concrete String encoding stays authoritative - a BOM or declaration that
contradicts it is a fatal error, not a silent mis-decode.

Parsing is DoS-bounded by a single arena memory ceiling (default 256 MiB,
counting node structs and text), which fits every standard document. Raise it
per parse for an unusually large one:

```ruby
Makiri::XML(huge_xml, max_bytes: 512 * 1024 * 1024)   # also Makiri::XML::Document.parse(..., max_bytes:)
```

Conformance is held by a regression net: the **W3C XML Conformance Test Suite**
(`rake conformance:xmlconf`, 100% of the in-scope non-validating XML-1.0 tests),
an XPath 1.0 differential vs Nokogiri/libxml2 (`rake conformance:xpath_xml`), and
property-based testing that requires Makiri's tree to be byte-identical to
Nokogiri's over generated documents (`rake conformance:xml_pbt`).

## Non-goals (v1.0)

* Passing a raw markup string straight to an insertion method
  (`node.add_child("<x/>")`); parse it into a fragment first
  (`Document#fragment` / `DocumentFragment.parse`). (Building XML from scratch
  (`XML::Document.new` + `#root=`), the node factories - `Document#create_element`
  etc. - fragments, node insertion (`#add_child` / `#before` / `#after` /
  `#replace`), and `#to_xml` serialization ARE supported.)
* XSLT, DTD / Schema / RelaxNG validation, XPointer, XInclude.
* Streaming / SAX parsing.
* Drop-in replacement for every Nokogiri method. Makiri covers the common
  HTML-scraping and manipulation surface. Deliberately not provided:
  - XHTML serialization variants (`to_xhtml`, `write_xml_to`); `#to_xml` is supported
  - XML/DTD construction (`create_internal_subset`, `external_subset`)
  - namespace *mutation* (`add_namespace_definition`); read introspection
    (`#namespace`, `#namespace_definitions`, `#namespaces`, `#collect_namespaces`)
    is supported on `Makiri::XML` nodes
  - Nokogiri internals (`decorate`, `slop!`, `validate`).

## Differences from Nokogiri

Makiri targets a Nokogiri-compatible API, but a few query behaviours differ.
Detailed, test-backed notes live in `spec/conformance/README.md`.

### XPath

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

### XML

* `Makiri::XML` is **XML 1.0 only and non-validating**.
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
* Otherwise the parsed tree is byte-identical to `Nokogiri::XML`'s (verified by
  the property-based differential), including namespaces, prolog/epilog comments
  and PIs, and adjacent-CDATA coalescing.

### CSS

* jQuery/Nokogiri CSS extensions are not supported (`:contains`, `:gt`, `:lt`, `:eq`, `:first`, ...)
  * Makiri uses Lexbor's standards-only selector engine.
    Use XPath (`xpath("//p[contains(., 'x')]")`) or Enumerable (`css('li')[1]`).
    Standard Level-4 selectors (`:is` / `:where` / `:has`) are supported; some of which Nokogiri rejects.
* Type selectors are ASCII case-insensitive (CSS-correct for HTML; `LI` matches `<li>`)
  * `Nokogiri::HTML5` is case-sensitive there.
* Class/ID selectors are matched case-insensitively regardless of quirks mode (a Lexbor behaviour)
  * In a no-quirks document browsers and `Nokogiri::HTML5` match them case-sensitively.

## Requirements

* CRuby 3.2 or newer.
* CMake (to build vendored Lexbor at install time).
* C99 toolchain.

## Build (development)

```sh
git submodule update --init --recursive
bundle install
bundle exec rake compile
bundle exec rake spec
```

## License

Apache License 2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
