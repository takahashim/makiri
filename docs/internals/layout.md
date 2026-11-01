# Source layout and features

The tree annotates each path. The reasoning that is not about one path - why
the crates are split as they are, what each gate holds, the features - is in
the sections after it, which the entries name.

```
lib/makiri/                Ruby API (Document, Node, Element, NodeSet,
                           XPathContext, ...)
ext/makiri/rust/           the extension (package makiri_rs, lib `makiri`)
                           and the root of a workspace with `engine/`,
                           `falloc/`, `nupur/` and `nupur/sys/`.
                           `reference/`, `fuzz/`, `lints/` and
                           `nupur/test/ext` (`nupur_test`) are crates of
                           their own, each its own `[workspace]` (see
                           "Workspaces")
  extconf.rb               hands the crate to cargo via create_rust_makefile;
                           owns the link arguments and the export trim. It
                           builds no Lexbor (`rake lexbor` does, for the
                           tests), and builds `--crate-type cdylib` alone
                           (see "cdylib alone")
  build.rs                 the gem's version, embedded for
                           `Makiri::NATIVE_VERSION`
  src/                     the extension (`makiri_rs`): the Ruby surface alone
    init.rs                Init_makiri: the class hierarchy and the
                           registration seam
    bridge/                the Ruby boundary, `#![forbid(unsafe_code)]`: it
                           re-exports nupur's `rb` (`Value` and the
                           class-checked handles, `Error`, the `method!` /
                           `function!` trampolines, class definitions) and
                           mints the checked string views over nupur's
                           String reads; raw String access is nupur's (see
                           "The bridge and the glue")
    glue/                  THE Ruby surface: every method Makiri defines, with
                           its argument reading, its errors and its
                           registration, one module per feature
                           (query/css/stylesheet/html_doc/xml_doc/node_set/
                           xpath_context/html_node/xml_node, with node for
                           the methods both node kinds share and
                           namespaces/dom_names/kwargs for argument reading;
                           each node module owns its read/mutate/css/
                           serialize). All `unsafe`-free
  nupur/                   the Ruby binding layer, a crate of its own over
                           rb-sys (through `sys/`). `nupur/src/lib.rs` is
                           the one statement of what it guarantees, what an
                           extension must uphold and what checks each, and
                           holds its module list; read it before touching
                           the boundary. `test/` is its own extension and
                           specs (`rake nupur:spec`). See "nupur and the
                           extension"
    sys/                   Ruby's C API for nupur (crate `nupur_sys`), the
                           one crate that depends on rb-sys. It holds the
                           protection (`protect`: the `rb_protect` thunk,
                           `$!`, a pending exit), the raising C calls, each
                           taking the protection's token (`raising`), and
                           `no_unwind`; for the rest it re-exports rb-sys
                           whole, and nupur reaches every rb-sys item
                           through it
  falloc/                  fallible allocation (crate `falloc`): every engine
                           allocation goes through here, so `rake oom` can
                           fail it and OOM raises rather than aborting the
                           host process. Holds one of the few `unsafe`
                           blocks of ours outside nupur (see "falloc")
  engine/                  the engine (crate `makiri_engine`): no Ruby, no
                           Lexbor (see "The engine boundary"; its features
                           are under "Features")
    src/doc_cell.rs        `DocCell`: a Document's content behind counted
                           read/write guards. `bridge::dom` wraps it; it lives
                           here so its compile_fail doctests run without a
                           Ruby
    src/ceiling_buf.rs     `Buf`: the owned, growable byte buffer with a
                           content ceiling (`cbuf` while it was C's libc
                           memory)
    src/utf8_decode.rs     the one UTF-8 validator + strict 1-codepoint
                           decoder (`cutf8`, the port of the C one)
    src/utf8_input.rs      HTML input sanitiser: invalid UTF-8 -> U+FFFD, per
                           WHATWG; uses `ceiling_buf` + `utf8_decode` only
    src/swar.rs            word-at-a-time byte tests (`word`, `has_zero`),
                           for the tokenizer's `seek4` and the line table
    src/no_unwind.rs       the engine's no_unwind sites and the check's
                           control
    src/xpath/             native XPath 1.0 engine, generic over a `Dom`
                           trait; `#![forbid(unsafe_code)]` and
                           Lexbor/Ruby-free. The two `Dom` instances live
                           with their layers: `html/dom/xpath.rs` (HTML) and
                           `xml/xpath.rs` (XML), joined for the glue by `Cx`
                           in `bridge/xpath/mod.rs`
    src/xml/               native XML reader (Ruby/Lexbor-free; own arena)
                           and its XPath `Dom` instance. One job per module:
      model, arena         the index arena
      tree/                the builder. `tree/cursor.rs` is the shared input
                           cursor; `tree/dtd.rs`, the subset validator, holds
                           a cursor and so CANNOT reach the tree
      chars/               character classes; `chars/expand.rs` is the
                           reference-expansion engine
      qname, dom_name      XML naming vs the WHATWG DOM's looser names, which
                           are NOT XML naming
      mutate, index,
      encoding_sniff
      serialize/           `out` = buffer + one escape table; `xml` plans a
                           prefix per name; `c14n` renders the document's own
                           declarations, adding a name's own prefix where its
                           namespace was set, never inventing one
    src/html/              the HTML parser ported from Lexbor and the DOM it
                           builds (Ruby/Lexbor-free):
      tokenizer/
      tree/                the tree builder, its guard, stamps and the
                           document-bytes parse
      dom/                 the index arena: handles, edits, copies, indexes,
                           serialization, CSS and XPath instances
      encoding/            sniffing, decoders
      fragment.rs
      res/                 tables generated from Lexbor's
    src/css_match/         the CSS matcher, over any DOM that implements its
                           traits (`css_match::dom`: the HTML arena's in
                           `html/dom/css.rs`, the XML arena's in
                           `xml/css.rs`); `#![forbid(unsafe_code)]`, Ruby-
                           and Lexbor-free, and naming no host
    src/css_cache.rs       the compiled-selector cache in front of the
                           matcher, which names both hosts
    src/selectors/         the CSS parser ported from Lexbor - its CSS syntax
                           tokenizer, rule/state stacks, selector states, and
                           the stylesheet rule phases and callbacks in
                           `parser/syntax.rs` - into an owned AST and rule
                           arena (`rules.rs`); plus `parse_stylesheet`'s
                           reader (`stylesheet.rs`) and the CSSOM selector
                           writer (`text.rs`). Ruby- and Lexbor-free,
                           `#![forbid(unsafe_code)]` at its root
  reference/               the Lexbor reference (crate `makiri_lexbor`, its
                           own workspace). Depends on the engine with
                           `lexbor-compat`; nothing that ships depends on it
    build.rs               bindgen over Lexbor's own headers -> the layout
                           and constants it reads, never transcribed by hand
    src/gvl.rs, caught.rs
    src/lexbor/            the Lexbor boundary. All of it is the reference
                           the ports are tested against: nothing here ships,
                           and the build that ships cannot name it (`lexbor`
                           is not a feature of it)
      abi                  the generated layout and functions (the `_noi`
                           twins included): the ONE place a Lexbor function
                           is declared, since a second `extern "C"` spelling
                           is a second Rust type for the symbol. Only the
                           four exports no header declares are written by
                           hand, and build.rs's `UNDECLARED_EXPORTS` fails
                           the build if their C definitions change
      adapter              the one reader of Lexbor's DOM structs (`html`),
                           plus the arena's size (`arena_bytes`), the copy
                           into the port's arena (`arena_copy`), source
                           location, tree guard and post-parse
      selectors,           the facades
      serialize,
      fragment
      css_parser.rs        the views over Lexbor's CSS selector lists; its
                           parse is a reference
  verus/                   Verus proofs, each over code copied from the
                           source. Lines only Verus needs end in
                           `// [verus]`; `rake verus:sync` holds the rest to
                           the source token for token
    ptr_table.rs           for `engine/src/ptr_table.rs`: `probe` always
                           stops (at the key or an empty slot); every
                           `PtrTable` / `PtrMap` operation, `grow` included,
                           keeps a slot free - the load factor - so it always
                           can; and each table is the `Map` it stands for
                           (`view`): `get` finds exactly what was put, since
                           `wf` keeps keys `unique` and probe paths
                           `clustered`
    ceiling_buf.rs         for `engine/src/ceiling_buf.rs`, with falloc's
                           `grow_capacity`: a `Buf` holds exactly what was
                           appended, its room never passes the ceiling, a
                           failure changes nothing, and `copy_small` copies
                           exactly - at every length, where Kani ran out of
                           memory
    limits.rs              `admit`, the one rule every work budget charges
                           through
    utf8.rs                for `engine/src/utf8_decode.rs` and
                           `xml/chars/mod.rs`: `decode1` answers exactly
                           vstd's well-formed first scalar (`vstd::utf8`),
                           the chain of decodes accepts exactly `valid_utf8`,
                           and `validate_chars` only valid UTF-8 - at every
                           length, where Kani stopped at 8 bytes
    xpath_number.rs        `xpath::number::extent`: the longest prefix that
                           is an XPath Number, 0 when none is - at every
                           length, where Kani stopped at 6 bytes
    xml_chars.rs           `xml::chars`' classes: NameStartChar is in
                           NameChar, and NameChar in Char, for every `u32`
    swar.rs                the tokenizer's `seek4` over `engine/src/swar.rs`:
                           it skips no byte it looks for and never passes
                           `end`, whatever the word
    source_lines.rs        `lines_build`: the line table is exactly 0 and
                           the offset past each newline, for every input
    int.rs                 `nupur::int` at both widths of a C `long`: the
                           Fixnum range exactly, the Fixnum's bits, and a
                           length and a capacity that never truncate
    xml_expand.rs          `xml::chars::expand`: `scan_char_ref` answers the
                           digits' exact value or refuses (never wraps), and
                           `expand_into` writes no more than it reads, so
                           never `Overflow`
  fuzz/                    cargo-fuzz harnesses, over the reference crate (as
                           `makiri`) and so the engine. Built on PRs, run
                           nightly; css-match.yml runs the two html_css*
                           (libfuzzer.yml's slow-inputs job runs html_css
                           too)
    xml, html
    xpath, xml_xpath,
    html_xpath
    css                    the XML matcher's entry points held to each other
    html_css               the HTML matcher over an arbitrary document,
                           asserting css and matches? agree
    html_css_diff          the same matcher against Lexbor's own engine
    html_diff              the ported tree builder against Lexbor's tree:
                           documents and fragments, with their
                           serializations and the depth guard's verdicts
    selector_diff,         the ported selector / stylesheet parser against
    stylesheet_diff        Lexbor's
