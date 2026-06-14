# Makiri

Makiri is a Ruby library for parsing and querying HTML and XML documents.

It uses [Lexbor](https://lexbor.com/) for HTML parsing and CSS selector matching, and includes a built-in native XPath 1.0 engine and XML 1.0 parser.
Makiri does not depend on libxml2.

> [!WARNING]
> Status: early release. APIs and behavior may change before v1.0.

## What / Why

Makiri uses Lexbor for HTML5 parsing and CSS selector support, and implements
XPath 1.0 evaluation in its own native engine, with no libxml2 dependency.

* HTML5 parsing via [Lexbor](https://lexbor.com)
  * Makiri uses Lexbor as the parsing backend and provides a Ruby-facing DOM/query layer.
* CSS selector support via Lexbor
  * Supports Lexbor-backed standard CSS selector querying, including `:is`/`:where`/`:has`
* Native XPath 1.0 engine
  * XPath is parsed and evaluated by Makiri's own engine, written from scratch.
  * Makiri does not depend on libxml2 for parsing, DOM representation, or XPath evaluation.
* Native XML 1.0 parser
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

# CSS selectors work too (lowered to the native XPath engine): a bare type
# selector binds to the document's default namespace, so this just works.
doc.css("entry").length                        # => 2
doc.css("feed > entry").map { |e| e.at_css("title").text }  # => ["Hello", "World"]

# Serialize back to XML
doc.to_xml                                 # => "<?xml version=\"1.0\"?>\n<feed ...>...</feed>\n"
doc.at_xpath("//a:entry", ns).to_xml       # => "<entry><title>Hello</title></entry>" (no declaration)
doc.to_xml(pretty: true)                   # indented, element-only content

# DOCTYPE is recognized but the DTD is not processed (no entities, no I/O):
dtd = Makiri::XML(%(<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.0//EN" "x.dtd"><html/>))
        .internal_subset
dtd.name         # => "html"
dtd.external_id  # => "-//W3C//DTD XHTML 1.0//EN"  (alias: #public_id)
dtd.system_id    # => "x.dtd"
```

The tree supports in-place mutation.

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

XML subtrees can be built with `Document#create_element` and related node factory methods,
then inserted with `#add_child`, `#before`, `#after`, or `#replace`;
namespaces are resolved at insertion time, and cross-document nodes are deep-copied.

`Document#import_node(node, deep = false)` brings a node into a document as a
detached copy, and works **across representations**: importing a `Makiri::HTML`
node into a `Makiri::XML::Document` (or vice versa) translates the subtree between
the two node representations, preserving namespaces (e.g. an inline `<svg>` keeps
the SVG namespace, HTML elements the XHTML namespace; custom namespaces are
preserved across both directions). An XML CDATA section has no HTML counterpart,
so importing one into an HTML document raises.

```ruby
doc   = Makiri::XML(%(<feed xmlns="urn:a" xmlns:dc="urn:dc"/>))
entry = doc.create_element("entry")
entry["dc:id"] = "42"                       # prefixed attr resolves on insertion
entry.add_child(doc.create_element("title", "Hello"))
doc.root.add_child(entry)

doc.to_xml   # => "...<entry dc:id=\"42\"><title>Hello</title></entry>..."
```

`Makiri::XML::Builder` is the Nokogiri-compatible DSL over those factories.

```ruby
builder = Makiri::XML::Builder.new do |xml|
  xml.feed("xmlns" => "http://www.w3.org/2005/Atom", "xmlns:dc" => "urn:dc") do
    xml.title("Example Feed")
    xml.entry("dc:id" => "1") do
      xml.title("First")
      xml.summary { xml.cdata("raw <b>html</b>") }
    end
  end
end

builder.to_xml                 # the whole document (with XML declaration)
builder.doc                    # the Makiri::XML::Document being built
```

XML parsing is bounded by an arena memory limit, 256 MiB by default,
and unusually large documents can raise it with `max_bytes:`.

```ruby
Makiri::XML(huge_xml, max_bytes: 512 * 1024 * 1024)   # also Makiri::XML::Document.parse(..., max_bytes:)
```

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
* A colon in a processing-instruction target is well-formed (`<?a:b ...?>` parses).
  * XML 1.0 §2.6: a `PITarget` is a `Name`, not an NCName, and Namespaces in XML
    1.0's normative conformance section constrains only element/attribute names
    (QNames), never PI targets. Nokogiri/libxml2 rejects it (`colons are forbidden
    from PI names`); Makiri follows the normative text. Only the reserved `xml`
    (any case) target is rejected.
* Otherwise the parsed tree is byte-identical to `Nokogiri::XML`'s (verified by
  the property-based differential), including namespaces, prolog/epilog comments
  and PIs, and adjacent-CDATA coalescing.

### CSS

* Most jQuery/Nokogiri CSS extensions are not supported (`:gt`, `:lt`, `:eq`, `:first`, ...)
  * Makiri uses Lexbor's selector engine, which is standards-based apart from one
    text-containment extension. Use XPath (`xpath("//p[contains(., 'x')]")`) or
    Enumerable (`css('li')[1]`) for the rest.
    Standard Level-4 selectors (`:is` / `:where` / `:has`) are supported; some of which Nokogiri rejects.
  * `:lexbor-contains("text")` **is** supported (on both HTML and XML) - Lexbor's
    spelling of the jQuery `:contains()` substring filter, matching an element
    whose text contains the string; append ` i` (`:lexbor-contains("text" i)`)
    for an ASCII case-insensitive match. (Nokogiri's name `:contains` is not an
    alias.) Like Lexbor's matcher, it tests the element's **immediate child text
    nodes** (not the deep string-value), so HTML and XML agree; on XML it lowers
    to XPath `child::text()[contains(., "text")]`.
* Untyped `:*-of-type` (`:first-of-type`, `:nth-of-type(an+b)`, ... with no type
  selector) is supported and correct on both HTML and XML - the "type" is the
  element's own expanded name.
  * Nokogiri (XML and HTML5) mistranslates these to first-/only-child
    (`//*[position()=1]` / `//*[last()=1]`), so it under-matches; Makiri matches
    Lexbor's HTML matcher.
* Type selectors are ASCII case-insensitive (CSS-correct for HTML; `LI` matches `<li>`)
  * `Nokogiri::HTML5` is case-sensitive there.

## Conformance

The XPath engine and XML parser are original code, so their correctness is held by
differential and standards harnesses in `spec/conformance/`.
The HTML XPath and CSS suites are differentials against **`Nokogiri::HTML5`**
(Gumbo / WHATWG, never libxml2's non-conformant HTML4 parser): both sides parse
HTML5, so the DOM is isomorphic and results are compared node-for-node. HTML
parsing itself is checked against the WHATWG html5lib-tests corpus, and
XPath-over-HTML semantics additionally against browsers via a WPT port.
See also [`spec/conformance/README.md`](spec/conformance/README.md).

| Suite | Input | Oracle | `rake` task |
|---|---|---|---|
| HTML parsing | HTML | WHATWG html5lib-tests (expected-tree corpus) | `conformance:html5` |
| XPath 1.0 | HTML | `Nokogiri::HTML5` (libxml2 XPath) — differential | `conformance:xpath` |
| XPath over HTML | HTML | browsers (WPT `domxpath`, hand-ported; runs under `rake spec`) | — |
| CSS selectors | HTML | `Nokogiri::HTML5#css` — differential | `conformance:css` |
| Well-formedness | XML | W3C XML Conformance Test Suite | `conformance:xmlconf` |
| XPath 1.0 | XML | `Nokogiri::XML` — differential | `conformance:xpath_xml` |
| Parsed tree (property-based) | XML | `Nokogiri::XML` — differential | `conformance:xml_pbt` |
| CSS selectors | XML | `Nokogiri::XML` — differential | `conformance:css_xml` |

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

### Vendored Lexbor version

`vendor/lexbor` is pinned to `3a2d595` (`v3.0.0-25`), an untagged `master`
commit, for fixes that v3.0.0 lacks: two upstreamed CSS-selector fixes (class/ID
case-sensitivity in quirks mode, and prefix-less type-selector namespacing), a
heap-overflow fix in the `:lexbor-contains()` parser, and other post-v3.0.0
bugfixes. Lexbor stays vanilla; we return to a release tag once one ships after
v3.0.0. See `CLAUDE.md` for details.

## License

Apache License 2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
