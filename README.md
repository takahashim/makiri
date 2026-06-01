# Makiri

HTML5 parsing and querying for Ruby, built on **vanilla Lexbor** and a
**native XPath 1.0 engine**. **No libxml2 dependency at any layer.**

> Status: pre-release. v0.1 in progress.

## Why

* HTML5 parsing via [Lexbor](https://lexbor.com) — no patches applied
  to the upstream library. Lexbor limitations are absorbed in a thin
  compat layer (`ext/makiri/lexbor_compat/`).
* XPath 1.0 evaluated by an engine written from scratch — no
  libxml2-derived code anywhere in the dependency tree. The motivation
  is to escape the long tail of libxml2 CVEs that bundled or
  libxml2-derived parsers carry.
* Security focus: per-evaluate XPath budgets, node-set size caps,
  input bounds, and fail-closed error handling (fuzz/differential
  validation against Nokogiri is planned).

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
doc.at_css("#main").inner_html          # => "<p class=\"lead\">Hello</p>..."
```

### XPathContext (namespaces and variables)

```ruby
ctx = Makiri::XPathContext.new(doc)
ctx.register_variable("cls", "lead")
ctx.evaluate('//p[@class=$cls]').first.text   # => "Hello"
```

## Non-goals (v1.0)

* XML parsing (HTML only).
* XSLT, DTD / Schema / RelaxNG validation, XPointer, XInclude.
* Streaming / SAX parsing.
* Drop-in replacement for every Nokogiri method. The supported subset
  is documented in `docs/API.md` (forthcoming).

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