vendor/lexbor/             git submodule, pinned 05b5d37 (v3.0.0-66), NEVER
                           patched
spec/fuzz/                 grammar-aware robustness fuzzer
spec/invariants/           randomized property checks (see its README)
spec/differential/         the recorded C-build answers + the probes (see
                           `rake diff`)
bench/                     Nokogiri-comparison benchmark
docs/design_doc.ja.md      authoritative design (read this)
```

## Workspaces

`reference/`, `fuzz/`, `lints/` and `nupur/test/ext` each declare their own
`[workspace]`, so the extension workspace's Cargo.lock - the one a source
install builds from - holds nothing of Lexbor's, and its build scripts read
no Lexbor header.

## cdylib alone

`extconf.rb` builds `--crate-type cdylib` alone because with an `rlib` in,
rustc runs NO LTO, and every call into `nupur` and the engine is then an
opaque cross-crate call. The light readers measured 9-25% slower when Nupur
was split; with LTO they are 12-14% faster than the one-crate build, which
never had LTO either. `rake no_unwind` builds the same way, since its proof is
of the shipped binary.

## Nupur and the extension

- Nupur cannot name the extension: it has no dependency on it. The extension
  names it `crate::nupur` and registers what Nupur needs from it at Init_
  (`INTERNAL_ERROR`, `inject::install`).
- `bridge` re-exports Nupur's `rb`, `typed`, `gvl`, `build`, `hash` and
  `args`; its other public modules are reached as `crate::nupur::...`, and
  `raw` is private to Nupur.
- A function on a no_unwind site's path is `#[inline]`, or the proof cannot
  see into it across the crate.

