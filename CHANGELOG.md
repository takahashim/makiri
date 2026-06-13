# Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

* XML processing-instruction targets now follow XML 1.0 §2.6: a PITarget is a
  `Name`, not an NCName, so a colon is permitted (`<?a:b ...?>` parses, and
  `create_processing_instruction("a:b", ...)` succeeds). Only the reserved `xml`
  (any case) is still rejected. Previously a colon in a PI target was rejected as
  not-well-formed, which was stricter than the spec (a PI target is not subject to
  namespace processing).

### Added

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

## [0.4.0] - 2026-06-12

### Added

* `Makiri::Lexbor::CSS.parse_stylesheet(text)`, a thin binding over Lexbor's
  CSS stylesheet parser that returns the parsed rules as plain Ruby primitives
  (`{type: :style, selectors: [{text:, specificity: [a,b,c]}, ...],
  declarations: [{name:, value:, important:}, ...]}` and nested
  `{type: :media, condition:, rules: [...]}`, in source order). Selector
  specificity and value normalization come from Lexbor; `css-syntax-3` error
  recovery means a broken stylesheet yields its valid rules instead of raising.
  Hosts the new `Makiri::Lexbor::*` namespace (the unabstracted lexbor-native
  surface, distinct from the Nokogiri-compatible `Makiri::*`).

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

* **Native XML 1.0 reader + in-place editor** - `Makiri::XML::Document.parse(source)`
  / `Makiri::XML(source)`. No libxml2: a strict, fail-closed parser builds its own
  node arena (case- and namespace-preserving), queried by the native XPath engine.
  * Strict & secure: fail-closed decode (bad UTF-8 / NUL -> `XML::SyntaxError`),
    duplicate attributes rejected, XML 1.0 only; verified against the W3C XML
    Conformance Test Suite.
  * Encoding autodetected (BOM / `<?xml encoding?>`); a contradicting String
    encoding is a fatal error, not a silent mis-decode.
  * DoS-bounded by a single arena byte ceiling (default 256 MiB; raise per parse
    with `max_bytes:`).
  * `<!DOCTYPE>` recognized but **not processed** (`#internal_subset` ->
    `XML::DocumentType`); zero entity/DTD I/O, so **XXE and billion-laughs are
    structurally impossible**. Kept off the tree, as in libxml2.
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
* A **frozen node is genuinely immutable** - every mutator raises `FrozenError`.

### Changed

* CSS queries reuse one shared Lexbor engine (GVL-safe) and `at_css` wraps the match
  directly: `at_css('#id')` ~5x faster than nokolexbor (was ~1.16x slower).
* HTML serialization pre-reserves its buffer - `to_html` now at parity with nokolexbor.
* Node-class names are the WHATWG DOM interface names (`CDATASection`, `Attr`,
  `DocumentType`, ...), with the Nokogiri spellings (`CDATA`, `DTD`) kept as aliases;
  added `Node#cdata?`.
* Text-index range table uses `uint32` bounds (24 -> 16 B/entry; ~27% less retained
  index, byte-identical text).
* Parsing **honours the input String's encoding** - Shift_JIS / EUC-JP / ... are now
  transcoded to UTF-8 instead of mangled.
* Parsing skips its UTF-8 validation scan when the String's coderange already proves
  it valid.
* Faster HTML parse/serialize: `memchr` line table + validate-only UTF-8 scan (~7%),
  and a single-copy serializer buffer (~1.2-1.3x).

### Fixed

* **Hardened the HTML/XML representation boundary.** HTML (Lexbor) and XML (arena)
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
selectors for Ruby - built on vendored [Lexbor](https://lexbor.com/) with **no
libxml2 / libxslt dependency at any layer**.

### Added

**Parsing & DOM**

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
  `Makiri::Error` - never silently truncated, repaired, or reinterpreted.
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

[Unreleased]: https://github.com/takahashim/makiri/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/takahashim/makiri/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/takahashim/makiri/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/takahashim/makiri/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/takahashim/makiri/releases/tag/v0.1.0
