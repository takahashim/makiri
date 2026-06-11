# Makiri - project guide for Claude Code

Makiri is a Ruby gem: an HTML5 parser + native XPath 1.0 query engine + CSS
selectors, with **no libxml2 / libxslt dependency at any layer**. It parses via
vendored Lexbor and queries via an original XPath engine. Security is a
first-class goal.

The authoritative design is **`docs/design_doc.ja.md`** (Japanese) - read it
before architectural decisions. This file is the operational summary: constraints,
how to build/test, the subsystem map, and the non-obvious gotchas. The exhaustive
API list lives in the code + specs + `CHANGELOG.md`, not here.

## Hard constraints

- **Vanilla Lexbor, no fork, no patches.** `vendor/lexbor` is a git submodule
  pinned to a release **tag**. Never `git apply` to it. Lexbor gaps are absorbed
  in `ext/makiri/lexbor_compat/`, never by editing Lexbor.
- **No libxml2 / libxslt** anywhere - not linked, vendored, or derived. The
  XPath engine is original. See `NOTICE`.
- **C identifier prefix `mkr_`** for everything we write (`mkr_xpath_*`,
  `mkr_parse_html`, `mkr_node_data_t`, `mkr_c{Element,...}`); Lexbor stays `lxb_*`.
- **Security-first / fail-closed.** Enforce per-evaluate XPath budgets and
  node-set caps, validate inputs, never return a truncated/wrong result (raise
  instead). Keep the build hardening flags (`-D_FORTIFY_SOURCE=2`,
  `-fstack-protector-strong`, `-fvisibility=hidden` + `RUBY_FUNC_EXPORTED` on
  `Init_makiri`). **Export only `Init_makiri`** from the compiled extension
  (`extconf.rb`: `-Wl,-exported_symbol,_Init_makiri` on macOS,
  `-Wl,--exclude-libs,ALL` on Linux): `-fvisibility=hidden` hides our own
  sources but *not* the prebuilt vendored Lexbor static lib, so without this the
  bundle re-exports ~1700 `lxb_*`/`lexbor_*` symbols and another Lexbor-based
  gem in the same process (e.g. `nokolexbor`) binds its `lxb_*` calls to our
  different Lexbor version → segfault. Keep Makiri's Lexbor private; verify with
  `nm -gU lib/makiri/makiri.bundle | grep -c ' T _lxb_'` → `0`. Every C change
  must stay clean under ASan+UBSan and keep the fuzzer green.

## Lexbor version

Pinned to **`3a2d595`** (`v3.0.0-25-g3a2d595`, builds cleanly with our
`LEXBOR_BUILD_SHARED=OFF` config). **Normally we pin to a release tag**, but this
is an *untagged master* commit taken deliberately: the latest release tag is
still v3.0.0, and master carries fixes Makiri needs, including **two CSS-selector
fixes we upstreamed** - `#369` (`3a2d595`: class/ID selectors now match
case-sensitively except in quirks mode, like browsers, instead of always
case-insensitively) and `#371` (`940162d`: a prefix-less type selector no longer
defaults to the universal namespace) - plus a **heap-overflow fix in the
`:lexbor-contains()` parser** (`8a14bc0`, reached via `Node#css`), the `#365`
tokenizer size-limit (DoS) fix, `<select size>` NULL-deref, ruby `rp`/`rt`
parse-error, HTML scope/attribute fixes, and encoding/URL memory fixes. All are
bugfixes (no feature/breaking churn). **Move back to a release tag** as soon as
one ships after v3.0.0 (it should contain all of the above) via `git submodule
update`; until then, keep this pin. Still **vanilla, NEVER patched** - the
constraint that relaxed is "release tag only", not "no fork".

## Build / test

