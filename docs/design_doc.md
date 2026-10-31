# Makiri Design Document

This is the English version of the design document. The Japanese version,
`docs/design_doc.ja.md`, is authoritative; where the two differ, it wins.

## 1. Basic policy

### Goals

* Provide a Nokogiri-compatible API for the parts of HTML5 scraping people
  actually use.
* Parse HTML5 quickly with a parser that follows the WHATWG HTML Standard.
  The parser is Makiri's own: a Rust port of Lexbor's tokenizer and tree
  builder, building an index-arena DOM (nodes are indexes into a table).
  The code above it (the bridge, XPath, CSS matching) reads the DOM through
  a facade, `crate::html_dom`. The vendored Lexbor is not part of the
  extension; it is built only as the reference the tests hold the port to
  (`rake lexbor`). The port's plan and progress are in
  `docs/rewrite_parser.ja.md`.
* Implement XPath 1.0 with an engine of Makiri's own, following the
  specification, with nothing from libxml2.
* Parse CSS selectors and stylesheets with a Rust port of Lexbor's CSS
  syntax tokenizer, syntax parser and selector parser; the shipping build
  runs no Lexbor CSS parser. Matching is done by `css_match`, a matcher of
  Makiri's own that ports Lexbor's state machine to safe Rust and handles
  HTML and XML through the same traits (`css_match::dom`). The matcher keeps
  selector nesting off the native stack, checks the whole selector before
  matching any node, and makes every allocation fallible. Its answers follow
  a browser's `querySelectorAll`, switching rules on whether the document is
  HTML and on the element's namespace. The earlier lowering of XML's CSS to
  XPath was removed once XML matching was checked against browsers.
* Report source locations for HTML elements: the line of the start tag,
  1-based. A node whose line cannot be determined (an element the parser
  inserted, a node in a fragment) returns nil rather than a guess. Columns
  and byte offsets are not exposed.
* Support DOM editing: adding, removing and replacing nodes, rewriting
  attributes and content, and `inner_html=`.
* Make XPath evaluation resistant to memory exhaustion and DoS. String sizes
  are capped, trees are walked iteratively rather than recursively, and the
  remaining native stack is checked. A limit raises instead of returning a
  truncated result (fail-closed).
* Bound parsing too. HTML parsing limits the tree depth (`max_tree_depth:`,
  default 400) and the size of the document relative to its input
  (`max_amplification:`, default 256 times the input and never less than
  64 MiB), and refuses a document past either with `Makiri::Error`. An XML
  document has an arena byte budget (`max_bytes:`, default 256 MiB) that
  counts over its whole life, edits included.
* Treat input strings as UTF-8 (§3). HTML parsing replaces invalid bytes
  with U+FFFD, as browsers do, and always succeeds. Strings passed to the
  programmatic APIs (XPath, CSS, DOM operations) must be valid UTF-8, or the
  call raises `Makiri::Error`. NUL (U+0000) is refused in names and
  expressions, and kept in text, comments and attribute values, as the DOM
  keeps it.
* Thread safety: parsing releases the GVL and scales across threads. XPath
  evaluation holds the GVL, which rules out memory corruption of a shared
  document or context by construction.
* An internal invariant violation must not bring down the host process.
  Every method exposed to Ruby turns a violation inside it into a dedicated
  exception that can be rescued, whether or not the input was untrusted.
  Outside a method (during initialization, say), the cleanup runs (`ensure`,
  `at_exit`, resource release) and only the current thread ends. Nothing can
  be raised in code the GC calls, so that code is kept trivial: the mark
  callback is proven by the compiler not to unwind, and the free and memsize
  callbacks catch a panic and carry on, leaking the payload or answering 0.
* Test with robustness fuzzing and hand-curated differential specs. The
  parser port is checked by differential fuzzing against its reference,
  Lexbor; by conformance data from fixed versions of the specifications
  (WPT's tree-construction tests, html5lib's tokenizer tests); by browsers;
  and by real web pages.

### Out of scope

* Full XML parsing: XML 1.0 only, not 1.1. The internal DTD subset is checked
  for well-formedness, and its declarations are not applied. A document
  whose tree would change if they were applied (default attribute values,
  expansion of declared entities, and so on) is an error rather than being
  silently misread.
* XSLT, DTD / Schema / RelaxNG validation, XPointer and XInclude.
* Streaming parsing: documents are parsed whole.
* SAX or pull-parser APIs.
* Full compatibility with libxml2's API; the long tail of corner cases is
  not followed.
* Drop-in compatibility with every Nokogiri method.

### Deliberately avoided

