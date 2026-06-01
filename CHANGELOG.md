# Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
* XPath name-test namespace matching is now **strict by default**
  (HTML5/WHATWG-faithful, matching browsers' `document.evaluate` and
  `Nokogiri::HTML5`): an unprefixed element name test resolves in the HTML
  namespace, so `//div` matches but `//svg`/`//path` do not — foreign
  (SVG/MathML) elements need a registered prefix (`//svg:path`). Pass
  `namespace_matching: :lax` to `Node#{xpath,at_xpath}` or `XPathContext.new`
  for the previous namespace-agnostic match where `//path` finds the SVG
  element. The mode affects only unprefixed element name tests; prefixed tests,
  the `*` wildcard, and attribute tests are unchanged. (HTML elements stay in
  the XHTML namespace, so `namespace-uri()` stays DOM-correct.)
* Context-sensitive fragment parsing (Nokogiri-compatible): `context:` on
  `DocumentFragment.parse` / `Document#fragment` (a tag-name String, `"svg"` /
  `"math"` for the foreign roots, or a `Node` for any element incl. foreign
  non-roots), and `Node#parse(html)` (parse in the receiver element's context).
  Defaults to a `<body>` context. Backed by Lexbor's by-tag-id fragment parser,
  so context-specific tokenizer states and foreign-content adjustment are
  handled. Makiri now passes the html5lib-tests fragment suite.
* `Element#content_fragment` — a `<template>`'s "template contents" as a
  `Makiri::DocumentFragment` (WHATWG DOM `HTMLTemplateElement.content`; nil for
  non-templates). The contents are not children of the `<template>` itself
  (matching browsers), so query the returned fragment
  (`tpl.content_fragment.css("p")`). CSS/XPath over the template element do not
  descend into the content.
* `Makiri::DocumentType` node class exposing a doctype's `public_id` /
  `system_id` (with `external_id` as a Nokogiri-compatible alias for the public
  id), plus `Document#internal_subset` to fetch it. Reads the identifiers Lexbor
  already parses; an empty or missing id reports `nil` (matching Nokogiri).
  XPath still cannot select the doctype (XPath 1.0 has no doctype node type).
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
* The common attribute predicates `[@name]` and `[@name='literal']` take a
  direct-attribute fast path in the XPath evaluator instead of building a
  throwaway node-set per candidate. `//li[@attr='v']` went from ~parity to
  ~1.5× faster than Nokogiri. With this, Makiri meets or beats Nokogiri on
  every benchmarked operation. Verified clean under ASan + the fuzzer.
* Parsing releases the GVL (`Makiri::HTML`/`Document.parse`): the source is
  copied into a C buffer up front, then `mkr_parse_html` runs under
  `rb_thread_call_without_gvl`, so threads parse concurrently. Single-thread
  throughput is unchanged; aggregate parse throughput on an 8-core machine
  roughly doubles (≈2× in `rake bench`'s threaded row). Verified clean under
  ASan + the fuzzer, and a `GC.stress` multi-thread spec.
* Handler-free XPath evaluation releases the GVL too (`Node#xpath`,
  `XPathContext#evaluate`): the expression is parsed under the GVL, then the
  compiled AST is evaluated under `rb_thread_call_without_gvl` (the engine only
  re-enters Ruby through the custom-function resolver, which is absent without a
  handler). Aggregate XPath throughput on 8 cores went from ~1.6× to ~3.3× the
  single-thread rate; single-thread latency is unchanged. Verified clean under
  ASan + the fuzzer and the `GC.stress` multi-thread spec.
* `//tag` (a document-rooted descendant name-test) is answered from a new
  document element index — `tag id → elements`, grouped in document order and
  co-built with the attribute→owner index in the same walk — instead of a full
  tree walk. `//li` went from ~parity to ~3.4× faster than Nokogiri. Only
  Lexbor's static tag-id range is indexed; custom-element tags (whose ids are
  pointer values) and foreign (non-HTML) content fall back to the walk, and
  each candidate is re-checked so results are byte-identical to it. The index
  is invalidated by the same mutation hooks as the attribute index. Verified
  clean under ASan + the fuzzer.

### Fixed

* XPath expression whitespace is now exactly XML S (`#x20 #x9 #xD #xA`). The
  lexer used C `isspace()`, which also skipped `\v`/`\f`; those are not XPath
  whitespace and now surface as a syntax error.
* XPath name tests now accept non-ASCII NCNames. The lexer was ASCII-only, so a
  valid name test like `//dØdd` raised a SyntaxError; it now classifies name
  characters by the XML 1.0 (5th ed.) NCName ranges via a UTF-8 decoder that
  fails closed on malformed input.
* XPath `[@name]` / `[@name='v']` attribute predicates now match the attribute
  name case-SENSITIVELY, consistent with the attribute-axis name test (and with
  XPath 1.0 and Nokogiri::HTML5). The fast path had delegated to Lexbor's
  case-insensitive HTML attribute lookup, so `//div[@Id]` wrongly matched `id`.
* Fragment parsing now preserves `<template>` contents. Deep node import
  (`DocumentFragment.parse` / `Document#fragment`) copied the normal child chain
  but not a template's separate content fragment, so an imported `<template>`
  lost its contents; import now fixes up template content recursively.
* XPath document order now places an element before its own attribute nodes
  (XPath 1.0 §5.1). The small-node-set fallback comparator sorted an attribute
  ahead of its owner element, so a union like `//p | //p/@id` returned the
  attribute first and disagreed with the indexed comparator used for larger
  node-sets. Found by the new XPath differential against libxml2.
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