```bash
git submodule update --init        # fresh clone only
bundle install
bundle exec rake compile           # builds vendored Lexbor static lib, then the ext
bundle exec rake spec
bundle exec rake clean             # wipe ext build dir (regenerates Makefile next compile)
bundle exec rake clean:lexbor      # wipe vendor/lexbor/{build,dist} (full Lexbor rebuild)
bundle exec ruby -Ilib -r makiri -e 'p Makiri::VERSION'   # smoke load

bundle exec rake sanitize          # rebuild ext w/ -fsanitize=address,undefined, run suite
bundle exec rake fuzz              # robustness fuzzer (spec/fuzz/); FUZZ_ARGS to tune
bundle exec rake fuzz:sanitize     # fuzz under ASan - the C engine's memory-safety net
bundle exec rake leaks             # macOS malloc-leak gate (ASan runs detect_leaks=0,
                                   # so this is the ONLY leak check; flags per-call
                                   # leak stacks through the ext incl. rescued raises)
bundle exec rake oom               # OOM-injection sweep: rebuilds with
                                   # MAKIRI_ALLOC_INJECT=1 and fails each core alloc
                                   # site in turn - every OOM branch must fail closed
                                   # (clean raise or baseline-identical result)
bundle exec rake "sanitize:lexbor" # also build vendored Lexbor under ASan (mraw-arena overflows)
bundle exec rake bench             # perf vs Nokogiri (bench-only gems; runs outside bundle)
```

Requires CRuby >= 3.2 and `cmake`.

### Build / runtime gotchas (read before debugging weirdness)

- **After adding a new `.c` file, run `rake clean compile`** (not plain
  `compile`). extconf globs sources at configure time; a stale Makefile silently
  drops the new file, and macOS's `-undefined dynamic_lookup` turns the missing
  symbols into runtime NULL jumps (segfault), not a link error. (*Header* edits
  are covered without a clean: extconf appends a coarse `$(OBJS): <all project
  headers>` rule to the Makefile, so touching any `.h` - e.g. a struct layout -
  recompiles every object instead of leaving stale-ABI ones behind.)
- **Sanitizer must be run via the rake task, not `bundle exec rspec`.**
  `MAKIRI_SANITIZE=<set>` makes extconf drop `_FORTIFY_SOURCE` and add
  `-fsanitize=<set> -O1 -g`; the task then preloads the ASan runtime
  (`DYLD_INSERT_LIBRARIES` on macOS, `LD_PRELOAD` on Linux). Without the preload
  a late-`dlopen`'d sanitized ext aborts with "interceptors are not working",
  and `bundle exec` drops `DYLD_*` on macOS. `ASAN_OPTIONS` disables
  LSan/container/odr checks (Ruby+Lexbor are uninstrumented); heap-overflow + UB
  in our code still fire. CI runs a separate `sanitize` job on Linux.
- **ASan *stack* instrumentation is deliberately OFF in sanitize builds**
  (`--param asan-stack=0` / clang `-mllvm -asan-stack=0` in extconf). CRuby is
  built with `RUBY_SETJMP = __builtin_setjmp`, so `rb_raise` unwinds via
  `__builtin_longjmp`, which ASan cannot intercept: a raise crossing an
  instrumented frame (ours, or a Ruby raise through `rb_protect` under the
  evaluator) leaves that frame's stack-redzone poison behind, and a later
  interceptor (memcpy & co.) in the uninstrumented interpreter trips over the
  stale shadow - a layout-sensitive spurious report that ASan then aborts on
  while rendering (`asan_thread.cpp` `kCurrentStackFrameMagic` CHECK; this took
  CI's sanitize jobs down via the XML-mutation PBT, which raises thousands of
  times - see `docs/ci-crash/INVESTIGATION.md`). Heap red zones, UBSan, and the
  `mkr_xml_node.c` arena poisoning are unaffected; only stack-buffer checks are
  lost (`-fstack-protector-strong` still covers smashing). Do not re-enable
  without solving the `__builtin_longjmp` shadow problem.
- **A plain `sanitize` build does NOT catch overflows inside Lexbor's `mraw`
  bump arena.** A sub-allocation overrunning into the next one stays within one
  malloc'd chunk, so the heap allocator's red-zones never see it (this is exactly
  how the v3.0.0 `:lexbor-contains()` overflow hid from ASan). To catch that
  class, build Lexbor *itself* under ASan: `MAKIRI_SANITIZE_LEXBOR=1` makes
  extconf pass `-DLEXBOR_BUILD_WITH_ASAN=ON` (Lexbor's mraw is ASan-aware - it
  poisons the arena and unpoisons each allocation, so an intra-arena overrun
  writes into poisoned memory and ASan reports it). Drive it with `rake
  "sanitize:lexbor"` (slow: full instrumented Lexbor rebuild; FUZZ_ARGS routes to
  the fuzzer). extconf stamps the Lexbor install mode (`plain`/`asan`) and
  auto-rebuilds on a switch, so an instrumented Lexbor never leaks into a normal
  build. Switching the Lexbor *commit* still needs `rake clean:lexbor` (the stamp
  tracks mode, not revision). No Lexbor patch - it is a vendor build flag.