* Forking Lexbor. Lexbor's bugs and gaps are handled in the port and reported
  upstream to be fixed there. The port keeps Lexbor's semantics and control
  flow. A behaviour change made to follow the Standard goes in as a change of
  its own, separate from changes made for ownership or safety. The known
  differences and their classification are recorded in
  `docs/parser_differences.ja.md`.
* Selling libxml2 compatibility. Where libxml2 and the HTML5 or XPath
  specifications disagree, Makiri follows the specification and documents the
  difference. The exception is XPath's number-to-string conversion
  (`string()`), which writes numbers as libxml2 does (exponent notation,
  15 significant digits) rather than by XPath 1.0 §4.2, because callers
  moving from Nokogiri compare the strings.

## 2. Ruby API surface

Top level:

```ruby
Makiri.HTML(source)   # → Makiri::HTML::Document
Makiri::HTML(source)  # alias
Makiri.parse(source)  # alias
Makiri.XML(source)    # → Makiri::XML::Document
Makiri::XML(source)   # alias
```

Class hierarchy:

```
Makiri::Node
├── Makiri::Document          (root)
├── Makiri::DocumentFragment
├── Makiri::Element
├── Makiri::Attr
├── Makiri::Text
├── Makiri::CDATASection
├── Makiri::Comment
├── Makiri::ProcessingInstruction
└── Makiri::DocumentType        (#public_id / #system_id / #external_id)

Makiri::NodeSet                (Enumerable)
Makiri::XPathContext
Makiri::XML::Builder           (builds an XML document from a block)
```

Node classes are named after the WHATWG DOM interfaces, and Nokogiri's names
(`CDATA`, `DTD`) are aliases. The objects created are of per-representation
leaf classes, `Makiri::HTML::*` and `Makiri::XML::*`, each inheriting from
the class above. Methods HTML and XML share live on the base classes; only
behaviour specific to one representation lives in the leaves.

Exceptions:

```
Makiri::Error                        (base / StandardError)
├── Makiri::XPath::SyntaxError
│   └── Makiri::XPath::LimitExceeded (per-evaluate budget exceeded)
├── Makiri::CSS::SyntaxError
├── Makiri::XML::SyntaxError
└── Makiri::XML::LimitExceeded
Makiri::InternalError                (directly under Exception)
```

Every error caused by input is under `Makiri::Error`, so `rescue
Makiri::Error` catches them all. An exceeded XPath budget is under
`XPath::SyntaxError` so that one class catches every error where the
expression could not be evaluated. `InternalError` alone is not under
`StandardError`: a broken internal invariant is not a bad argument, so
`rescue => e` should not catch it together with input errors. A host that
rescues it by name can still turn just that request into a 500.

Node methods (covering about 90% of scraping idioms):

```
#name #node_name  #attributes #to_h
#[] #[]= #delete                   #key? #keys #values
#attr #get_attribute #set_attribute #attribute #has_attribute? #remove_attribute
#node_type #element? #elem? #text? #comment? #document? #fragment? #blank? ...
#classes #add_class #append_class #remove_class  (class attribute helpers)
#parent #children #element_children #traverse
#next #previous #next_element #previous_element
#first_element_child #last_element_child
#root #document #ancestors         #path
#xpath #at_xpath                   #css #at_css #matches?
#search #at                        (CSS or XPath, detected)
#text #inner_text #content #content=
#inner_html #inner_html= #outer_html #outer_html= #to_html  (HTML only)
#to_s                              (XML also has #to_xml / #canonicalize)
#add_child #<< #add_previous_sibling #before
#add_next_sibling #after
#remove #unlink                    #replace
#parse                             (fragment parse in an element's context, HTML only)
#line                              (source location, 1-based / nil, HTML only)
#== #eql? #hash                    (node identity; one wrapper per node)
```

`#xpath` / `#at_xpath` take a `namespace_matching:` option (`:strict` by
default, or `:lax`). Any other keyword binds a namespace prefix, as in
Nokogiri's `xpath("//s:p", s: uri)`, and `XPathContext.new` registers such
keywords too. `Element#content_fragment` returns a `<template>`'s template
contents as a `DocumentFragment`.

Further Document methods (HTML and XML):

```
#root
#create_element #create_text_node #create_comment #fragment
#internal_subset                   (DocumentType node, or nil)
```

HTML Documents only:

```
#title #title= #body #head
#encoding #meta_encoding #meta_encoding=
#quirks_mode                       (0 no-quirks / 1 quirks / 2 limited)
#errors                            (parse warnings; an empty array for now)
```

NodeSet (Enumerable):

```
#[] #at #each #first #last #length #size #empty?
#map #select #reject ...           (Enumerable)
#text #to_html
#css #xpath #search #remove
#+ #- #& #|
```

XPathContext:

