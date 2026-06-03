# Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1] - 2026-06-04

### Added

* `Node#clone_node(deep = false)` — a copy of the node, owned by the same
  document and detached from any parent (the DOM `cloneNode`, whose `deep`
  defaults to `false` — a missing/`nil`/`false` argument is a shallow clone; a
  truthy one copies the subtree). Built on the same `import_node` +
  `<template>`-content fixup the fragment parser uses, so a deep-cloned
  `<template>` keeps its contents. Fails closed: a failed import raises rather
  than returning a partial node.
* `Document#import_node(node, deep = false)` — a copy of `node` owned by the
  receiver document (the DOM `importNode`, whose `deep` likewise defaults to
  `false`). Unlike `Node#clone_node`, the copy is owned by the target rather
  than the node's own document, so it is the way to bring a node across
  documents (Makiri never moves a node between arenas); the source is left
  untouched. Same import + `<template>`-content fixup as `clone_node`, and fails
  closed on a failed import.
* `Node#pointer_id` — the underlying `lxb_dom_node_t` pointer as an Integer,
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
selectors for Ruby — built on vendored [Lexbor](https://lexbor.com/) with **no
libxml2 / libxslt dependency at any layer**.

### Added

**Parsing & DOM**

* `Makiri::HTML` / `Makiri.parse` — HTML5 parsing via vendored, unpatched Lexbor,
  with browser-compatible UTF-8 decoding (invalid bytes → U+FFFD; parsing never
  fails on bad bytes). Read-only navigation and attribute/text readers across
  `Document`, `Element`, `Attribute`, `Text`, `CData`, `Comment`,
  `ProcessingInstruction`, `DocumentType`, and `DocumentFragment`.
* `Node#line` — 1-based source line of an element, reconstructed from the
  tokenizer without patching Lexbor (nil when the location is unknown).
* `Element#attribute_nodes` and `Attribute#{name,value,parent,element}`, backed
  by a lazily-built attribute→owner index in the Lexbor compat layer.
* `Document#{root,title,body,head,encoding,meta_encoding,meta_encoding=,
  quirks_mode,internal_subset,errors}` and `Makiri::DocumentType#{public_id,
  system_id,external_id}`.

**XPath**

* Native XPath 1.0 query engine (no libxml2/libxslt): `Node#{xpath,at_xpath}`
  and `Makiri::XPathContext` (`evaluate`, namespace/variable binding, custom
  function handlers that dispatch unknown functions to a Ruby object). 26
  built-in functions with spec-faithful semantics (XML NCNames including
  non-ASCII, node-set vs node-set comparisons per §3.4, document order per §5.1,
  Unicode-aware `translate`/`substring`).
* Namespace matching is **strict by default** (HTML5/WHATWG-faithful, like
  browsers' `document.evaluate` and `Nokogiri::HTML5`); pass
  `namespace_matching: :lax` for the namespace-agnostic, `Nokogiri::HTML`-style
  match.
* Per-context compiled-AST cache, and fail-closed per-evaluate budgets
  (operation / recursion-depth / node-set / string-byte caps) that raise
  `Makiri::XPath::LimitExceeded` on overrun.

**CSS**

* `Node#{css,at_css,matches?}` via Lexbor's selector engine (descendant-only,
  document order). Malformed selectors raise `Makiri::CSS::SyntaxError`.

**Mutation & serialization**

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

**Safety & concurrency**

* UTF-8 text-input contract: HTML and fragment parsing are lenient (invalid
  bytes → U+FFFD, never reject), while strings passed to the XPath / CSS /
  DOM-mutation APIs must be valid UTF-8 with no NUL byte, otherwise they raise
  `Makiri::Error` — never silently truncated, repaired, or reinterpreted.
* Thread-safe by construction: parsing releases the GVL (concurrent parse scales
  ~2× on 8 cores), while XPath evaluation holds the GVL so sharing a document or
  context across threads cannot corrupt memory. Fail-closed string caps and
  iterative (non-recursive) tree walks resist stack-exhaustion DoS.

**Performance** (`rake bench`, vs Nokogiri/libxml2)

* Meets or beats Nokogiri on every benchmarked operation: parse ~3×, css ~12×,
  at_css ~1000×, serialize ~4×, `//tag` ~3.4×, `[@attr='v']` predicate ~1.5×,
  attribute axis ~1.3×, traverse ~1.2×, full-text extraction ~parity. Backed by
  a document element index (for `//tag`), a direct-attribute predicate fast
  path, and a hashed per-evaluate string-value cache.

**Tooling**

* Vendored Lexbor as a git submodule (pinned v3.0.0, applied without patches).
  Build hardening flags; AddressSanitizer+UBSan build (`rake sanitize`);
  grammar-aware robustness fuzzer (`rake fuzz` / `rake fuzz:sanitize`);
  benchmark harness (`rake bench`); conformance harnesses (html5lib-tests, WPT
  domxpath, CSS differential vs `Nokogiri::HTML5`). GitHub Actions CI across
  Ruby 3.2–4.0 × Ubuntu/macOS plus a sanitizer job.

[Unreleased]: https://github.com/takahashim/makiri/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/takahashim/makiri/releases/tag/v0.1.0