- **Our XML bump arena (`mkr_xml_node.c`) is ASan-red-zoned, so its intra-arena
  overflows ARE caught** - the same blind spot as Lexbor's mraw, but this is our
  own TU. `arena_alloc` poisons each fresh 64 KiB chunk and unpoisons only the
  bytes a cut hands out (the `[size, need)` alignment tail stays poisoned), so a
  write past one `arena_node`/`arena_bytes`/`scratch_bytes` cut hits poisoned
  memory and ASan reports it. It auto-activates under any `-fsanitize=address`
  build (`__has_feature`/`__SANITIZE_ADDRESS__`) - no extra flag, unlike Lexbor -
  and is a no-op otherwise. So plain `rake sanitize` / `fuzz:sanitize --target
  xml,mutate` already cover the arena. Everything else we write (XPath engine,
  CSS lowering, glue, core) uses plain malloc/calloc/realloc, which ASan
  red-zones per allocation - no arena, no special handling. Keep the unpoison at
  exactly the requested `size` (not `need`); widening it to `need` would silence
  off-by-one-into-padding overflows.
- **`node->user` is reserved** for source-location byte offsets (see below) - do
  not repurpose it.
- The fuzzer's `spec/fuzz/*.rb` are deliberately not `*_spec.rb`, so `rake spec`
  ignores them; findings land in `spec/fuzz/regressions/` (gitignored).

## Layout

```
lib/makiri/                Ruby API (Document, Node, Element, NodeSet, XPathContext, ...)
ext/makiri/
  makiri.{c,h}             Init_makiri, module/class refs
  core/                    Ruby-free safety primitives, split by concern under
                           the mkr_core.h umbrella: mkr_alloc (overflow-checked
                           alloc/grow), mkr_hash (ptr hash + pow2 sizer), mkr_text
                           (string-type lattice / mkr_verified_text_t), mkr_buf
                           (growable buffer + mkr_spanbuf bounded writer),
                           mkr_span (bounded reader - byte-scanning parser TUs
                           may read input ONLY through it; lint-enforced via
                           raw_scan_call / raw_cursor_member), mkr_utf8 (the one
                           validator + strict 1-codepoint decoder)
  bridge/                  the Ruby boundary - the ONLY layer allowed raw Ruby String
                           access (RSTRING) and mkr_verified_text_t minting
  glue/                    Ruby <-> C surface, one file per feature (ruby_node/doc/node_set/
                           xpath/css/serialize/mutate.c)
  xpath/                   native XPath 1.0 engine (mkr_xpath_*)
  lexbor_compat/           attr->owner index, source location, post-parse orchestration
vendor/lexbor/             git submodule, pinned 3a2d595 (v3.0.0-25), NEVER patched
spec/fuzz/                 grammar-aware robustness fuzzer
bench/                     Nokogiri-comparison benchmark
docs/design_doc.ja.md      authoritative design (read this)
```

## Subsystems

**Text-input contract.** Parsing **honours the input String's encoding**
(`mkr_ruby_to_utf8`, `bridge/ruby_string.c`): UTF-8 / US-ASCII / ASCII-8BIT pass
through untouched (the UTF-8 common case is a single encoding compare - no
transcode, no copy), any other encoding (Shift_JIS, EUC-JP, ISO-8859-1, ...) is
`rb_str_encode`'d to UTF-8 (invalid/undef → U+FFFD) so its content survives
instead of being read as raw UTF-8. After that the bytes are UTF-8. **HTML
parsing then decodes leniently like a browser**: `mkr_utf8_sanitize`
(`utf8_input.c`) replaces any remaining invalid UTF-8 with U+FFFD (a NUL is left
for the HTML5 tokenizer to drop/replace), so parse/fragment **never fail** on
bad bytes and the DOM is always valid UTF-8. The validation is a dedicated
validate-only scan (Unicode well-formed table + word-at-a-time ASCII); it is
skipped entirely when the String's cached coderange (read via `ENC_CODERANGE`,
no forced scan) already proves it valid - `mkr_parse_html`'s `assume_valid` and
`mkr_ruby_str_known_valid_utf8`. The **programmatic APIs are strict**:
`mkr_verify_text` (`bridge/ruby_string.c`) raises `Makiri::Error` for invalid UTF-8 or an
embedded NUL at the XPath/CSS/mutation boundaries (expr, selector, attribute
name/value, `content=`, `name=`, `create_*`, variable/namespace) - never
truncate/repair. Don't drop these checks; the engine assumes NUL-terminated,
valid-UTF-8 C strings.