```
.new(node, namespace_matching: :strict)
#evaluate(expr)                    (an unknown function is XPath::SyntaxError)
#register_namespace(prefix, uri)   (#register_ns is an alias)
#register_namespaces(hash) #register_variables(hash)
#register_variable(name, value)
#node=(node)                       (rebind the context node, same document)
```

A context caches the compiled AST of each expression, so evaluating the same
expression again skips the parse. `#evaluate` holds the GVL while it
evaluates, so concurrent `evaluate` / `register_*` / `node=` calls on one
context, and mutation of the document during an evaluation, are serialized
by the GVL and cannot corrupt memory.

## 3. The UTF-8 text-input contract

Makiri handles text internally as UTF-8 only. It honours an input String's
encoding: a String in another encoding is converted to UTF-8 before it
crosses the boundary (UTF-8, US-ASCII and ASCII-8BIT are not converted). A
String's bytes are copied exactly once, with no Ruby code run between the
read and the end of the copy, so whatever Ruby later does to the String does
not reach the copy. At the boundary:

* HTML parsing and fragment parsing are lenient, as browsers are.
* The programmatic APIs are strict. A string given to them must be valid
  UTF-8, or the call raises `Makiri::Error`; Makiri never truncates, repairs
  or reinterprets it.
* NUL (U+0000) is handled by the kind of input. Names (element names,
  attribute names, qualified and local names, namespaces, a PI's target and
  data), selectors, XPath expressions and variable names, and engine input
  are refused if they contain NUL. Data (the content of text and comments,
  through `create_text_node` / `create_comment` / `content=`, and attribute
  values, through `[]=` / `set_attribute_ns`) keeps NUL, as the DOM does, in
  HTML and XML alike. An XML serializer refuses a tree holding a character
  outside XML 1.0's character class (U+0000 among them) with
  `Makiri::Error` when it writes it. It decides from the data as written,
  not from a flag a setter left behind.

XPath functions of the caller's own (Nokogiri's handler object) are not
accepted. No Ruby code runs during an evaluation, and a call to a function
XPath 1.0 does not define is an `XPath::SyntaxError`. The exception is
Nokogiri's built-ins `css-class` and `local-name-is`, which answer as
Nokogiri's do under a prefix bound to Nokogiri's builtin namespace.

Arguments are not converted. A string argument must be a `String`, and a
name argument (an attribute or element name, a prefix, a selector, an
expression) a `String` or a `Symbol`; anything else is a `TypeError`.
`to_s` / `to_str` are the caller's Ruby code, and the extension does not run
them. The conversions that do use `to_s` are on the `lib/` side (the
`String()` of the parse entry points, `XML::Builder`), before the extension
is entered.

## 4. Memory model

* DOM nodes are allocated in a per-document arena and freed with it, all at
  once; a node in a tree is never freed on its own. XML and HTML each use an
  index arena of Makiri's own (a node is an index into a `Vec`, a string a
  span of a byte store), which contains no `unsafe`.
