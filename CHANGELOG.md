# Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

* The text index's per-element range table stores its slice-run bounds as
  `uint32` indices instead of `size_t`, shrinking each entry from 24 to 16 bytes
  (the node pointer plus two 32-bit indices) — about a third off the table, the
  index's largest allocation. On a 2k-element document the retained text index
  drops ~27% (~480 → ~352 KB) with byte-identical text and no change in
  extraction speed (still ~4× Nokogiri). A document with more than `UINT32_MAX`
  text slices (impossible in practice) fails the index build closed and the
  caller falls back to a walk.

* Parsing now **honours the input String's encoding**. UTF-8, US-ASCII and
  ASCII-8BIT (binary) are used directly (the UTF-8 common case is unchanged — a
  single encoding check, no extra work); any other encoding (Shift_JIS, EUC-JP,
  ISO-8859-1, Windows-1252, …) is transcoded to UTF-8 (invalid/undefined →
  U+FFFD) so its content is preserved. Previously every input was read as raw
  UTF-8 bytes, so a Shift_JIS or EUC-JP document was mangled into U+FFFD; it now
  decodes correctly (matching `Nokogiri`). Applies to `Makiri::HTML` /
  `Makiri.parse`, `DocumentFragment.parse`, and `Document#fragment` / `Node#parse`.
* Parsing skips its UTF-8 validation scan when the input String's cached
  coderange already proves it valid (pure ASCII, or valid in the UTF-8
  encoding) — read without forcing a scan. Invalid/unknown input is sanitised
  as before. Most effective on multibyte-heavy already-validated input; the DOM
  is unchanged.
* The HTML serializer's line table is built with `memchr`, and the UTF-8
  validity check is a dedicated validate-only scan (Unicode well-formed table +
  word-at-a-time ASCII), instead of decoding every code point — together ~7%
  faster parsing on a 235 KB document, output byte-identical.
* `Node#{to_html,to_s,outer_html,inner_html}` collect Lexbor's serializer output
  into one growing C buffer (`mkr_buf`) and copy it into a Ruby String once,
  instead of `rb_str_cat` per emitted chunk. The per-chunk Ruby-string growth
  (capacity + coderange bookkeeping over thousands of appends) was the dominant
  cost; the single-copy path is ~1.2–1.3× faster on a 2k-element document
  (now at parity with `nokolexbor`), with byte-identical output.

### Fixed

* The compiled extension exported the entire vendored Lexbor symbol table
  (~1700 `lxb_*` / `lexbor_*` symbols) instead of only `Init_makiri`:
  `-fvisibility=hidden` hides our own sources but not the prebuilt Lexbor static
  library, whose symbols were re-exported into the bundle's dynamic table.
  Loading Makiri together with another Lexbor-based extension (e.g. `nokolexbor`)
  in the same process could then bind that gem's `lxb_*` calls to Makiri's
  different Lexbor version and **segfault**. The linker now exports only
  `Init_makiri` (`-Wl,-exported_symbol,_Init_makiri` on macOS,
  `-Wl,--exclude-libs,ALL` on Linux), keeping Makiri's Lexbor fully private.
  Affects precompiled gems; rebuild required.

## [0.2.0] - 2026-06-04

### Added

* `Element#tag_name` (DOM `tagName`) — the qualified name uppercased for an
  HTML element in an HTML document (`"DIV"`), keeping the original case for
  SVG/MathML; `nil` for non-elements. Complements `#name`, which stays the
  lowercase qualified name.
* `ProcessingInstruction#target` (DOM `target`) — a PI's target name; `nil` for
  other node kinds. Its data is read via `#content`/`#text`.
* `Document#create_processing_instruction(target, data)` (DOM
  `createProcessingInstruction`) and `Document#create_document_fragment` (DOM
  `createDocumentFragment`, an empty fragment to build up programmatically —
  unlike `#fragment` / `DocumentFragment.parse`, which parse HTML). Both produce
  a detached node owned by the document; PI creation fails closed when the data
  contains the `?>` terminator (matching the DOM constraint). (DOM
  `createCDATASection` is intentionally not provided: per WHATWG DOM it throws on
  an HTML document, which is the only kind Makiri produces.)
* `Node#{namespace_uri, prefix, local_name}` — the WHATWG DOM per-node
  namespace accessors on `Element` and `Attribute` (`nil` on other node kinds).
  `namespace_uri` resolves an element's namespace from its node (so an HTML
  element is the XHTML namespace `http://www.w3.org/1999/xhtml`, not `nil` — the
  DOM-faithful value browsers and `namespace-uri()` return; SVG/MathML get their
  own URI), and agrees byte-for-byte with the `namespace-uri()` XPath function.
  For attributes it is `nil` unless prefixed, where it returns the parser-assigned
  foreign-content namespace (`xlink`/`xml`/`xmlns`). `prefix` is the prefix
  segment of the qualified name (`nil` for the usual unprefixed HTML5 case), and
  `local_name` is the name without that prefix. Previously a node's namespace was
  reachable only through XPath (`namespace-uri()`/`local-name()`).
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

[Unreleased]: https://github.com/takahashim/makiri/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/takahashim/makiri/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/takahashim/makiri/releases/tag/v0.1.0