**Parsing & source location** (`lexbor_compat/post_parse.c`, `source_loc.c`).
`mkr_parse_html` drives Lexbor's low-level pipeline (`parser_create`/`init` →
`parse_chunk_begin` → override the tokenizer's token-done callback, **chaining**
the parser's tree builder → `chunk_process`/`chunk_end`) so it can record each
element start-tag's byte offset (`token->begin`). After the tree is built,
`mkr_pos_assign_to_dom` walks pre-order, matches each element to the next
recorded token by tag id (bounded lookahead), and stamps `offset+1` into
`node->user`; a line table (`mkr_lines_t`, built once) resolves that to a
1-based line. `Node#line` returns an Integer, or **nil** when unplaceable
(parser-inserted implicit html/head/body, text/comment/attribute nodes) - never
a wrong line. Recorder bounded by `MKR_POS_MAX_TOKENS` (fail closed → nil, never
wrong). The document outlives `lxb_html_parser_destroy` (it only unrefs
tkz/tree). Tracking is **always on**: it rides the parse (~7% over no-tracking,
measured). An earlier `line: :text`/`:none` option was removed - `:text` (a
separate source scan) measured *slower* (~36%) and was only approximate.

**attr→owner index** (`lexbor_compat/dom_index.c`). Lexbor never links an
attribute back to its element, so we build an open-addressing hash (pointer
keys, lazy two-phase build - count, size once, fill; iterative DFS, no recursion
→ no stack DoS; OOM fails closed and retries). The build also **backfills each
attribute's `node.parent`** to its owner (safe: Lexbor walks the tree via
first_child/next, never attr.parent), so the XPath engine handles
parent/ancestor axes and document-order over attributes with no special-casing.
Reached via `mkr_parsed_attr_owner`; `mkr_parsed_dom_index_invalidate` drops it
after any mutation so it rebuilds on the next query. The same walk **co-builds
an element index** (`tag id → elements`, document-order CSR) used by the XPath
`//tag` fast path; only Lexbor's static tag-id range `[1, LXB_TAG__LAST_ENTRY)`
is bucketed - custom-element tag ids are *pointer values* (`lxb_tag_append`),
so those elements are left out and `//customtag` falls back to the tree walk.
Reached via `mkr_parsed_element_index` / `mkr_element_index_tag` /
`mkr_element_index_has_foreign`; invalidated with the attr index.