* The word that hands an index-arena node to Ruby packs a per-document unique
  stamp and the node's index into one word, so words of different documents
  never collide (the property Lexbor's node pointers had). Resolving another
  document's word is refused rather than taken for a different node.
* The HTML index arena's byte store is append-only and counts the dead bytes
  that `set_data` replacements leave behind. Once the dead bytes exceed both
  the live bytes and a floor scaled to the node count, the store is rewritten
  where an edit begins (right after the indexes are dropped). Spans are
  indexes, so the rewrite invalidates no handle. The XML arena is never
  rewritten: the bytes of a replaced value or a removed node stay until the
  document goes, and keep counting against `max_bytes:`. `dup` compacts one,
  copying only the live content into a new document with the same budget.
* Removing a node from the tree detaches it; it does not destroy it. A live
  Ruby wrapper pointing at a removed node never dangles.
* An index arena's indexes hold ids and spans and borrow neither nodes nor
  strings, so the order in which they and the document are released does not
  matter. What does matter is that every edit drops them before it changes
  anything (`HtmlParsed::edit` → `invalidate_indexes`).
* A Ruby wrapper GC-marks the document it belongs to and keeps the arena
  alive as long as the wrapper lives (HTML and XML alike).
* A Ruby value a wrapped object keeps alive is stored only in a slot owned
  by the Ruby boundary layer; the payload (the Rust struct) holds no Ruby
  value. The slots are written once, together with the payload, right after
  the object is allocated, and the mark callback marks every slot as pinned.
  With no per-type trace to write, neither a forgotten mark nor a value stored
  before its object exists can happen. An object whose content is not yet
  complete gets its hidden ivars written while it still has its class, is
  then hidden, and is shown once its content is filled in.
* Each node has one wrapper, kept in a table the document owns. Reaching the
  same node twice returns the same Ruby object, so whatever was put on the
  object (`equal?`, instance variables, singleton methods, `freeze`) is not
  lost. The document GC-marks the wrappers in its table, so they live as long
  as the document does (the reverse of the keepalive above).
* Allocation in the engine goes through one API that can return failure as a
  value, so OOM is an exception rather than an abort (§5). Ruby-side storage
  (the wrappers) is allocated by Ruby's allocator and leaves OOM to Ruby's
  own `NoMemoryError`. That raise is a non-local exit, so every C call that
  can raise sits under `rb_protect` inside the Ruby boundary layer (`nupur`'s
  `raw`) and comes back as an `Err`; it never jumps over Rust frames.
* Memory held outside Ruby (the arenas, NodeSet arrays, an XPathContext's AST
  cache) is reported to the GC. Reports are made where the GVL is held, and
  the boundary layer's free callback gives each report back from the amount
  the payload declares. No `Drop` of Makiri's calls a Ruby function.
* Indexes and caches belong to the document and are dropped on mutation, with
  the drop hooks gathered in as few places as possible. Each evaluation reads
  the indexes afresh from the document.
* A cache valid only within one evaluation (the string-value cache, XML's
  document-order index) is owned by that evaluation. HTML's document-order
  index is owned by the document and used across evaluations: no Ruby runs
  during an evaluation, and the document is not edited. The XPath engine
  reaches it through the `Dom` items `KEEPS_ORDER` / `kept_position` /
  `build_kept_order`, and does not branch on the kind of host.

All of this is tested under `GC.stress` and compaction.

Thread safety: the GVL is released in exactly two places, the HTML parse and
the XML parse. A parse copies its input into a buffer outside the Ruby heap
and then releases the GVL. The document it builds is not yet shared with
anyone, so nothing can race it, and parsing scales across threads. XPath
evaluation, by contrast, holds the GVL. The engine and the DOM are not
thread-safe against concurrent mutation; holding the GVL serializes an
evaluation against mutation of the same document, and concurrent `evaluate` /
`register_*` calls on the same context, so none of them can corrupt memory.
CSS, serialization and the read accessors hold the GVL too.
`spec/threading_spec.rb` checks shared documents and contexts under
`GC.stress` (required in CI).

## 5. Build system

* One place, the extension's configure step, owns the build's decisions:
  building dependencies, link arguments, and mode switches such as the
  sanitizer.
* A dependency build records its mode in a stamp and is rebuilt when the mode
  changes. An instrumented dependency must never end up in a normal build.
* The extension's own build mode is fixed when it is configured and stays
  until it is configured again. Work that needs an instrumented build starts
  from a clean rebuild, and going back to a normal build is a clean rebuild
  too; the switch back is not automatic.
* Lexbor is used as it is, with no patches and no fork, so following upstream
  stays a submodule update.
* Pins are to release tags as a rule. Makiri pins to anything else only when
  a fix it needs is not in a tag yet. It then records why, and which tag
  would let it move back to a tag, in the developer documentation and the
  changelog, and moves back once that condition is met.
* Lexbor's struct layouts and constants are generated from its own headers,
  never copied by hand.
* The extension exports only the two symbols Ruby needs to load it: the
  initialization function and the ABI version. It does not re-export
  Lexbor's symbols: another gem in the same process using the same library
  would bind to a different version's implementation and crash.
* Exports are restricted at the source. Creating no name to export is the
  mechanism; a post-link step that strips names cannot work at all on some
  platforms, so it is kept only as a backstop.
* A CI gate (`rake symbols`) checks the exports mechanically. It reads the
  whole export table and asserts that no name appears besides the
  initialization function and the ABI version.
* Allocation APIs that abort are forbidden. Engine allocation goes through
  one API that can return failure as a value, so OOM is an exception rather
  than an abort. A lint and an injection test that fails each allocation
  site in turn hold this rule.
* Optimization is one release profile, the same on every platform
  (`opt-level = 3`, `lto = true`, `codegen-units = 1`). The extension builds
  as a cdylib alone, so LTO applies across its crates. The profile changes
  only for a gain measured with the benchmarks.
* The parser port's static tables (tag names, attribute names, namespaces,
  the character-reference search tree, the tree builder's tag categories and
  adjustment tables) are generated from Lexbor's headers and sources and from
  WHATWG's `entities.json`, never copied by hand. Tests check that each
  generated table matches Lexbor's entry for entry.
