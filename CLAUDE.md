# Makiri — project guide for Claude Code

Makiri is a Ruby gem: an HTML5 parser + native XPath 1.0 query engine + CSS
selectors, with **no libxml2 / libxslt dependency at any layer**. It parses via
vendored Lexbor and queries via an original XPath engine. Security is a
first-class goal.

The authoritative design is **`docs/design_doc.ja.md`** (Japanese) — read it
before architectural decisions. This file is the operational summary: constraints,
how to build/test, the subsystem map, and the non-obvious gotchas. The exhaustive
API list lives in the code + specs + `CHANGELOG.md`, not here.

## Hard constraints

- **Vanilla Lexbor, no fork, no patches.** `vendor/lexbor` is a git submodule
  pinned to a release **tag**. Never `git apply` to it. Lexbor gaps are absorbed
  in `ext/makiri/lexbor_compat/`, never by editing Lexbor.
- **No libxml2 / libxslt** anywhere — not linked, vendored, or derived. The
  XPath engine is original. See `NOTICE`.
- **C identifier prefix `mkr_`** for everything we write (`mkr_xpath_*`,
  `mkr_parse_html`, `mkr_node_data_t`, `mkr_c{Element,…}`); Lexbor stays `lxb_*`.
- **Security-first / fail-closed.** Enforce per-evaluate XPath budgets and
  node-set caps, validate inputs, never return a truncated/wrong result (raise
  instead). Keep the build hardening flags (`-D_FORTIFY_SOURCE=2`,
  `-fstack-protector-strong`, `-fvisibility=hidden` + `RUBY_FUNC_EXPORTED` on
  `Init_makiri`). Every C change must stay clean under ASan+UBSan and keep the
  fuzzer green.

## Lexbor version

