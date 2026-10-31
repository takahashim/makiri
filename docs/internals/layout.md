# Source layout and features

```
lib/makiri/                Ruby API (Document, Node, Element, NodeSet, XPathContext, ...)
ext/makiri/rust/           the extension: package makiri_rs, lib `makiri`, and the
                           root of a workspace with `engine/`, `falloc/` and
                           `nupur/` (`reference/`, `fuzz/` and `lints/` are
                           crates of their own, each `[workspace]`, so this
                           workspace's Cargo.lock - the one a source install
                           builds from - holds nothing of Lexbor's, and its build
                           scripts read no Lexbor header)
  extconf.rb               hands the crate to cargo via create_rust_makefile; owns
                           the link arguments and the export trim (it builds no
                           Lexbor: `rake lexbor` does, for the tests). It builds
                           `--crate-type cdylib` alone: with an `rlib` in, rustc
                           runs NO LTO, and every call into `nupur` and the
                           engine is an opaque cross-crate call (the light
                           readers measured 9-25% slower when nupur was split;
                           with LTO they are 12-14% faster than the one-crate
                           build, which never had LTO either). `rake no_unwind`
                           builds the same way, since its proof is of the
                           shipped binary
  nupur/                   the Ruby binding layer, a crate of its own over rb-sys.
                           `nupur/src/lib.rs` is the one statement of what it
                           guarantees, what an extension must uphold and what
                           checks each (its module list is there too); read it
                           before touching the boundary. What matters here:
                           it cannot name the extension (no dependency), which
                           names it `crate::nupur` and registers what it needs
                           at Init_ (`INTERNAL_ERROR`, `inject::install`);
                           `bridge` re-exports its public modules, and `raw`
                           is private to it; a function on a no_unwind site's
                           path is `#[inline]`, or the proof cannot see into
                           it across the crate. `test/` is its own extension
                           and specs (`rake nupur:spec`)
  build.rs                 the gem's version, embedded for `Makiri::NATIVE_VERSION`
  falloc/                  fallible allocation (crate `falloc`) - every engine
                           allocation goes through here, so `rake oom` can fail
                           it and OOM raises rather than aborting the host
                           process (the glue's Ruby-side storage is Ruby's
                           xmalloc; see `engine_rules.md`). Its `try_box` holds the
                           ONE `unsafe` block of ours outside nupur (a fallible
                           `Box<T>` has no stable safe spelling), which is why
                           it is a crate of its own; its test build includes
                           `nupur/src/utf8_copy.rs` for Miri
  engine/                  the engine (crate `makiri_engine`): no Ruby, no
                           Lexbor - it depends on `falloc`, `encoding_rs` and
                           `chardetng` alone, so a Ruby name there does not
                           compile. `forbid(unsafe_code)` at its root (bar the
                           `rake no_unwind` check build). Features: `html-dom`
                           (default), `alloc-inject`, `lexbor-compat` (the
                           Lexbor reference's switches and harnesses). The
                           extension and the reference re-export its modules
                           at their roots, so `crate::xml` there is its
    src/doc_cell.rs        `DocCell`: a Document's content behind counted
                           read/write guards (`bridge::dom` wraps it; here so
                           its compile_fail doctests run without a Ruby)
    src/ceiling_buf.rs     `Buf`: the owned, growable byte buffer with a
                           content ceiling (`cbuf` while it was C's libc memory)
    src/utf8_decode.rs     the one UTF-8 validator + strict 1-codepoint decoder
                           (`cutf8`, the port of the C one)
    src/utf8_input.rs      HTML input sanitiser: invalid UTF-8 -> U+FFFD, per
                           WHATWG; `ceiling_buf`+`utf8_decode` only
    src/no_unwind.rs       the engine's no_unwind sites and the check's control
    src/xpath/, xml/, html/, css_match/, selectors/ - below, after the extension's
  reference/               the Lexbor reference (crate `makiri_lexbor`, its own
                           workspace): `build.rs` (bindgen over Lexbor's own
                           headers -> the layout and constants it reads, never
                           transcribed by hand), `src/lexbor/` (below), `gvl.rs`
                           and `caught.rs`. Depends on the engine with
                           `lexbor-compat`; nothing that ships depends on it
  src/                     the extension (`makiri_rs`): the Ruby surface alone
    init.rs                Init_makiri: the class hierarchy and the registration seam
    bridge/                the Ruby boundary - the ONLY layer allowed raw Ruby String
                           access (RSTRING) and verified-string minting. `rb/` is
                           its value and method layer over rb-sys (`Value` and the
                           class-checked handles, `Error`, the `method!` /
                           `function!` trampolines, class definitions), which
                           replaced magnus under its names. The C calls that
                           can raise are nupur's own (`nupur/src/raw/`,
                           private to it), as is the wrap-then-store
                           constructor; a raise becomes `Err` there. `rake unsafe:boundaries` pins that boundary:
                           the crate denies `unsafe_code` (`lib.rs`), so every
                           file that needs it carries an `allow` and the script
                           holds each one's count exactly - a new file fails even
                           where a parent module's `allow` kept rustc quiet - plus
                           every `forbid` file, which it counts. `glue/**`, `xpath/**`
                           and `css_match/**` are fully safe
                           (their module roots carry
                           `#![forbid(unsafe_code)]`, which the gate pins);
                           `rake dylint` holds `rb_sys::` to `bridge/` and the
                           C calls that can raise to `nupur`'s `raw/`, and
                           `Value::from_raw` is `pub(in crate::bridge)`. A
                           borrowed HTML handle made a lifetime-free `RawNode`
                           in `bridge/` is `raw_node_crossing`, each crossing
                           an allow with its reason (`with_html_node` /
                           `HtmlParsed::mint_key` / `resolve` are the checked
                           entries to reach for instead)
    glue/                  THE Ruby surface: every method Makiri defines, with
                           its argument reading, its errors and its
                           registration, one module per feature
                           (query/html_doc/xml_doc/node_set/xpath_context/
                           html_node/xml_node, each node module owning its
                           read/mutate/css/serialize). All `unsafe`-free; the
                           wrappers, TypedData and the operations that keep a
                           structure's invariants are `bridge/`'s, and
                           `rake dylint`'s `ruby_surface_in_bridge` fails on a
                           method defined - or an argument list split - in
                           `bridge/`
  engine/src/
    xpath/                 native XPath 1.0 engine, generic over a `Dom` trait;
                           `#![forbid(unsafe_code)]` and Lexbor/Ruby-free. The
                           two `Dom` instances live with their layers:
                           `html/dom/xpath.rs` (HTML) and `xml/xpath.rs` (XML),
                           joined for the glue by `Cx` in
                           `bridge/xpath/mod.rs`
    xml/                   native XML reader (Ruby/Lexbor-free; own arena), plus
                           its XPath `Dom` instance. One job per module:
                           `model`+`arena` (the index arena), `tree/` (the
                           builder, with `tree/cursor.rs` - the shared input
                           cursor - and `tree/dtd.rs`, the subset validator that
                           holds a cursor and so CANNOT reach the tree),
                           `chars/` (character classes + `chars/expand.rs`, the
                           reference-expansion engine), `qname` (XML naming) vs
                           `dom_name` (the WHATWG DOM's looser names, which are
                           NOT XML naming), `mutate`, `index`, `encoding_sniff`,
                           and `serialize/` (`out` = buffer + one escape table,
                           `xml` = plans a prefix per name, `c14n` = renders the
                           document's own declarations, adding a name's own
                           prefix where its namespace was set, never inventing one)
    html/                  the HTML parser ported from Lexbor and the DOM it builds
                           (Ruby/Lexbor-free): `tokenizer/`, `tree/` (the tree
                           builder, its guard, stamps and the document-bytes
                           parse), `dom/` (the index arena: handles, edits,
                           copies, indexes, serialization, CSS and XPath
                           instances), `encoding/` (sniffing, decoders),
                           `fragment.rs`, and `res/` (tables generated from
                           Lexbor's)
    css_match/             the CSS matcher, over any DOM that implements its
                           traits (`css_match::dom`: the HTML arena's in
                           `html/dom/css.rs`, the XML arena's in `xml/css.rs`);
                           `#![forbid(unsafe_code)]`, Ruby- and Lexbor-free,
                           and naming no host. The compiled-selector cache in
                           front of it, which names both, is `css_cache.rs`
    selectors/             the CSS parser ported from Lexbor (its CSS syntax
                           tokenizer, rule/state stacks, selector states, and
                           the stylesheet rule phases and callbacks in
                           `parser/syntax.rs`) into an owned AST and rule arena
                           (`rules.rs`), plus `parse_stylesheet`'s reader
                           (`stylesheet.rs`) and the CSSOM selector writer
                           (`text.rs`); Ruby- and Lexbor-free,
                           `#![forbid(unsafe_code)]` at its root
  reference/src/
    lexbor/                the Lexbor boundary: `abi` - the generated layout and
                           functions (the `_noi` twins included), the ONE place
                           a Lexbor function is declared (a second `extern "C"`
                           spelling is a second Rust type for the symbol).
                           Only the four
                           exports no header declares are written by hand, and
                           build.rs's `UNDECLARED_EXPORTS` fails the build if
                           their C definitions change - `adapter`, the one reader of
                           Lexbor's DOM structs, plus the element index,
                           text index, source location and post-parse - and the
                           selectors/serialize/fragment facades, the views over
                           Lexbor's CSS selector lists (`css_parser.rs`; its
                           parse is a reference) and the XPath HTML backend
                           (`xpath.rs`). All of it is the reference the ports
                           are tested against: nothing here ships, and the
                           build that ships cannot name it (`lexbor` is not
                           a feature of it)
  verus/                   Verus proofs, each over code copied from the source (lines
                           only Verus needs end in `// [verus]`, and `rake verus:sync`
                           holds the rest to the source token for token):
                           `ptr_table.rs` for `engine/src/ptr_table.rs`: `probe`
                           always stops (at the key or an empty slot), every
                           `PtrTable` / `PtrMap` operation, `grow` included, keeps
                           a slot free - the load factor - so it always can, and
                           each table is the `Map` it stands for (`view`): `get`
                           finds exactly what was put, since `wf` keeps keys
                           `unique` and probe paths `clustered`;
                           `ceiling_buf.rs` for `engine/src/ceiling_buf.rs`: a `Buf` holds exactly
                           what was appended, its room never passes the ceiling,
                           a failure changes nothing, and `copy_small` copies
                           exactly - at every length, where Kani ran out of memory -
                           with falloc's `grow_capacity`; `limits.rs`: `admit`, the
                           one rule every work budget charges through
  fuzz/                    cargo-fuzz harnesses, over the reference crate (as
                           `makiri`) and so the engine (xml/html, xpath/xml_xpath/
                           html_xpath, css = the XML matcher's entry points held
                           to each other, html_css = the
                           HTML matcher over an arbitrary document, asserting
                           css and matches? agree, html_css_diff = the same
                           matcher against Lexbor's own engine, html_diff =
                           the ported tree builder against Lexbor's tree,
                           documents and fragments - with their
                           serializations and the depth guard's verdicts -
                           selector_diff /
                           stylesheet_diff = the ported selector / stylesheet
                           parser against Lexbor's; built on
                           PRs, run nightly. css-match.yml owns the two
                           html_css*)
vendor/lexbor/             git submodule, pinned 05b5d37 (v3.0.0-66), NEVER patched
spec/fuzz/                 grammar-aware robustness fuzzer
spec/invariants/           randomized property checks (see its README)
spec/differential/         the recorded C-build answers + the probes (see `rake diff`)
bench/                     Nokogiri-comparison benchmark
docs/design_doc.ja.md      authoritative design (read this)
```

Features are per crate. The **engine** has **`html-dom`** (default: the
parser port's index-arena DOM, `html::dom`, behind the `html_dom` facade and
parsed by the ported tree builder `html::tree`, with what sits over it - the
HTML CSS matcher, XPath's HTML instance; only the bare engine - Kani - is
without it), **`alloc-inject`** (the `rake oom` hook, through to `falloc`'s)
and **`lexbor-compat`** (`compat::as_lexbor`, the conformance harness and the
tokenizer's traced run - what only the Lexbor reference reads; never in a build
that ships). The **extension** has **`alloc-inject`** alone (to the engine,
`falloc` and nupur). The **reference** has **`css-reference`** (the OLD CSS
engine, `lexbor::selectors`, for the `html_css_diff` fuzz target) and
**`lexbor-reference`** (`lexbor::tree_diff`, `lexbor::selector_diff` and
`lexbor::stylesheet_diff`, Lexbor's tree, selector lists and stylesheets beside
the ports', for the `html_diff`, `selector_diff` and `stylesheet_diff` fuzz
targets). There is no `ruby` feature (the extension is the Ruby surface, and
the engine has no Ruby to turn off) and no `lexbor` one (the reference crate
is Lexbor, and nothing that ships depends on it, so `nm` finds no `lxb_`
symbol). (The CSS-to-XPath lowering XML's CSS ran on before `css_match` stayed
as the XML matcher's reference, behind `xml-css-reference`, until the browser
comparison made it redundant.) `rake rust:test` runs the engine bare and as
shipped, `falloc`, nupur and the reference. Lexbor's DOM is not an
extension build any more
(`MAKIRI_LEXBOR_DOM=1` was retired in stage 9; the tests that held the
arena's edits and XPath to Lexbor's DOM retired with the migration):
`lexbor::tree_port_tests` and the `html_diff` fuzz target hold the tree
builder - and its serializations - to Lexbor's tree node for node, and the
matcher is held to Lexbor's own engine over an arena copy of
Lexbor's parse, node for node by walk position (`lexbor::twin`). The arena's
byte store is append-only between compactions: an edit's start
(`HtmlParsed::edit`, after the indexes - which hold spans - are dropped)
rewrites it once the dead bytes outweigh the live ones
(`html/dom/compact.rs`), so repeated `content=` does not grow a document.
The fuzz crate builds the reference with `css-reference,lexbor-reference`,
and Kani builds the engine with `--no-default-features`. The ~30 features that
used to stand here were migration scaffolding, one per ported C file, and went
with the C.