* There are five Rust crates. The four other than the reference make up the
  extension's workspace. Dependencies run one way: a crate lower down knows
  nothing of the crates above it.
  - The allocation base (`falloc`): allocation that returns failure as a
    value, and the hook that injects failures. It depends on no other crate.
  - The engine (`makiri_engine`): the HTML tokenizer and tree builder, the
    index-arena DOM, the XML reader, XPath, selector parsing and CSS
    matching. It depends only on `falloc` and two Rust libraries
    (`encoding_rs`, `chardetng`), and knows neither Ruby nor Lexbor.
  - The Ruby boundary layer (`nupur`, over rb-sys): it holds all the
    `unsafe` needed to talk to Ruby and exposes only safe types. It depends
    on neither the extension nor the engine, so it cannot know the
    extension's needs. What the extension must hand the layer (the internal
    error's exception class, the raise-injection hook) it registers at
    initialization.
  - The extension (`makiri_rs`): it depends on the engine and the boundary
    layer and holds only the Ruby surface (`bridge`, `glue`, initialization).
    `bridge` and `glue` contain no `unsafe`.
  - The reference (`makiri_lexbor`): the vendored Lexbor with its generated
    layout (bindgen), the adapter that reads Lexbor's DOM, and the
    differential tests that hold the port to Lexbor. It depends on the
    engine, and the shipping extension does not depend on it. Like the fuzz
    crate it is a workspace of its own, so the extension workspace's
    Cargo.lock (the one a source install uses) holds nothing for Lexbor.

  The extension and the reference re-export the engine's modules at their
  crate roots, so `crate::xml` names the engine's module in either.
* The engine's freedom from Ruby rests on the dependency graph rather than
  on conventions or lints. The engine depends on neither rb-sys nor `nupur`,
  so naming a Ruby value or anything of the boundary layer does not compile.
  That the body of a GVL-released parse, and the closure of an edit that
  borrows the document `&mut`, call no Ruby comes down to their calling only
  engine functions.
* The engine contains no `unsafe`: its crate root is
  `forbid(unsafe_code)`. Stable Rust has no safe way to write a fallible
  `Box<T>` allocation, so that allocation keeps one `unsafe`, and this is why
  the allocation base is a crate of its own (`falloc`). `nupur`'s `utf8_copy`,
  which Miri checks without a Ruby, is included in this crate's test build,
  where `unsafe` is allowed. The calls to undefined symbols the
  unwind-freedom proof relies on appear only in the build made for the
  proof, and are allowed there alone.
* The modules on the border between the engine and the extension are placed
  as follows.
  - The compiled-selector cache (`css_cache`) is in the engine. It knows both
    DOMs and no Ruby. It is per thread because no query releases the GVL,
    and that is stated as part of the matcher's entry-point contract.
  - The engine asks for the remaining native stack, and the extension hands
    it the function that checks it at initialization (`stack::install`). The
    OOM-injection hook has the same shape: the engine and `falloc` hold only
    the slot that receives the hook.
  - The branches where the port departs from Lexbor (`compat`) are in the
    engine, and the switch that makes it run as Lexbor does (`as_lexbor`) is
    an engine feature. Where it departs from Lexbor is the engine's own
    knowledge, and only the reference uses the switch. In the shipping build
    the branches are constants.
  - The guard that borrows a document's content (`doc_cell`) is in the
    engine. It exists because several references on the Ruby side share one
    document, and only the extension uses it, but it is Ruby-free. Its
    promises (a loan does not outlive its guard, a guard does not leave its
    thread) are checked by `compile_fail` doctests without a Ruby, which
    requires a crate that does not depend on Ruby.
  - The child-position memo (`child_index`) is in the engine. The version
    that invalidates it (`tree_version`) is kept per document by the
    extension and passed in.
  - The cell keyed by the GVL (`gvl`'s `GvlCell`) is in the reference and not
    in the shipping build. The latch that catches a panic inside a C
    callback (`caught::PanicLatch`) exists twice: the reference has its own
    for Lexbor's callbacks, and in the shipping build `nupur`'s
    (`nupur::caught`) is used by the GVL-released body and the
    `rb_hash_foreach` callback.
  - The engine internals the reference's tests use (the switch to run as
    Lexbor does, the conformance harness, the tokenizer's traced run, and so
    on) are exposed by the engine's `lexbor-compat` feature, which no
    shipping build includes.
* The engine's public API is limited to what the extension and the reference
  call. The engine's invariants (edits happen only inside the closure that
  lends the document, a half-built node never enters a tree, and so on) are
  still held by types, not by crate visibility, and every dependent crate
  (the extension, the reference, fuzz) follows the same type rules.
* The token that proves the GVL is held is made only where Ruby calls in: the
  method trampolines and the initialization function. Its helper is
  `unsafe`, so extension code, which cannot contain `unsafe`, reaches it only
  through a macro's expansion.
