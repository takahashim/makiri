# Makiri

Standards-oriented HTML5 parsing, CSS selector querying, and XPath 1.0
querying for Ruby, powered by Lexbor and a native XPath engine.

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

# XPath 1.0 (native engine — no libxml2)
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

### XML (read-only, secure)

`Makiri::XML(source)` parses XML with a native, strict, well-formedness-checking
parser (no libxml2) and queries it through the same native XPath 1.0 engine.
Element-name case and namespaces are preserved. It is **read-only** and
**fail-closed**: malformed input, a DOCTYPE/DTD, or a duplicate attribute raises
`Makiri::XML::SyntaxError`, and operations XML does not support raise
`NotImplementedError` rather than returning a wrong result.

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
doc.root.to_xml      # raises NotImplementedError (read-only)
```

CSS is intentionally unavailable for XML (Lexbor's selector engine lower-cases
names, which breaks XML case/namespace matching) - use XPath. XML mutation and
serialization are a later phase.

## Non-goals (v1.0)

* XML writing (the XML reader above is read-only): no XML mutation, no XML/XHTML
  serialization (`to_xml`), no `Makiri::XML::Document` construction.
* XSLT, DTD / Schema / RelaxNG validation, XPointer, XInclude.
* Streaming / SAX parsing.
* Drop-in replacement for every Nokogiri method. Makiri covers the common
  HTML-scraping and manipulation surface. Deliberately not provided:
  - XML/XHTML serialization variants (`to_xml`, `to_xhtml`, `write_xml_to`)
  - XML/DTD construction (`create_internal_subset`, `external_subset`)
  - namespace introspection beyond `namespace-uri()` (`namespace_definitions`, `add_namespace`, `collect_namespaces`)
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

### CSS

* jQuery/Nokogiri CSS extensions are not supported (`:contains`, `:gt`, `:lt`, `:eq`, `:first`, …)
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