**text index** (`lexbor_compat/text_index.c`). Removes the per-call descendant
walk from text extraction (the cache-bound cost on Lexbor's 96-byte nodes). One
lazy build (count, size once, fill; explicit **heap**-stack DFS via
`mkr_grow_reserve`, no recursion → no stack DoS) records a flat document-order
array of every TEXT/CDATA node's **borrowed** `mkr_borrowed_text_t` slice, a
prefix-sum of their lengths, and a pointer-keyed open-addressing hash mapping
each element/fragment to the `[start,end)` run of slices its subtree owns. A
`Node#text` is then a hash lookup + `mkr_ruby_str_from_slices` (one pre-sized
memcpy run; **~4× faster than libxml2 at all sizes**), no element node touched.
Cached on `mkr_parsed_t.text_index`; `mkr_parsed_text_index_invalidate` drops it
from the **same single mutation hook** as the attr index, so a borrowed slice
can never point at reallocated/detached text storage. Reached via
`mkr_parsed_text_slices` (returns 0 → caller walks: fragments, build OOM).
Fail-closed: a build OOM leaves it unbuilt and the walk fallback serves.

**XPath engine** (`xpath/mkr_xpath_*.{c,h}`). Original implementation: lexer →
recursive-descent parser → AST → evaluator + 26 built-in functions. The only
external hook is `mkr_dom_node_name_qualified` (in `mkr_xpath.c`). Per-evaluate
budgets (op count, recursion depth, step/predicate/arg counts, node-set & string
caps) live in `mkr_xpath.c` and fail closed with `MKR_XPATH_ERR_LIMIT`. Ruby:
`Node#{xpath,at_xpath}(expr, handler=nil)`, `Makiri::XPathContext`
(`.new`, `#evaluate`, `#register_namespace`/`#register_ns`, `#register_variable`).
`#xpath` returns a NodeSet for node-sets, else String/Float/boolean. Errors map
SYNTAX→`XPath::SyntaxError`, LIMIT→`XPath::LimitExceeded`, else `Makiri::Error`.
Custom functions: unknown calls route through the engine resolver to
`handler.<local_name with - → _>`, run under `rb_protect` (a Ruby exception
becomes `Makiri::Error`, never a long-jump through the evaluator); node-set
returns from a foreign document are rejected. The **namespace axis is not
implemented** (raises "not implemented", never silently empty); Nokogiri/libxml2
*does* implement it (e.g. `<svg>` in HTML yields the `xml`+`svg` namespace
nodes), so this is a documented behaviour difference - see README "Differences
from Nokogiri". `namespace-uri()`/`local-name()` are implemented.
**Namespace matching of name tests is strict by default** (HTML5/WHATWG-faithful,
like browsers' `document.evaluate` and `Nokogiri::HTML5`): an *unprefixed*
element name test resolves in the HTML namespace, so `//div` matches but
`//svg`/`//path` do NOT - foreign (SVG/MathML) elements need a registered
prefix (`//svg:path`). Pass `namespace_matching: :lax` (on `Node#{xpath,at_xpath}`
or `XPathContext.new`) for the namespace-agnostic, `Nokogiri::HTML`-style match
where `//path` finds the SVG element. The mode affects *only* unprefixed
element name tests; prefixed tests, the `*` wildcard, and attribute tests are
unchanged (see `mkr_xpath_internal.h` §2–§5). Makiri keeps HTML elements in the
XHTML namespace (so `namespace-uri()` is correct, unlike `Nokogiri::HTML5`'s
null).

**CSS** (`glue/ruby_html_css.c`). `Node#{css,at_css,matches?}` via Lexbor's
`lxb_selectors`. The engine (`css_memory`+`css_parser`+`css_selectors` and the
`selectors` traversal object) is **built once and reused for every query** -
safe with no locking because CSS holds the GVL throughout (it never releases
it), so calls are serialized; between calls only the parsed list's arena is
reset (`lxb_css_memory_clean`) and the parser returned to its CLEAN stage
(`lxb_css_parser_clean`), and the traversal engine self-cleans after each
find/match. Per-call create/destroy used to dominate a cheap query and lost to
nokolexbor on `at_css('#id')`; reuse makes it ~5× faster than nokolexbor.
`lxb_selectors_find` runs with `MATCH_FIRST` to dedup comma lists; `at_css`
**stops at the first match and wraps that one node** (no NodeSet / no Ruby
`#first`). Results are **descendant-only** (context node excluded, like Nokogiri)
and in document order; capped at `MKR_NODE_SET_MAX`; malformed →
`Makiri::CSS::SyntaxError` (the shared engine is reset, so it recovers).

**Serialization** (`glue/ruby_html_serialize.c`). `Node#{to_html,to_s,outer_html}` =
Lexbor `serialize_tree_cb`, `#inner_html` = `serialize_deep_cb`; the callback
collects Lexbor's many small chunks into one growing C buffer (`mkr_buf`,
**pre-reserved to ~the output size** via `mkr_buf_reserve` so the per-chunk
appends don't realloc on every geometric step) and the
whole thing is copied into a UTF-8 Ruby String once - markedly faster than
`rb_str_cat` per chunk (its per-append capacity + coderange bookkeeping was the
serializer's dominant cost), and at parity with `nokolexbor`. (Serializing
straight into a growing Ruby String avoids the final copy but measured *slower* -
the intermediate growth is GC-tracked; the untracked C buffer + one copy wins.)
`pretty: true` uses `serialize_pretty_*` (Lexbor
quotes text nodes in that mode). A `DocumentFragment` serializes via the deep
serializer (the tree serializer rejects a fragment node). `Node#text`/`#content`
(`mkr_node_content`) serves descendant text from the **text index** (see
below) - a hash lookup + one pre-sized `mkr_ruby_str_from_slices` memcpy run,
no per-call tree walk - and falls back to a direct iterative walk for
non-indexed nodes (fragments). For a Document it returns the **root element's**
text (DOM makes a Document's textContent null, which is not what callers want).

**Mutation** (`glue/ruby_mutate.c`). Tree edits (`add_child`/`<<`,
`add_previous_sibling`/`before`, `add_next_sibling`/`after`, `remove`/`unlink`,
`replace`) over Lexbor insert/remove. We **detach, never destroy** - the arena
owns node memory and live Ruby wrappers may alias a removed node; move semantics
= detach-then-insert. Attribute `[]=` / `delete`; `Node#name=` renames in place
(create a fresh element so the doc interns the name, copy its
`local_name`/`prefix`/`ns`/`upper_name`/`qualified_name`, destroy the throwaway -
identity preserved); `Node#content=`. `Document#{create_element,create_text_node}`.
Fragments: `DocumentFragment.parse(html)` (own backing doc) and
`Document#fragment(html)` (bound to a doc) parse in a throwaway `<body>` context
and `lxb_dom_document_import_node` (deep) each child into the target arena;
inserting a fragment splices its **children**. Guards Lexbor omits: same-document
only, no self-cycles, attribute nodes can't be tree children. Every structural /
attribute change calls `mkr_parsed_dom_index_invalidate`.

**Ruby surface niceties.** Node classes: Document, Element, Attribute, Text,
Comment, CData, ProcessingInstruction, DocumentFragment (mapped in
`mkr_wrap_node` by DOM node type). Convenience: `Node#{root,ancestors,path}`
(path round-trips through `#at_xpath`), `Node#{attributes,to_h}`,
`Node#{search,at}` (CSS/XPath auto-detect: starts `/ ./ .. .// ( @` or contains
`::` ⇒ XPath, else CSS), `Document#{body,head,encoding,meta_encoding}`
(`encoding` is always "UTF-8"). `NodeSet#{|,+,&,-}` are identity-dedup,
encounter-order (**not** doc-order), `#{css,xpath,search}` run per node and union
(return `self` when empty so an empty set stays a NodeSet), `#{last,at,remove}`.
`Element.new(name, doc)` / `Text.new(content, doc)` delegate to
`Document#create_*` (they override `.new`, since the allocator is undef'd).

## Performance

**Makiri currently meets or beats Nokogiri/libxml2 on every `rake bench` row**
(parse ~3×, css ~12×, at_css ~6000× vs Nokogiri / ~5× vs nokolexbor, serialize ~4×, traverse ~1.2×, xpath
attr-axis ~1.3×, `[@attr='v']` predicate ~1.5×, `//tag` ~3.4× faster, full-text
extraction ~4×). Plus parsing scales across threads (~2× on 8 cores) since
it releases the GVL. Key decisions that got there, worth not regressing:

- **Parsing releases the GVL; XPath evaluation does NOT** (`ruby_doc.c`,
  `ruby_xpath.c`): parse copies the source to a C buffer then runs
  `mkr_parse_html` under `rb_thread_call_without_gvl` - safe because a freshly
  parsed document is not yet shared, so it can't race anything. **XPath holds
  the GVL for the whole evaluation by design** (`mkr_eval_compiled` is a plain
  GVL-held call). The engine and DOM are not thread-safe against concurrent
  mutation, and holding the GVL makes that safe *by construction*: the GVL
  serialises all Ruby-thread C code, so an XPath walk never runs in parallel
  with a tree mutation, with another `evaluate` on the same context, or with a
  `register_variable`/`register_namespace`/`node=` on the same context - no
  locking needed (and none is used). An earlier version released the GVL for
  handler-free XPath (it scaled queries ~3.3× across threads), but the locking
  required to make a GVL-released walk safe against shared-document mutation was
  judged not worth the verification burden; single-thread XPath speed is
  unaffected by holding the GVL. **Do not reintroduce a GVL-released XPath
  path** without the full document/context locking story. Parse still scales
  (~2× on 8 cores); verify with `bench`'s threaded rows and the `GC.stress`
  `spec/threading_spec.rb` (which now also asserts shared-document XPath+mutation
  and shared-context evaluate are crash-free under the GVL).
- **`//tag` is served from the element index** (`mkr_xpath_eval.c`
  `try_descendant_tag_index`): a document-rooted, predicate-free, unprefixed
  descendant name-test pushes the tag bucket instead of walking. Pure-HTML only
  (`has_foreign` guard) and each candidate is re-checked with
  `node_principal_match`, so the result is identical to the walk; custom/unknown
  tag names fall through. See the element index note above.

- **The CSS engine is built once and reused** (`glue/ruby_html_css.c`, see the
  subsystem note): the per-call create/init/destroy of the Lexbor CSS object
  graph dominated a cheap query and lost to nokolexbor on `at_css('#id')`; a
  process-global engine (safe because CSS holds the GVL throughout) reset with
  `lxb_css_memory_clean` + `lxb_css_parser_clean` between calls makes `at_css`
  ~6000× Nokogiri / ~5× nokolexbor (was ~1.16× *slower* than nokolexbor). `at_css`
  also wraps the single first match directly (no NodeSet / no Ruby `#first`). Do
  not reintroduce per-call engine teardown; verify with `bench`'s `at_css`/`css`
  rows and `fuzz:sanitize --target css` (the reuse is the memory-safety risk).
- **`Node#text` is served from the text index** (`lexbor_compat/text_index.c`,
  see the subsystem note): a per-document, lazily-built, mutation-invalidated
  map from node → its document-order text-slice run, turning text extraction
  into a hash lookup + one pre-sized memcpy instead of a cache-bound walk over
  96-byte nodes (~4× libxml2, vs the former ~parity). The walk fallback stays
  for non-indexed nodes. Do not regress to walking on the indexed path; verify
  with `bench`'s "full document text" row and `spec/text_index_spec.rb` (which
  asserts byte-identity with a plain walk across subtrees + mutations).
- **String-value cache is hashed** (`mkr_xpath_value.c`): a pointer-keyed
  open-addressing index over an ordered store, so per-node predicate compares
  are O(1), not the old O(n²) linear scan. The ordered store keeps
  snapshot/partial-truncate working for nested (handler-triggered) evals.
- **`[@name]` / `[@name='lit']` predicates take a direct-attribute fast path**
  (`mkr_match_attr_pred`/`mkr_filter_attr_pred` in `mkr_xpath_eval.c`): a
  position-independent filter via `lxb_dom_element_has_attribute`/`get_attribute`
  instead of building a throwaway node-set per candidate; anything else falls
  through to the generic evaluator.
- **`Node#at_xpath` first-match short-circuit** (`mkr_xpath_eval.c`
  `mkr_try_first_match`, entered via `mkr_xpath_eval_compiled_first`): `at_xpath`
  wants only node-set[0], so for the common "first descendant by name (+ a
  position-independent `[@a]`/`[@a='v']` predicate)" shapes - `//x`, `//x[@a]`,
  `//*[@a='v']`, `.//x`, `descendant::x[...]` (after the `//` peephole; one or two
  steps) - it walks the subtree in **document order and stops at the first match**
  instead of materialising+sorting the whole set, the XPath analogue of at_css's
  `MATCH_FIRST`. Cost becomes O(position of first match): a front hit is ~µs
  (vs ~280µs full-eval), trailing/absent fall back to a full scan. Reuses
  `node_principal_match` + the `[@attr]` matcher so it's **byte-identical to
  `xpath(e).first`** (asserted by `spec/at_xpath_first_spec.rb`). Anything else
  (positional predicates, functions/variables, reverse axes, unions, prefixes,
  longer paths) returns 0 from the recogniser → full evaluator. Only `at_xpath`
  uses it; `xpath` always builds the full set.
- **Per-context compiled-AST cache** (`mkr_xpath.c`): an `XPathContext` parses
  each expression once and re-runs the cached AST (bounded by `MKR_AST_CACHE_MAX`).
  `Node#xpath` uses a throwaway context and does not cache.
- Tree-walk speed is structurally capped by Lexbor's 96-byte node (we can't
  shrink it); investigated nodeset-pool / prefetch follow-ups were **not** shipped
  because, with no remaining slower-than-Nokogiri row, they'd add lifetime /
  speculative complexity for no measurable win.

Working on perf: capture a `rake bench` baseline, change one thing, re-bench,
and **ship only a measurable win** that keeps `rake spec` + `rake fuzz:sanitize`
green. (Note: a per-node Ruby wrapper cache was considered and rejected -
`node->user` is taken, and a node→VALUE side table creates a GC-lifetime problem
for no clear gain on already-winning paths.)