* Calls into the engine and the boundary layer cross crates, and the
  cdylib's LTO inlines them. Building the extension as an `rlib` as well would
  stop rustc from running LTO, so the fuzz crate depends on the reference
  (which re-exports the engine) rather than on the extension.
* Features belong to each crate. The engine has `html-dom` (the default: the
  index-arena HTML DOM, CSS matching over it, and XPath's HTML instance;
  without it the engine is the bare one),
  `lexbor-compat` (the switch to run as Lexbor does, used only by the
  reference) and `alloc-inject` (OOM injection). `falloc` and `nupur` have
  `alloc-inject` too, and the extension's `alloc-inject` turns on all three.
  The reference has `css-reference` and `lexbor-reference` (the references
  the differential fuzzers compare against). The features that once removed
  the Ruby boundary or read Lexbor are gone: they became the extension and
  the reference crates themselves. The shipping extension links no Lexbor and
  reads none of its headers; rb-sys, reading Ruby's headers, is the only user
  of bindgen left.

## 6. Testing

### 6.1 Unit and integration tests (rspec, `spec/`)

Per-class behaviour: parsing corner cases, reading nodes, navigation,
mutation, serialization, encoding, XPath, CSS and source locations.

### 6.2 Robustness fuzzing (`spec/fuzz/`)

Run with `rake fuzz` or `rake fuzz:sanitize`. The fuzzer generates and
mutates XPath and CSS expressions along their grammars and checks that
Makiri raises nothing but `Makiri::Error`: no foreign exception, crash or
hang.