## falloc

The glue's Ruby-side storage is Ruby's xmalloc, not falloc (see
`engine_rules.md`). falloc's `try_box` holds an `unsafe` block, because a
fallible `Box<T>` has no stable safe spelling; that is why falloc is a crate
of its own. Outside Nupur the only others are `Init_makiri`'s call to
`run_init` (`src/init.rs`) and the `rake no_unwind` check build's sites
(`engine/src/no_unwind.rs`); `rake unsafe:boundaries` counts each. Its test
build includes `nupur/src/utf8_copy.rs` for Miri.

## The engine boundary

The engine depends on `falloc`, `encoding_rs` and `chardetng` alone, so a
Ruby name in it does not compile. It is `forbid(unsafe_code)` at its root,
except in the `rake no_unwind` check build. The extension and the reference
re-export its modules at their roots, so `crate::xml` in either is the
engine's.

## The bridge and the glue

`glue/` defines every method; the wrappers, TypedData and the operations that
keep a structure's invariants are `bridge/`'s. `rake dylint`'s
`ruby_surface_in_bridge` fails on a method defined - or an argument list
split - in `bridge/`.

The C calls that can raise are Nupur's own (`nupur/src/raw/`, private to it),
as is the wrap-then-store constructor; a raise becomes `Err` there.

