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
    in-place tree edits (attributes, content, remove).
  * Conformance is held by the W3C XML Conformance Test Suite, an XPath
    differential, and property-based testing vs Nokogiri (see below).
* Bounded, fail-closed execution
  * XPath evaluation is bounded by per-evaluation limits on work, memory, and recursion.
  * HTML parsing bounds the tree depth (`max_tree_depth:`, default 400), since tree
    construction is quadratic in it and a parse cannot be interrupted.
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

# Nesting deeper than 400 elements raises Makiri::Error (Nokogiri's default);
# max_tree_depth: raises the limit, and a negative value disables it
Makiri::HTML(deep_html, max_tree_depth: 2000)

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
# A node below the root serializes self-contained: no XML declaration, but the
# namespace declarations its subtree needs, so the output re-parses the same.
doc.at_xpath("//a:entry", ns).to_xml       # => "<entry xmlns=\"http://www.w3.org/2005/Atom\"><title>Hello</title></entry>"
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
e.delete("id")            # remove an attribute
doc.at_xpath("//draft").remove

doc.root.to_xml           # => "<feed xmlns:dc=\"urn:dc\"><entry dc:k=\"v\">Bye</entry></feed>"
```

XML subtrees can be built using `Document#create_element` and other node factory methods,
then inserted into a document with `#add_child`, `#before`, `#after`, or `#replace`.

When a node is created, it inherits its namespace from the context where it is
first inserted; if it already has a namespace, that namespace is preserved.

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

Makiri targets a Nokogiri-compatible API, but a few behaviours differ - XPath
name matching, XML namespaces, CSS extensions, serialization and text input.
See [NOKOGIRI_DIFFERENCES.md](NOKOGIRI_DIFFERENCES.md).

## Conformance

The XPath engine and XML parser are original code, so their correctness is held by
differential and standards harnesses in `spec/conformance/`.
The HTML XPath and CSS suites are differentials against `Nokogiri::HTML5`
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
* A Rust toolchain (stable) with `cargo` - the extension is a Rust crate.
* libclang (clang's development library): rb-sys runs bindgen at build time and
  reads Ruby's headers through it.
* CMake and a C toolchain, to build the vendored Lexbor static library.

## Build (development)

```sh
git submodule update --init --recursive
bundle install
bundle exec rake compile
bundle exec rake spec
```

## License

Apache License 2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