The cargo-fuzz harnesses (`ext/makiri/rust/fuzz/`) drive the engine without
Ruby. Some check only that nothing breaks (`html`, `xml`, `xpath`,
`xml_xpath`, `html_xpath`, `css`, `html_css`); the differential ones check
agreement with the reference: `html_diff` (the ported tree builder against
Lexbor's tree, its serialization and the depth limit's verdicts),
`selector_diff` (the ported selector parser against Lexbor's),
`stylesheet_diff` (the ported stylesheet parser against Lexbor's) and
`html_css_diff` (Makiri's matcher against Lexbor's). `css` also checks that
the XML matcher's entry points agree with each other. The fuzz crate depends
on the reference (`makiri_lexbor`), not on the extension, and links what it
compares against only through the reference's `lexbor-reference` /
`css-reference` features.

> The original plan to give the fuzzer a diff mode with Nokogiri as the
> oracle was not adopted. Lexbor and libxml2 have different HTML5 semantics,
> and structural diffs over random input give many false positives. The
> differential fuzzers compare only against a reference that should have the
> same semantics (Lexbor, where the port comes from), and expect zero
> disagreements. The only differences allowed are safety fixes, and only
> those listed in the differences table. Comparison with Nokogiri is a
> conformance check separate from fuzzing (§6.4).

### 6.3 Property and differential tests (`spec/invariants/`, `spec/differential/`)

Randomized inputs check invariants: namespaces, tree shape, index
staleness, serialization and the text-input contract. Answers an earlier
implementation gave are also fixed as a baseline and compared against the
current build. The baseline is never regenerated, and a disagreement is a
finding to investigate. An intended change rewrites the baseline in the same
commit as the behaviour change, so it is reviewed as a difference.

### 6.4 Conformance and differences from the reference

Checks against the specifications and the reference are grouped under
`rake conformance:*`.

* HTML parsing: WPT's tree-construction data (where html5lib's tests now
  live) runs through a Ruby runner (`conformance:html5`) and a crate runner
  (`conformance:parser`). Both read the same list of known divergences
  (`spec/conformance/parser_known_divergences.txt`), and a listed test that
  passes also fails the run. The tokenizer runs html5lib's tokenizer tests.
* The parser port is checked against Lexbor as its reference: its trees must
  match on every WPT input, on random documents and fragments, and on real
  web pages (web100k, `conformance:web100k`, not run in CI)
  (`lexbor::tree_diff`). The migration-time checks that held the index arena
  to the Lexbor adapter for reads, edits and XPath (`lexbor::arena_tests`,
  `html_xpath_diff`) were retired once the arena became the only
  implementation. The ported selector parser must match Lexbor's parse
  (`lexbor::selector_diff`), and the stylesheet parser is held to Lexbor's
  the same way (`lexbor::stylesheet_diff`).
* Browsers (Chromium, Firefox, WebKit) serve as evidence, not as the pass
  criterion (`conformance:browsers`). Disagreements between oracles are
  recorded in the differences table.
* Comparison with Nokogiri: XPath, CSS, XML and the Builder are each
  compared with Nokogiri, and XML is also checked against the W3C XML Test
  Suite. The expected number of disagreements is zero. Only differences
  listed explicitly as policy are allowed, and they are fixed in
  hand-curated specs and in the difference documents (§7,
  `docs/parser_differences.ja.md`, `NOKOGIRI_DIFFERENCES.md`).
* The versions of the specifications, WPT, html5lib-tests and
  `entities.json` are fixed and recorded in `docs/parser_differences.ja.md`.

### 6.5 Memory safety and resources (sanitizer, leaks, OOM)

* The specs and the fuzzers run under AddressSanitizer. Makiri's own arenas
  address nodes and strings by index rather than handing out pieces cut from
  a large region, as a bump allocator does. A bump allocator needs poisoning
  for the sanitizer to catch an overflow from one piece into the next; with
  index addressing, an out-of-range access inside the arena is caught by a
  bounds check, so no poisoning is needed.
* The vendored Lexbor is not in the shipping build; it is built plain, as
  the reference only. Its bump arena is not a target of the sanitizer
  checks, and it gets no patches.
* Leak checks are kept separate from the sanitizer and cover paths that exit
  through an exception as well.
* OOM injection fails every allocation site in turn and checks that each
  failure fails closed: a clean raise, or the same answer as when nothing
  failed.
* The Ruby-free core (allocation, buffers, UTF-8 validation and decoding) is
  also formally verified.

### 6.6 Performance (`bench/`)

`rake bench` compares Makiri with Nokogiri. An optimization is kept only
when the benchmarks show a measured improvement and the specs, fuzzers and
sanitizer runs are green; otherwise it is reverted. Before the parser port
became the default, every benchmark row was checked for a regression in
kind (a row falling to Nokogiri's level or below); see the progress section
of `docs/rewrite_parser.ja.md`.

### 6.7 CI

GitHub Actions runs every supported Ruby version on each OS. The sanitizer
runs on Linux only. Only the jobs that build the reference build the
reference Lexbor with `rake lexbor`: clippy including the reference
(`rake lint`), `rake rust:test`, the Rust tests under ASan (the sanitize
job), the differential fuzzers and `conformance:parser`.

## 7. Compatibility

Lexbor's HTML5 parsing semantics are closer to the WHATWG specification than
libxml2's, so some results differ from Nokogiri's. Makiri follows the
specification and fixes the differences in curated specs:

| Item | Makiri | Nokogiri / browser |
| --- | --- | --- |
| Implicit `<head>` | Always inserted (HTML5) | HTML4 (libxml2) creates one only when an element belongs in it; HTML5 always does |
| Namespace resolution of name tests | Strict by default, `:lax` opt-in | Strict in HTML5; HTML4 has no namespaces and behaves like `:lax` |
| XPath namespace axis | Not implemented (an explicit error) | Implemented by libxml2 |
| Parsing invalid bytes | Replaced with U+FFFD, as browsers do; parsing never fails | HTML4 guesses the encoding of binary input and keeps invalid bytes in input declared as UTF-8; HTML5 replaces them with U+FFFD |
| ASCII case folding of name tests | HTML elements and attributes are folded (SVG/MathML kept as written) | Nokogiri does not fold (HTML4 or HTML5); browsers fold |
| Namespace of HTML elements | The XHTML namespace (attributes in no namespace) | No namespace in HTML5 (`namespace-uri()` is the empty string) |
| `<?target data?>` in HTML | A processing-instruction node (the HTML Standard's current definition) | A comment in HTML5; a processing instruction in HTML4 |
| Scope of a CSS search from a node | Matched against the whole document, results limited to descendants (as `querySelectorAll`, in HTML and XML) | The selector is anchored at the node |
| Unprefixed type selector in XML's CSS | Matches elements in any namespace (as browsers); the default namespace is given by the ns Hash's `"xmlns"` | The root element's default namespace (no namespace when an ns Hash is given) |
| Renaming an element (`name=`) | Not possible (the DOM has no rename) | Possible |

HTML4 and HTML5 in the Nokogiri column mean `Nokogiri::HTML4` and
`Nokogiri::HTML5` (checked with Nokogiri 1.19 / libxml2 2.13).

Where the HTML parser departs from a fixed version of the Standard (in
Lexbor's default behaviour and in the port), where the port departs from
Lexbor for safety, and where oracles disagree with each other is listed,
numbered, in `docs/parser_differences.ja.md`.

HTML name tests and HTML attributes fold ASCII case, and SVG/MathML names
are kept as written. HTML's XPath attribute axis leaves out the `xmlns` /
`xmlns:*` attributes of foreign (SVG/MathML) elements, which are attributes
in the XMLNS namespace; an `xmlns` written on an HTML element shows as an
ordinary attribute. Nokogiri returns a foreign element's `xmlns` as an
attribute as well.

## 8. Design decisions

These can change if circumstances do.

* `HTML()` is the definition and `parse()` an alias, matching Nokogiri.
* The HTML parser is a Rust port of Lexbor, not html5ever. html5ever's
  allocation cannot report failure (which conflicts with fail-closed), its
  atom table is shared across the whole process, and its tendrils contain
  `unsafe` that does not fit with `falloc`. The port maps Lexbor's state
  functions and insertion modes one to one, so that agreement with Lexbor can
  be checked by differential fuzzing. Changes to follow the Standard go in
  afterwards, as changes of their own, once agreement is established.
* The port's DOM was an index arena from the start, without going through a
  pointer-keeping DOM first. A node takes about half the bytes of a node in
  Lexbor's DOM, and an out-of-range access is caught by a bounds check. The
  bridge was first changed to treat a node as a reference into a document
  (`RawNode`) and to edit only inside a closure that borrows the document
  `&mut`, so that one facade could switch between the two backends.
  The content a Document owns is held in the engine's `doc_cell::DocCell`
  (a `RefCell` inside), and `bridge::dom` holds its read and edit guards. The guards are the only way to the content, and an
  edit is refused at run time while a read is still held.
* Selector parsing is also a port of Lexbor's parser, so the parse results
  can be owned by a cache without being tied to the lifetime of Lexbor's
  shared arena. Reading stylesheets (`parse_stylesheet`) uses the port too.
  The rule structure and the error recovery are Lexbor's, but declaration
  names and values are no longer normalized per property: every property
  follows one rule (names lower-cased, values the source text with
  whitespace runs collapsed), the intermediate option in
  `docs/rewrite_parser.ja.md`.
* Each node has one wrapper, cached in the document's table (§4). At first
  wrappers were not cached and nodes were compared by identity, but then
  every visit to a node returned a different object, and the state carried
  by the object (`equal?`, instance variables, singleton methods, `freeze`)
  was silently lost. NodeSet and Node comparisons still use the node's
  identity.
* XPath evaluation does not release the GVL. It once did, running XPath in
  parallel across threads (scaling about 3.3x). But the walk with the GVL
  released raced with mutation of a shared document and with concurrent
  `evaluate` / `register` calls on the same context, causing use-after-move
  and memory corruption. Guarding each document and context with a mutex was
  considered, but every later change to the engine would have to keep the
  rule "the GVL-released section touches no Ruby and reads only locked
  state", and that maintenance burden is large. Holding the GVL, which
  serializes the native code of every Ruby thread, removes the races
  themselves without any lock. Single-thread performance is unchanged, and
  parsing still releases the GVL and runs in parallel; what is given up is
  only running XPath in parallel over one shared document. A GVL-released
  XPath path will not come back until document and context locking is in
  place.
* The engine is a crate of its own (§5). Even before the split it was
  written without knowing Ruby or Lexbor and ran without Ruby (a build
  without the `ruby` feature, Kani, Miri, Verus, cargo-fuzz, Ruby-free
  measurements). But the boundary was defined by hand-maintained module
  lists (the lint's `ENGINE`, the unsafe check's `ENGINE_DIRS`, the roots of
  `forbid`), and the lists disagreed with each other: deleted modules stayed
  and new ones were missing. As a crate, the boundary is the dependency
  graph, and the lists go, as does what the lint guarding them
  (`ruby_layer_in_engine`) checks in the engine; the lint stays only for one
  Ruby-free module in `nupur`, `utf8_copy`. This is the same idea as
  splitting off the boundary layer (`nupur`), and LTO over the single cdylib
  build keeps the performance.
* Parsing is lenient and the APIs are strict. HTML parsing decodes as a
  browser does, replacing invalid bytes with U+FFFD, and always succeeds. That
  conforms to HTML5, lets broken pages be scraped, and the replacement with
  U+FFFD also prevents smuggling. A string passed to a programmatic API, on
  the other hand, is an instruction for processing rather than data bytes,
  so it must be valid UTF-8 with no NUL, and a violation raises.

## 9. Migrating from Nokogiri (unofficial)

Existing Nokogiri code such as:

```ruby
require 'nokogiri'
doc = Nokogiri::HTML(source)
doc.css('.foo').map(&:text)
```

runs after rewriting it as:

```ruby
require 'makiri'
doc = Makiri::HTML(source)
doc.css('.foo').map(&:text)
```

The goal is for about 90% of the code commonly used for HTML scraping to work
by replacing `Nokogiri` with `Makiri`. Most of the remaining 10% or so
behaves differently because Makiri is stricter about conforming to HTML5.