These gates hold the boundary:

- `rake unsafe:boundaries`. The crate denies `unsafe_code` (`lib.rs`), so
  every file that needs it carries an `allow`, and the script holds each
  one's count exactly: a new file fails even where a parent module's `allow`
  kept rustc quiet. It also counts every `forbid` file. `rake unsafe:fix`
  rewrites both records from the tree, but not the `static mut` table.
- `bridge/**` and `glue/**` are fully safe: their module roots carry
  `#![forbid(unsafe_code)]`, as the engine's root does for the whole engine.
  The gate pins the roots in `FORBID_ROOTS` (`bridge`, `glue` and the
  engine's `css_match`, `selectors`, `xml` and `xpath`).
- rb-sys is Nupur's, reached through `nupur_sys`: the extension depends on
  neither, so no file of it names Ruby's C API, and `rake dylint`'s
  `raw_ruby_outside_bridge` refuses an rb-sys item outside Nupur (and
  `bridge/`, which cannot reach one). The lint resolves the item to where
  rb-sys defines it, so the re-export does not hide it. `rake dylint` also
  holds the C calls that can raise to `nupur`'s `raw/` and to
  `nupur_sys::protect`.
- `Value::from_raw` is Nupur's, `pub(crate)` and `unsafe`, so the extension
  cannot make a `Value` from a raw `VALUE`.
- Making a borrowed HTML handle into a lifetime-free `RawNode` in `bridge/` is
  a `raw_node_crossing`, and each crossing is an allow with its reason.
  `with_html_node` / `HtmlParsed::mint_key` / `resolve` are the checked
  entries to reach for instead.

## Features

Features are per crate.

- **Engine**
  - **`html-dom`** (default): the parser port's index-arena DOM, `html::dom`,
    behind the `html_dom` facade and parsed by the ported tree builder
    `html::tree`, with what sits over it - the HTML CSS matcher and XPath's
    HTML instance. Only the bare engine is without it: `rake rust:test` and
    `rake lint` build the engine with `--no-default-features` too.
  - **`alloc-inject`**: the `rake oom` hook, through to `falloc`'s.
  - **`lexbor-compat`**: `compat::as_lexbor`, the conformance harness and the
    tokenizer's traced run - what only the Lexbor reference reads. Never in a
    build that ships.
- **Extension**: **`alloc-inject`** alone, passed to the engine, `falloc`
  and Nupur.
- **Reference**
  - **`css-reference`**: the OLD CSS engine, `lexbor::selectors`, for the
    `html_css_diff` fuzz target.
  - **`lexbor-reference`**: `lexbor::tree_diff`, `lexbor::selector_diff` and
    `lexbor::stylesheet_diff` - Lexbor's tree, selector lists and
    stylesheets beside the ports' - for the `html_diff`, `selector_diff` and
    `stylesheet_diff` fuzz targets.

The fuzz crate builds the reference with `css-reference,lexbor-reference`.
`rake rust:test` runs the engine bare and as shipped, `falloc`, Nupur and the
reference.

There is no `ruby` feature: the extension is the Ruby surface, and the engine
has no Ruby to turn off. There is no `lexbor` feature either: the reference
crate is Lexbor, and nothing that ships depends on it, so `nm` finds no
`lxb_` symbol.

The ~30 features that used to stand here were migration scaffolding, one per
ported C file, and went with the C. The XML matcher has no reference
feature: the browser comparison (`rake conformance:css_browsers`) holds it.

## The retired Lexbor DOM

Lexbor's DOM is not an extension build any more: `MAKIRI_LEXBOR_DOM=1` was
retired in stage 9, and the tests that held the arena's edits and XPath to
Lexbor's DOM retired with the migration. What holds the port to Lexbor now:

- `lexbor::tree_port_tests` and the `html_diff` fuzz target hold the tree
  builder - and its serializations - to Lexbor's tree node for node.
- The matcher is held to Lexbor's own engine over an arena copy of Lexbor's
  parse, node for node by walk position (`lexbor::twin`).

## Byte-store compaction

The arena's byte store is append-only between compactions. An edit's start
(`HtmlParsed::edit`, after the indexes - which hold spans - are dropped)
rewrites it once the dead bytes outweigh the live ones
(`html/dom/compact.rs`), so repeated `content=` does not grow a document.