Pinned to **v3.0.0** (builds cleanly with our `LEXBOR_BUILD_SHARED=OFF` config).
Fixes landed on master *after* v3.0.0 that we want — pick them up at the **next
release tag** via `git submodule update`, not an untagged commit:
`#365` tokenizer size-limit fix (DoS-relevant), `<select size>` NULL-deref fix,
ruby `rp`/`rt` parse-error fix.

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
bundle exec rake fuzz:sanitize     # fuzz under ASan — the C engine's memory-safety net
bundle exec rake bench             # perf vs Nokogiri (bench-only gems; runs outside bundle)
```

Requires CRuby >= 3.2 and `cmake`.

### Build / runtime gotchas (read before debugging weirdness)

- **After adding a new `.c` file, run `rake clean compile`** (not plain
  `compile`). extconf globs sources at configure time; a stale Makefile silently
  drops the new file, and macOS's `-undefined dynamic_lookup` turns the missing
  symbols into runtime NULL jumps (segfault), not a link error.
- **Sanitizer must be run via the rake task, not `bundle exec rspec`.**
  `MAKIRI_SANITIZE=<set>` makes extconf drop `_FORTIFY_SOURCE` and add
  `-fsanitize=<set> -O1 -g`; the task then preloads the ASan runtime
  (`DYLD_INSERT_LIBRARIES` on macOS, `LD_PRELOAD` on Linux). Without the preload
  a late-`dlopen`'d sanitized ext aborts with "interceptors are not working",
  and `bundle exec` drops `DYLD_*` on macOS. `ASAN_OPTIONS` disables
  LSan/container/odr checks (Ruby+Lexbor are uninstrumented); heap-overflow + UB
  in our code still fire. CI runs a separate `sanitize` job on Linux.
- **`node->user` is reserved** for source-location byte offsets (see below) — do
  not repurpose it.
- The fuzzer's `spec/fuzz/*.rb` are deliberately not `*_spec.rb`, so `rake spec`
  ignores them; findings land in `spec/fuzz/regressions/` (gitignored).

## Layout

```
lib/makiri/                Ruby API (Document, Node, Element, NodeSet, XPathContext, ...)
ext/makiri/
  makiri.{c,h}             Init_makiri, module/class refs
  glue/                    Ruby <-> C bridge, one file per surface (ruby_node/doc/node_set/
                           xpath/css/serialize/mutate.c)
  xpath/                   native XPath 1.0 engine (mkr_xpath_*)
  lexbor_compat/           attr->owner index, source location, post-parse orchestration
vendor/lexbor/             git submodule, pinned v3.0.0, NEVER patched
spec/fuzz/                 grammar-aware robustness fuzzer
bench/                     Nokogiri-comparison benchmark
docs/design_doc.ja.md      authoritative design (read this)
```

## Subsystems

**Parsing & source location** (`lexbor_compat/post_parse.c`, `source_loc.c`).
`mkr_parse_html` drives Lexbor's low-level pipeline (`parser_create`/`init` →
`parse_chunk_begin` → override the tokenizer's token-done callback, **chaining**
the parser's tree builder → `chunk_process`/`chunk_end`) so it can record each
element start-tag's byte offset (`token->begin`). After the tree is built,
`mkr_pos_assign_to_dom` walks pre-order, matches each element to the next
recorded token by tag id (bounded lookahead), and stamps `offset+1` into
`node->user`; a line table (`mkr_lines_t`, built once) resolves that to a
1-based line. `Node#line` returns an Integer, or **nil** when unplaceable
(parser-inserted implicit html/head/body, text/comment/attribute nodes) — never
a wrong line. Recorder bounded by `MKR_POS_MAX_TOKENS` (fail closed → nil, never
wrong). The document outlives `lxb_html_parser_destroy` (it only unrefs
tkz/tree). Tracking is **always on**: it rides the parse (~7% over no-tracking,
measured). An earlier `line: :text`/`:none` option was removed — `:text` (a
separate source scan) measured *slower* (~36%) and was only approximate.

**attr→owner index** (`lexbor_compat/attr_owner.c`). Lexbor never links an
attribute back to its element, so we build an open-addressing hash (pointer
keys, lazy two-phase build — count, size once, fill; iterative DFS, no recursion
→ no stack DoS; OOM fails closed and retries). The build also **backfills each
attribute's `node.parent`** to its owner (safe: Lexbor walks the tree via
first_child/next, never attr.parent), so the XPath engine handles
parent/ancestor axes and document-order over attributes with no special-casing.
Reached via `mkr_parsed_attr_owner`; `mkr_parsed_attr_index_invalidate` drops it
after any mutation so it rebuilds on the next query. The same walk **co-builds
an element index** (`tag id → elements`, document-order CSR) used by the XPath
`//tag` fast path; only Lexbor's static tag-id range `[1, LXB_TAG__LAST_ENTRY)`
is bucketed — custom-element tag ids are *pointer values* (`lxb_tag_append`),
so those elements are left out and `//customtag` falls back to the tree walk.
Reached via `mkr_parsed_element_index` / `mkr_element_index_tag` /
`mkr_element_index_has_foreign`; invalidated with the attr index.

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
implemented** (raises "not implemented", never silently empty).

**CSS** (`glue/ruby_css.c`). `Node#{css,at_css}` via Lexbor's `lxb_selectors`
(per call: build `css_memory`→`css_parser`+`css_selectors`→`selectors`, parse,
`lxb_selectors_find` with `MATCH_FIRST` to dedup comma lists, tear down).
Results are **descendant-only** (context node excluded, like Nokogiri) and in
document order; capped at `MKR_NODE_SET_MAX`; malformed → `Makiri::CSS::SyntaxError`;
`at_css` stops at the first match.

**Serialization** (`glue/ruby_serialize.c`). `Node#{to_html,to_s,outer_html}` =
Lexbor `serialize_tree_cb`, `#inner_html` = `serialize_deep_cb`, both streaming
into a UTF-8 Ruby String. `pretty: true` uses `serialize_pretty_*` (Lexbor
quotes text nodes in that mode). A `DocumentFragment` serializes via the deep
serializer (the tree serializer rejects a fragment node). `Node#text`/`#content`
over an element streams descendant text directly in `mkr_node_content` (no
Lexbor temp buffer); for a Document it returns the **root element's** text (DOM
makes a Document's textContent null, which is not what callers want).

**Mutation** (`glue/ruby_mutate.c`). Tree edits (`add_child`/`<<`,
`add_previous_sibling`/`before`, `add_next_sibling`/`after`, `remove`/`unlink`,
`replace`) over Lexbor insert/remove. We **detach, never destroy** — the arena
owns node memory and live Ruby wrappers may alias a removed node; move semantics
= detach-then-insert. Attribute `[]=` / `delete`; `Node#name=` renames in place
(create a fresh element so the doc interns the name, copy its
`local_name`/`prefix`/`ns`/`upper_name`/`qualified_name`, destroy the throwaway —
identity preserved); `Node#content=`. `Document#{create_element,create_text_node}`.
Fragments: `DocumentFragment.parse(html)` (own backing doc) and
`Document#fragment(html)` (bound to a doc) parse in a throwaway `<body>` context
and `lxb_dom_document_import_node` (deep) each child into the target arena;
inserting a fragment splices its **children**. Guards Lexbor omits: same-document
only, no self-cycles, attribute nodes can't be tree children. Every structural /
attribute change calls `mkr_parsed_attr_index_invalidate`.

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
(parse ~3×, css ~12×, at_css ~1000×, serialize ~4×, traverse ~1.2×, xpath
attr-axis ~1.3×, `[@attr='v']` predicate ~1.5×, `//tag` ~3.4× faster, full-text
extraction ~parity). Plus parsing scales across threads (~2× on 8 cores) since
it releases the GVL. Key decisions that got there, worth not regressing:

- **Parsing and handler-free XPath release the GVL** (`ruby_doc.c`,
  `ruby_xpath.c`): parse copies the source to a C buffer then runs
  `mkr_parse_html` under `rb_thread_call_without_gvl`; `Node#xpath` /
  `XPathContext#evaluate` parse the AST under the GVL then run
  `mkr_xpath_eval_compiled` without it (the engine and all `lexbor_compat`/
  `xpath` code are Ruby-free — Ruby is re-entered only via the custom-function
  resolver, so the GVL is held whenever a handler is installed). Threads parse
  and query concurrently (~2×/~3.3× on 8 cores); verify with `bench`'s threaded
  rows and the `GC.stress` `spec/threading_spec.rb`.
- **`//tag` is served from the element index** (`mkr_xpath_eval.c`
  `try_descendant_tag_index`): a document-rooted, predicate-free, unprefixed
  descendant name-test pushes the tag bucket instead of walking. Pure-HTML only
  (`has_foreign` guard) and each candidate is re-checked with
  `node_principal_match`, so the result is identical to the walk; custom/unknown
  tag names fall through. See the element index note above.

- **String-value cache is hashed** (`mkr_xpath_value.c`): a pointer-keyed
  open-addressing index over an ordered store, so per-node predicate compares
  are O(1), not the old O(n²) linear scan. The ordered store keeps
  snapshot/partial-truncate working for nested (handler-triggered) evals.
- **`[@name]` / `[@name='lit']` predicates take a direct-attribute fast path**
  (`mkr_match_attr_pred`/`mkr_filter_attr_pred` in `mkr_xpath_eval.c`): a
  position-independent filter via `lxb_dom_element_has_attribute`/`get_attribute`
  instead of building a throwaway node-set per candidate; anything else falls
  through to the generic evaluator.
- **Per-context compiled-AST cache** (`mkr_xpath.c`): an `XPathContext` parses
  each expression once and re-runs the cached AST (bounded by `MKR_AST_CACHE_MAX`).
  `Node#xpath` uses a throwaway context and does not cache.
- Tree-walk speed is structurally capped by Lexbor's 96-byte node (we can't
  shrink it); investigated nodeset-pool / prefetch follow-ups were **not** shipped
  because, with no remaining slower-than-Nokogiri row, they'd add lifetime /
  speculative complexity for no measurable win.

Working on perf: capture a `rake bench` baseline, change one thing, re-bench,
and **ship only a measurable win** that keeps `rake spec` + `rake fuzz:sanitize`
green. (Note: a per-node Ruby wrapper cache was considered and rejected —
`node->user` is taken, and a node→VALUE side table creates a GC-lifetime problem
for no clear gain on already-winning paths.)
