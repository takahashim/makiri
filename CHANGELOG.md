# Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
* Project scaffolding (gemspec, Rakefile, README, LICENSE).
* Vendored Lexbor as a git submodule under `vendor/lexbor/`,
  pinned and applied without patches.
* C extension skeleton under `ext/makiri/`.
* HTML5 parsing and read-only DOM traversal: `Makiri::HTML`/`Makiri.parse`,
  `Document#{root,title,errors}`, and `Node` navigation/attribute readers.
* `Element#attribute_nodes` and `Attribute#{name,value,parent,element}`, backed
  by a lazily-built attribute→owner index in the Lexbor compat layer.
* Native XPath 1.0 query engine (no libxml2/libxslt): `Node#xpath`,
  `Node#at_xpath`, and `Makiri::XPathContext` with namespace and variable
  binding. Enforces per-evaluation budgets that fail closed on overrun.
* `Node#line` — 1-based source line of an element, reconstructed from the
  tokenizer without patching Lexbor. Returns nil when the location is unknown.
* CSS selector queries via Lexbor's selector engine: `Node#css` and
  `Node#at_css` (descendant-only, document order). Malformed selectors raise
  `Makiri::CSS::SyntaxError`.
* HTML serialization: `Node#to_html`/`#to_s`/`#outer_html` (outer),
  `Node#inner_html` (children), and `NodeSet#to_html`/`#text`.
* GitHub Actions CI: compile + full spec suite on Ruby 3.2/3.3/3.4 across
  Ubuntu and macOS.
* DOM mutation: `Node#add_child`/`<<`, `#add_previous_sibling`/`#before`,
  `#add_next_sibling`/`#after`, `#remove`/`#unlink`, `#replace`; attribute
  `#[]=` and `#delete`/`#remove_attribute`; `Document#create_element` and
  `#create_text_node`; and fragment assignment `#inner_html=` / `#outer_html=`.
  Inserts validate same-document, reject cycles, and use move semantics.
* XPath custom function handlers: `XPathContext#evaluate(expr, handler)` and
  `Node#xpath(expr, handler)` dispatch unknown functions to a Ruby object
  (handler exceptions surface as `Makiri::Error`).
* Pretty-print serialization: `Node#to_html(pretty: true)` /
  `#inner_html(pretty: true)`.
* Per-context compiled-AST cache: an `XPathContext` parses each expression once
  and re-evaluates the cached AST.
* Convenience API: `Node#root`, `Node#ancestors`, `Node#path` (round-trips
  through `#at_xpath`); `NodeSet#|`, `#+`, `#&`, `#-`, `#last`, `#at`.
* AddressSanitizer + UndefinedBehaviorSanitizer build: `MAKIRI_SANITIZE` extconf
  flag and a `rake sanitize` task that runs the suite under the runtime; a CI
  job exercises it on Linux.
* Robustness fuzzer (`spec/fuzz/`, `rake fuzz` / `rake fuzz:sanitize`):
  grammar-aware XPath/CSS fuzzing that asserts Makiri only ever raises
  `Makiri::Error` (never a foreign exception, crash, or hang).
* Benchmark harness (`bench/`, `rake bench`) comparing against Nokogiri.

### Performance

* The per-evaluation XPath string-value cache is now hashed (pointer-keyed
  open-addressing index over an ordered store) instead of linear-scanned. This
  removes the O(n²) behaviour of per-node predicate comparisons like
  `//li[@x="v"]` over a large node-set — measured from ~1.6× slower than
  Nokogiri to parity. Verified clean under ASan (suite + fuzzer + nested-eval).
* Text extraction (`Node#text`/`#content` over elements) streams descendant
  text straight into the result string, avoiding Lexbor's intermediate buffer
  and extra copy (~1.74× → ~1.59× of Nokogiri; the remaining gap is the
  inherent descendant-tree walk).

### Fixed

* `Document#text`/`#content` now returns the root element's text (it was empty
  because a Document's DOM textContent is null).
* Nokogiri-compatibility helpers: `Node#attributes`, `Node#search`/`#at`
  (CSS/XPath auto-detected), `Node#content=`, `Document#body`/`#head`,
  `NodeSet#css`/`#xpath`/`#search`/`#remove`, and `Element.new`/`Text.new`.
* `Node#name=` (in-place element rename), `Node#to_h` (name => value hash),
  `Document#encoding`/`#meta_encoding`/`#meta_encoding=`, the
  `ProcessingInstruction` node class, and `DocumentFragment` (via
  `DocumentFragment.parse` or `Document#fragment`; inserting a fragment splices
  its children).
