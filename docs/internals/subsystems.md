# Subsystems

The HTML DOM in this file is the arena (`html::dom`, behind the `html_dom`
facade, parsed by `html::tree`); Lexbor's tree is only the reference the
parser is held to (`lexbor::tree_port_tests`, `html_diff`).

## Text-input contract

Input text takes two paths. A document parse and a fragment parse decode
leniently, as a browser does, and never fail on bad bytes. The programmatic
APIs (XPath, CSS, mutation, factories) are strict and raise.

### Document parsing: the browser's encoding sniffing

A DOCUMENT parse decodes its bytes as a browser does: `html::encoding`
decides, `html::tree::parse_document_bytes` drives. It follows:

- the HTML Standard's encoding sniffing algorithm: the BOM; the transport
  layer's encoding (`encoding:`, else the String's own when it has one); for
  an ASCII-8BIT String, the 1024-byte `<meta>` prescan, then UTF-8 if the
  bytes are valid UTF-8, then `chardetng` over 64 KiB from the first
  non-ASCII byte (`DETECT_MAX`);
- "change the encoding": a `<meta>` met while the confidence is tentative
  re-reads the document;
- the Encoding Standard's decoders (`encoding_rs`, into a `falloc` buffer).

`Document#encoding` is the encoding the document was read in. A Ruby
encoding the Standard has no label for (UTF-32, CP51932) is transcoded by
Ruby as before (`DocumentSource`, over `html::encoding::transport_for`).

### Fragment parsing: the String's own encoding

Fragment parsing takes text, so it has no sniffing; it honours the input
String's encoding instead (`nupur::string::to_utf8`, replacing):

- UTF-8, US-ASCII and ASCII-8BIT pass through untouched. The UTF-8 common
  case is one read of the encoding index from the String's flags: no call,
  no transcode, no copy.
- Any other encoding (Shift_JIS, EUC-JP, ISO-8859-1, ...) is
  `rb_str_encode`'d to UTF-8 (invalid/undef → U+FFFD), so its content
  survives instead of being read as raw UTF-8.

After that the bytes are UTF-8. A document's UTF-8 skips the decoder, since
the sanitiser below replaces as the UTF-8 decoder does
(`the_sanitiser_is_the_utf8_decoder`).

### Lenient decoding: parsing never fails on bad bytes

HTML parsing then decodes leniently, like a browser: `sanitize`
(`utf8_input.rs`) replaces any remaining invalid UTF-8 with U+FFFD and
leaves a NUL for the HTML5 tokenizer to drop or replace. So parse and
fragment **never fail** on bad bytes, and the DOM is always valid UTF-8.

The validation is a dedicated validate-only scan (Unicode well-formed table
plus word-at-a-time ASCII). It is skipped entirely when the String's cached
coderange (read via `ENC_CODERANGE`, no forced scan) already proves it
valid: `parse_html`'s `assume_valid` and `RString::known_utf8` (nupur).

### Strict programmatic inputs

The programmatic APIs are strict. `ruby_verified_text`
(`bridge/string/view.rs`) raises `Makiri::Error` for **invalid UTF-8
everywhere** at the XPath, CSS and mutation boundaries (expr, selector,
attribute name and value, `content=`, `create_*`, variable, namespace), and
never truncates or repairs it.

An embedded NUL (U+0000) has a two-tier contract:

- **Rejected** for names, tags, namespaces, PI target and data, selectors,
  XPath, variables and all engine inputs (`text::VerifiedText`, a `&str`
  with no NUL). This is a logical contract, since every consumer, Lexbor's
  selector parser included, reads a slice and none a C string.
- **Accepted** for the HTML data family, so the DOM can hold U+0000 as
  browsers do: text and comment node content (`create_text_node`,
  `create_comment`, `content=`) and attribute values (`[]=`,
  `set_attribute_ns`). These data-family sites go through
  `ruby_verified_data` (a distinct type, `RubyData`, UTF-8-validated but
  NUL-permitting), never `ruby_verified_text`.

A NAME given to a factory or setter goes through `ruby_verified_name` (XML:
`bridge::xml::verified_name`). It has the same `RubyText` contract, but its
NUL refusal is `ArgumentError`, like every other name refusal there (the
DOM's InvalidCharacterError), so a caller mapping `ArgumentError` to a
DOMException misses none. Removers and readers keep `ruby_verified_text`.

The strict inputs honour the String's encoding too
(`nupur::string::read_utf8`, over `nupur::string::to_utf8`). A String not in
UTF-8, US-ASCII or ASCII-8BIT is transcoded to UTF-8 first, with NO
replacement: a character or byte it cannot convert is `Makiri::Error`
("could not be converted to UTF-8"), never repaired, and any other raise in
the transcode (`NoMemoryError`, an interrupt) is passed through as it is.

Every reader's transcode - a fragment's, a strict view's, the XML decode's -
is that one function, `nupur::string::to_utf8`. It alone tells the String's
fault (an `EncodingError`, `Transcode::Fault`, with the caller's `$!` kept)
from what is passed on; each reader words the fault itself. A length cap
(`ruby_try_verified_text`, over `nupur::string::read_utf8_within`) is on the
UTF-8.

### Checked views own a copy

Both checked views (`RubyText`, `RubyData`) deref to `&str`, and each OWNS A
COPY of the (UTF-8) String's bytes. The copy is taken with no Ruby between
the read and the copy (`nupur::utf8_copy::Utf8Copy`). Up to 64 bytes are
inline, so a UTF-8 selector or expression allocates nothing; one in another
encoding allocates its transcode.

The copy is checked after it is taken, and its UTF-8 is checked by std's
`from_utf8` over the copy, ALWAYS. The cached coderange is not consulted
there, since a wrong one (another C extension's, a Ruby bug) would make the
`&str` undefined behaviour. The parse source and the XML input check still
take the coderange as a hint, where a wrong one stays safe code. Nothing
Ruby does afterwards - a GC, the caller rewriting or growing the String -
reaches the copy.

The other inputs are copies too:

- The HTML source (`HtmlSource`) is the copy a document parse needs to read
  it with the GVL released.
- The XML input: the decode copies the document once, BOM stripped, and both
  the strict check and the parse read that copy.

No String's bytes are borrowed outside `nupur` at all. A String is read only
by `RString::copy_to` (into a caller's buffer) and `append_to` (into a
`Vec`'s reserved room, never growing it), each a read and a copy with
nothing run between.

The views used to hold the String under `rb_str_locktmp`, with a copy when
it was already locked, a frozen fast path and a raise to re-propagate - all
to spare a copy of a few bytes.

### XML data

`Makiri::XML`'s data takes NUL too (`bridge::xml::verified_data`), as the
DOM does, and so does every character XML has no `Char` for: the XML
mutators hold what the DOM holds. The serializers refuse a tree XML cannot
write, deciding from the data as it is written (`serialize::out`'s
byte-class table), never from a flag a setter could leave stale. XML names
stay NUL-strict, like HTML's.

Don't drop the UTF-8 checks or route a name or engine string through the
data path. The two contracts are the types `RubyText` and `RubyData`
(`bridge/string/view.rs`).

### No argument is converted

A string argument is a `String` (`bridge::ruby::string_arg`); a name -
attribute, element, prefix, selector, expression, encoding - is a `String`
or a `Symbol` (`name_arg`). Anything else is a `TypeError`, since a
`to_s`/`to_str` is the caller's Ruby. What converts with `to_s` is `lib/`'s,
outside the extension: the parse entries (`String(source)`, an IO's `read`)
and `XML::Builder`.

`rake dylint`'s `runs_callers_ruby` fails on every call that would run the
caller's Ruby, by resolved path. Two are allowed, with each wrapper along
the way an allow with its reason:

- `NodeSet#each`'s `yield` (through `rb::method::yield_value`, over
  `rb_yield`);
- `XML::Namespace.new` (`Module::call_method`, over `rb_funcallv`).

A registered method's parameters are held to the rule by type:
`bridge::rb::TryConvert` exists only for `Value`, `bool` and Makiri's
receiver types (`HtmlSelf`, `XmlSelf`, `XPathCtxSelf`, `NodeSetSelf`), so a
`method!` / `function!` taking an `RString` or an `i64` does not compile.
magnus converted those with `to_str` / `to_int`; `rake dylint`'s
`converted_argument` lint held the line until the switch.

## Parsing and source location

The tree builder stamps an element when it is CREATED FOR ITS OWN START TAG
(`TreeBuilder::created_offset`, `html/tree/stamp.rs`): a real start-tag
token with no base element. It never stamps a `Tok::fake`, an
adoption-agency or reconstruction copy, or an element implied by another
tag's token (the `<tr>` a stray `<td>` implies). So every element the parser
invents stays nil, and a void element fostered out of a table has its line
(L1 in `docs/parser_differences.ja.md`).
`lexbor::tree_diff::offsets_name_their_tags` holds the stamps to the one
property `#line` promises: every stamp sits at a start tag of the element's
own name.

The offset goes in the node's `source` field (see `engine_rules.md`), and a
line table (`source_lines::Lines`, built once per parse) resolves it to a
1-based line. `Node#line` returns an Integer, or **nil** when the node
cannot be placed - never a wrong line. The nil cases are every element the
parser invents, text, comment and attribute nodes, nodes copied from another
document, and fragments (their parse stamps nothing). The first `#line` is
O(1), and a mutation needs no ordering.

The Lexbor-era design stamped from a token-done hook around Lexbor's tree
builder. An even earlier one paired recorded tokens with elements after the
parse, and parser-made elements stole later tags' lines.

## HTML parse guard

Every parse runs under a `Guard` (`html/tree/guard.rs`) that the tree
builder checks after each token: `Guard::for_document` from `parse_html`,
and `Guard::for_fragment` (below the synthetic `<html>`) from
`parse_fragment`. It bounds:

- the tree depth (`max_tree_depth:`, default 400, Nokogiri's);
- the document's size against its input (`max_amplification:`, default 256
  times the input and never less than 64 MiB).

`ParseLimits` carries both, and a negative keyword lifts either.

The size bound exists because "reconstruct the active formatting elements"
re-creates every unclosed formatting element before each character: ~390
`<b>`s with distinct attributes then `<div>x</div>` repeated built a 62 KB
input into a 344 MB document, where ordinary documents build 10-46 times
their input (`spec/html_amplification_spec.rb`). The check reads
`Document::approx_bytes`, O(1) per token. A refused parse fails closed
(`Makiri::Error`, "document size limit exceeded"); the tree of an accepted
one is the Standard's. The check is per token, not per insertion, so a void
element at the limit is not refused.

There is no limit on a `<select>`'s options. The selectedness setting
algorithm is decided from each select's state (`html/tree/select.rs`,
`SelectStates`), so a select builds in linear time - keep it so, or the
10,000 limit it replaced has to come back.

## Attribute parents

**An attribute's parent is its element**, held in the attribute node's own
`parent` link, which appending the attribute sets and removing it clears
(`html/dom/mod.rs`). `Document::parent` reads it live, so the XPath
parent/ancestor axes, document order and `Attr#parent` all agree and need no
index.

## The indexes

The indexes (`html/dom/index.rs`, owned by the parse handle, `HtmlParsed`)
are built lazily and name nodes by id and text by its span in the byte
store, so neither borrows the arena. **Every edit drops them before anything
changes** (`HtmlParsed::edit` -> `invalidate_indexes`). The byte store is
compacted - moving every span - only right after (`html/dom/compact.rs`), so
no index survives a span it names.

### Element index

`DomIndex` is a `tag id -> elements` document-order CSR over the static tag
range, for the XPath `//tag` fast path (`xpath/step_index.rs`;
`DomIndex::tag_bucket`). A custom tag, a foreign element (`has_foreign`) or
an HTML element written in upper case (`has_case_kept`) sends `//tag` back
to the tree walk.

It is built in one walk (each element's tag and id, then a counting sort) by
the first `//tag` that asks for a bucket (`HtmlDom::name_bucket` ->
`HtmlParsed::ensure_dom_index`). It is never built up front, so an XPath
that names no tag (`/html/body`) pays nothing for it. OOM answers no bucket,
and the step walks.

**Every evaluate reads the index afresh from the handle.** A reused
`XPathContext` that kept the one it first saw answered from a dropped index -
a use-after-free in the Lexbor era (`spec/xpath_context_mutation_spec.rb`).

### Document-order index

`DocOrder` holds each node's position in XPath's document order - a node,
its attributes, its children; `<template>` contents left out, as the XPath
walk leaves them - as a dense `NodeId -> u32` array sized to the arena.
XPath's sorts read it through `Dom::KEEPS_ORDER` / `kept_position` /
`build_kept_order` (`xpath/order.rs`, `sort_by_kept_order`). A node it does
not hold (detached, a fragment's, appended since) falls back to the
parent-chain compare.

It is built by the first sort large enough to want it (`INDEX_BUILD_MIN`)
and kept across evaluates until an edit drops it; OOM falls back for the
rest of that evaluate. It is counted in `HtmlParsed::external_bytes` and
reported to the GC once the evaluate that built it is over
(`account_kept_order`). XML keeps the evaluate's own `OrderIndex`. Verify
with `spec/xpath_kept_order_spec.rb` and `xpath::order::speed::order_speed`
(ignored; a measurement).

### Text index

`TextIndex` holds every text node's span in document order and each
container's `[start, end)` run of them (a dense array by `NodeId`, built in
one walk). A `Node#text` is then a lookup plus `utf8_str_from_slices` - one
pre-sized memcpy run, no element node touched (**~4x libxml2**).

It is built only once the walks `Node#text` has served since the last edit
cover half the document (`HtmlParsed::ensure_text_index` /
`note_text_walk`). The build costs about one walk of the document, so a
one-off `li.text` walks its own few nodes instead.

It is reached via `HtmlParsed::text_slices`. `None` (not due, a fragment, a
build OOM) means the caller walks (`bridge::html::subtree_text`: one walk
collecting the slices, one copy). Verify with `spec/text_index_spec.rb`,
which holds it byte-identical to a plain walk across subtrees and
mutations.

## Child counting and indexing

`#child_count` / `#child_at` and their element twins
(`engine/src/child_index.rs`) walk `dom_rules::Tree`'s links - the insertion
rules' description of each tree, so the three cannot disagree. They remember
where they were in a per-document `ChildPositionMemo` (a `PtrMap` per list,
emptied past `CHILD_MEMO_MAX`), so an index loop is O(1) a step.

The memo answers only for the `tree_version` it was filled under. That
invariant is what makes a recorded token, read back through
`TokenTree::node_of`, name the right node; the lookup is checked, so a stale
token is walked afresh rather than answered. So keep every child-list edit
moving the version BEFORE it changes anything (`record_edit`). Any edit
empties the whole memo (the module doc's known limit).

The four Ruby methods are generic over `bridge::wrapper::ChildListHost` in
`glue::node`. The element-only navigation (`first_element_child`,
`next_element`, ...) is `child_index`'s too, over the same links and element
test.

## XPath engine

The XPath engine (`engine/src/xpath/`) is an original implementation: lexer
→ recursive-descent parser → AST → evaluator + 26 built-in functions. The
only external hook is `Dom::qualified_name` (in `xpath/dom.rs`). Per-evaluate
budgets (op count, recursion depth, step/predicate/arg counts, node-set and
string caps) live in `xpath/limits.rs` and fail closed with `Status::Limit`.

### Recursion and the native stack

`enter_recursion` also asks `crate::stack::exhausted()`, installed by
`bridge::stack` at `Init_makiri` (see its module doc). A depth COUNT alone
assumes the caller's frame started with a full-sized native stack. A small
`Fiber` (`RUBY_FIBER_MACHINE_STACK_SIZE`), or a caller already deep in its
own recursion, does not have one: there the count refuses too late, and
Ruby's `SystemStackError` handling longjmps past `Drop`. CSS matching, HTML
and XML, keeps its nesting on a heap stack and needs no check.

Every other recursion in the engine passes a `crate::stack::Depth` down as
an argument: the XML writers (`to_xml`, `canonicalize`), the DTD's
content-model groups, `parse_stylesheet`'s rule nesting. Its `enter` refuses
both on its limit and on a low stack (asked every `STACK_CHECK_EVERY`
levels), so a new recursion written with one cannot count and forget the
stack. The writers did, and a 500-deep document killed the process in a
128 KiB Fiber (`spec/native_stack_guard_spec.rb`).

### Ruby API and errors

`Node#{xpath,at_xpath}(expr, [namespaces])` and `Makiri::XPathContext`
(`.new`, `#evaluate`, `#register_namespace`/`#register_ns`,
`#register_variable`). `#xpath` returns a NodeSet for node-sets, else a
String, Float or boolean. Errors map SYNTAX → `XPath::SyntaxError`, LIMIT →
`XPath::LimitExceeded`, anything else → `Makiri::Error`.

### No custom functions

**There are no custom functions**: Makiri runs no Ruby while it evaluates.
An unprefixed call the core library lacks is an `XPath::SyntaxError` when
the expression is parsed (`parse_function_call`, over
`funcs::is_core_function`), a prefixed one when it is evaluated (the prefix
is bound then). A Nokogiri-style handler argument is an `ArgumentError`.

The handler bridge that used to answer such calls went with them
(`notes/ruby_boundary_simplification_plan.ja.md`): an `rb_protect` per call,
a Fiber-suspended walk left holding the document's guard, and a mutation
gate on every edit while one ran.

### Namespace axis

The **namespace axis is not implemented**: it raises "not implemented",
never answers silently empty. Nokogiri/libxml2 *does* implement it (e.g.
`<svg>` in HTML yields the `xml` + `svg` namespace nodes), so this is a
documented behaviour difference; see `NOKOGIRI_DIFFERENCES.md`.
`namespace-uri()` and `local-name()` are implemented.

### Namespace matching of name tests

**Name tests are namespace-strict by default**, faithful to HTML5/WHATWG
like browsers' `document.evaluate` and `Nokogiri::HTML5`. An *unprefixed*
element name test resolves in the HTML namespace, so `//div` matches but
`//svg` and `//path` do NOT; foreign (SVG/MathML) elements need a registered
prefix (`//svg:path`). Pass `namespace_matching: :lax` (on
`Node#{xpath,at_xpath}` or `XPathContext.new`) for the namespace-agnostic,
`Nokogiri::HTML`-style match, where `//path` finds the SVG element.

**The rule for the two modes: strict is the specification, lax is
Nokogiri** - whatever Nokogiri does for the host. So the mode affects *only*
unprefixed element name tests in HTML (`Nokogiri::HTML` has no namespaces).
Prefixed tests, the `*` wildcard and attribute tests are unchanged, and in
XML the flag changes nothing, because `Nokogiri::XML` (libxml2) is as
namespace-strict as the spec.

The decision is the `Dom::unprefixed_matches(n, is_attr, lax)` policy item,
which the axis and the `[@a]` fast path both reach through
`nodetest::unprefixed_attr_matches`. Makiri keeps HTML elements in the
XHTML namespace, so `namespace-uri()` is correct, unlike `Nokogiri::HTML5`'s
null.

### ASCII case folding of name tests

**Name tests fold ASCII case on HTML elements**, following browsers and WPT
`domxpath`, NOT the HTML Standard, whose XPath section only sets the default
element namespace. `//DiV` finds `<div>`, and `[@Id]` its `id`, in both
modes; SVG/MathML names stay exact (`refX`). One rule,
`nodetest::names_equal` over `Dom::folds_name_case`, serves the name test
AND the `[@attr]` fast path - keep it that way. The host's getAttribute
(`id()`, `lang()`) has rules of its own.

The HTML attribute axis also skips attributes in the XMLNS namespace (a
foreign element's `xmlns`/`xmlns:*`), as the XML backend skips
declarations.

## XPath host policy

**Host policy lives in `Dom`, never in a host test.** Where XPath over HTML
and over XML differ, the difference is a named item of the `Dom` trait
(`xpath/dom.rs`, "host policy"):

- `test_name` / `attr_test_name` (local vs qualified name);
- `unprefixed_matches` (the strict rule);
- `attr_ns_uri` (an attribute's OWN namespace - `namespace-uri(//div/@id)`
  is `""`);
- `ID_ATTRIBUTE` (`id` in HTML, none in XML);
- `LANG_ATTRIBUTES`.

The engine never asks which host it walks; the `IS_XML` flag that did is
gone, and a new policy is a new item stated in each `impl`. The HTML backend
reports the DOM's case-preserved `localName` (`refX`, `foreignObject`), not
Lexbor's lower-cased stored name.

## CSS

CSS is `css_match/`, `css_cache.rs` and `selectors/`.
`Node#{css,at_css,matches?}` (`glue::html_node::css`,
`glue::xml_node::css`) match through `css_match` on both representations.

### Matcher, parser and cache

`css_match` is an original, `#![forbid(unsafe_code)]` Rust port of Lexbor's
`lxb_selectors_*` state machine, NOT Lexbor's own matcher: its
`find`/`found_check`/`not_found` loop over a chain's compounds, with nested
list-pseudos as contexts on a heap task stack. It never uses native
recursion for selector nesting, as `lxb_selectors_run`'s loop never recurses
either.

Selector PARSING is the parser port (`crate::selectors`,
`docs/rewrite_parser.ja.md` stage 5): Lexbor's CSS syntax tokenizer, its
rule and state stacks and its selector states, line for line, into an owned
index-arena AST. The AST is read through views of the same shape as
`lexbor::css_parser`'s (`List`, `Lists`, `Selector`, `Simple`, ...), so the
matcher reads either. `lexbor::selector_diff` holds the port to Lexbor's
parser view for view (the `selector_diff` fuzz target too); the two answers
that differ on purpose are `docs/parser_differences.ja.md` S2/S3.

The glue reaches the parser through `css_cache`: an adaptive
compiled-selector cache (`CachePolicy`, the OLD HTML engine's - same
constants, same flush-on-cap/bypass-on-low-hit-rate/retest behavior) holding
each parsed selector it keeps, so a repeated selector string is not
re-parsed every call. The adaptive cache policy lives only in `css_cache`.

**`parse_stylesheet` is the port too** (`selectors::parse_stylesheet` +
`selectors/stylesheet.rs`). It keeps Lexbor's rule structure and recovery
exactly, but writes names and values by one rule for every property, not by
Lexbor's per-property reserialization (`docs/parser_differences.ja.md`
C1-C3):

- a declaration name is lower-cased unless it is `--*`;
- an at-rule name is lower-cased;
- a value is the text between Lexbor's offsets, with whitespace runs
  collapsed.

Two Lexbor pieces remain, only as references:

- `lexbor::css_parser`: its `parse` and its views over Lexbor's OWN lists
  are what `selector_diff` and `stylesheet_diff` compare against, built only
  for tests and the reference features.
- `lexbor/selectors.rs`, the OLD `lxb_selectors`-callback engine this
  replaced for HTML, is the differential-testing reference
  (`lexbor::tests::css_match::agrees_with_the_old_lexbor_engine_on_standard_selectors`
  and its randomized siblings, and the `html_css_diff` fuzz target) and
  nothing else. It is `#[cfg(test)]` or the `css-reference` feature, so no
  build that ships carries it, and it has no cache of its own: parse, run,
  clean per query.

### The matcher

**The matcher is Lexbor's control flow, in safe Rust** (`css_match`'s
`Compiled` and `Query`). `compile` decodes every simple selector once into a
`Step` (Lexbor's entry) and splits every chain - top level and every nested
list - into one compound table. A list-pseudo's
`Step` holds the index range of its alternatives' chains, so matching never
looks a list up.

`Query::step_chain` matches a chain right to left with Lexbor's
`find`/`found_check`/`not_found` loop: a failed compound backtracks to the
nearest descendant or subsequent-sibling combinator to its right
(exhaustive, like Lexbor's). A `:is()`/`:where()`/`:not()`/`:has()`/`of S`
is a nested context - a `Task` on a heap stack, Lexbor's
`lxb_selectors_nested_t` - that the loop returns to with its verdict. The
rightmost compound is tested before any of that is set up, so most
candidates cost one compound test.

An `:is()`/`:where()`/`:not()` whose every alternative is one compound with
nothing nested in it (`Step::inline`, decided by `compile`) is answered in
place, with no task: `li:not(.r1)` went from 1.37x to 0.84x Lexbor's time.
It goes through `check_compound` rather than calling `check_simple` itself.
With a second caller, rustc stopped inlining `check_simple`, and every plain
scan got ~10% slower (`.item`, `main a`) - keep `check_simple`
single-caller.

**Do not bring back a general continuation machine for plain chains.** The
previous design (a frame stack plus a slab of parked continuations, every
compound a frame) was correct and allocation-free, and was still the whole
gap to Lexbor: 1.7-3.5x Lexbor's time on the Ruby-free probe, against
1.0-1.6x after this (`css` bench 4.0k -> 7.9k i/s).

### Scratch: a warm query allocates nothing

**Every table and stack a query uses is borrowed from a `Scratch`** that the
Ruby glue keeps in `css_cache`'s per-thread engine and hands back after the
query, as Lexbor keeps its entry/nested pools in `lxb_selectors_t`. So a
warm `css`/`at_css`/`matches?` allocates nothing but its result. The vectors
cross lifetimes empty, through `recycle` (std's in-place collect, which
reuses the allocation and can never allocate); one grown past
`SCRATCH_KEEP` items is dropped rather than kept. `matches?` was 1.2-2.9x
Lexbor's time before (the allocations - a `Small` spill, the stacks, the
compile work list - were most of a one-candidate call) and is 0.8-1.2x
after.

The stacks are cleared, not freed, between candidates. Every growth is
`falloc`'s (`MatchFailure::Oom` -> `Makiri::Error`), `rake oom`'s `css`
scenario reaches each site, and there is no `Box::new`/`Rc`/`vec!` in the
file.

### `#id` and `.class`

`#id` / `.class` read the element's `attr_id` / `attr_class` shortcut
(`HtmlElement::id_attr`/`class_attr`), as Lexbor does. These are the DOM's
ID and class attributes, the no-namespace ones: an unprefixed `id` set IN a
namespace is not the ID (`spec/attribute_dom_algorithms_spec.rb`). The
by-qualified-name lookup used before took it when it was the only `id`.

### One matcher, two DOMs, a browser's answers

`css_match` reads a DOM only through `css_match::dom`'s traits (`Host`,
`CssNode`, `CssElement`, `CssAttr`, `CssDoc`), named after the HTML arena's
handles. Their HTML impls (`html/dom/css.rs`) only forward, so the HTML
matcher monomorphizes to what it was; keep `bench:css` flat when touching
them. The XML arena's impls (`xml/css.rs`) resolve no names
(`Host::RESOLVES_NAMES`, bytes compared) and have no `id`/`class` shortcut.
They answer a type selector from the document node out of the XML name
indexes (`CssNode::candidates`; `LocalIndex` for a bare name in any
namespace), every candidate re-tested.

The rules follow a browser's `querySelectorAll` (`rake
conformance:css_browsers` holds them to Chromium/Firefox/WebKit):

- `CssNode::folds_case` - an HTML element in an HTML document - decides case
  folding and the 46-name value table, and `is_html` (the namespace) the
  form states. So an XHTML element in XML has `:checked` but compares `DIV`
  as written.
- A bare type selector matches in any namespace.
- A prefix comes from the ns Hash (`css_match::Namespaces`), resolved at
  compile. Its `"xmlns"` entry is the default namespace, which a compound
  with no type selector is kept to as well, bar an `:is()`/`:where()`/`:not()`
  argument's subject. An undeclared prefix is `CSS::SyntaxError`, except
  inside the forgiving `:is()`/`:where()`, which drop that alternative alone
  (`compile`'s `Nesting`).
- Ancestors outside the context node take part.

There is one `Scratch` per host, kept by `css_cache` beside the one
host-free AST cache.

### Tag and attribute name resolution

**Tag and attribute names are resolved to Lexbor's ids once per query**,
each the first time a candidate reaches it - Lexbor's own lazily set
`entry->id` (`css_match::simple::Name`, `HtmlDoc::resolve_attr_name`,
`HtmlElement::attr_by_resolved_name`). Lexbor keys element and attribute
local names by their ASCII-lower-cased form. So a tag-id match IS the
case-folded name comparison, and an attribute local-id match is a
pre-filter the adapter confirms with exactly `attr_by_name`'s comparison:
the answer never changes, only how many name reads it takes.

Ids are the document's own (custom elements and new attribute names are
interned per document), so they are resolved per query in the walked
document, and nothing document-specific is kept in `css_cache`. A tag id is
compared without asking each node for its document, as Lexbor's `entry->id`
is: every node a walking query reaches is in the walked tree, and Makiri
never moves a node between documents (an insert from another document
inserts a copy). The per-node `owner_document` read it replaced cost
`css("li")` ~5% (~10% on a document not in cache). An attribute id is still
confirmed by the adapter, which falls back to bytes for another document's
element.

`matches?` (one candidate) does not resolve, even lazily: a lookup costs
more than the one comparison it saves (`ul > li.item` 85 -> 117 ns when
tried). It compares the stored local name, one Lexbor call rather than the
two `dom_local_name` makes. An attribute whose Lexbor `qualified_name` is 0
is named by its lower-cased local name, so a local-id match confirms without
reading the name.

Whether a value compares case-insensitively by default (the 46-name table of
`lxb_selectors_match_attribute_html_case_insensitive`) is settled per
selector at compile time (`Step::value_ci`), as Lexbor's per-id `switch` is;
the per-node table scan it replaced cost `[type=text]` ~40%. A lone
top-level chain whose rightmost compound has a type selector refuses other
tags before the machine starts (`QueryPlan::refuses`, `css_match/plan.rs`).

Measured on the Ruby-free probe against the previous commit: attribute
scans -27 to -31%, type scans -12%, `matches?` -13 to -16%, `.class` and `*`
within +/-6%.

Name lookup matches Lexbor's except for two documented departures, both the
HTML Standard's case-sensitivity rule where Lexbor folds case on every
element:

- an attribute NAME is case-sensitive on SVG/MathML;
- a TYPE selector is compared as written to a non-HTML element's localName
  (lower-cased to an HTML one's), so `fegaussianblur` misses SVG
  `feGaussianBlur` (`resolved_names_agree_with_the_old_engine`,
  `type_selector_case_follows_the_element_namespace`).

The tag-id compare stays the first test; `written_case_holds` confirms it,
and reads no name on an element with no written name. Keep its slow half
out of line: inline, it cost every plain type scan 5-10%.

### `:nth-child(... of S)`

**`:nth-child(... of S)` is on the heap stack too** (`NthOfTask`). It used
to call a fresh `run` natively per sibling from `check_simple`, so `of S`
nested 300 deep raised `SystemStackError` in a 128 KiB Fiber and wedged the
shared CSS engine for the process.

It also counts by the CSS definition where Lexbor does not (a comma list, a
combinator or a pseudo-class in `S`; `css_match`'s module doc lists them).
So it is checked against a spec oracle
(`nth_child_of_s_agrees_with_a_spec_oracle`), and the Lexbor differential
fuzzer leaves `of S` out.

**Every deferred simple selector becomes a task in one place,
`Query::deferred_task`** - a new deferred kind goes there.

### `:has()` nesting

**`:has()` nesting is heap-based too, not just `:is`/`:where`/`:not`.**

An earlier version of this port answered `:has()` with ordinary Rust
recursion (`has_forward`), reasoning that `MAX_COMPOUNDS` (64) bounded it
safely. That holds for ONE `:has()` chain's own compound-by-compound depth,
but not for `:has()` NESTED inside another `:has()`'s argument.
`check_simple`'s `Has` arm called `has_matches` EAGERLY (unlike
`:is`/`:where`/`:not`, already deferred to a heap stack), so each nesting
level added a fresh native call chain with its own 64-deep budget, unbounded
by nesting depth.

The crash was confirmed as real and reachable: `:has(` × 300 `div` `)` × 300
against a 302-deep document, inside a `Fiber` given only
`RUBY_FIBER_MACHINE_STACK_SIZE`'s documented minimum (128 KiB), crashed the
WHOLE PROCESS with an uncaught `SystemStackError` that escaped every
`rescue` (exit code 1). That is worse than an ordinary raise, since Ruby's
stack-overflow handling `longjmp`s past every Rust frame between the
overflow and the nearest `rb_protect`, skipping `Drop` (`crate::stack`'s
module doc). `css_cache` borrows its engine cell, and the borrow's `Drop`
releases it; a `longjmp` mid-match would have left the cell borrowed
forever, wedging `Node#css`/`#at_css`/`#matches?` for the rest of the
process - not just crashing the one call.

Lexbor's own C engine never had this problem: `:has()`/`:is()` nesting is
handled by `lxb_selectors_nested_t`, a heap structure, not C recursion
(`lxb_selectors_run`'s loop). The fix works the same way:
`ForwardTask`/`HasCursor` (`css_match/query.rs`) answer `:has()`'s forward
search - candidate iteration over Descendant/Child/NextSibling/
SubsequentSibling, multi-compound stepping, and nesting - entirely on the
SAME heap task stack `:is()` uses. There is NO nesting-depth cap, matching
`:is()`'s own "verified safe at 500,000 levels" guarantee rather than adding
a new arbitrary limit.

Verified:

- The differential fuzzer
  (`agrees_with_the_old_engine_on_randomly_generated_selectors`, which
  generates nested `:has()`/`:is()`/`:where()`/`:not()` with all four
  combinators) still agrees with the OLD engine.
- `has_nested_inside_has_is_heap_based_not_native_recursion` reproduces the
  crash fixture on a 128 KiB thread stack and gets a correct answer, not an
  overflow.
- The original Ruby-level repro (small `Fiber`, depth-300 nesting) now
  raises a catchable `Makiri::Error` and leaves the CSS engine fully usable
  afterward, never `SystemStackError`. The error is `WorkExceeded`:
  `:has()`'s own Descendant search explores a genuinely large candidate
  space for this adversarial shape. That is the SAME pre-existing budget
  mechanic the old native-recursive design also relied on, just no longer
  reachable via a crash first.

Since C20 in `docs/parser_differences.ja.md`, the parser rejects a `:has()`
inside a `:has()`, as Selectors 4 does, so the public API no longer reaches
this shape. The matcher still answers it, and the tests that hold it to
Lexbor's engine parse as Lexbor did (`compat::as_lexbor`).

### Scope and the work budget

`select_all`/`select_first`/`matches_any` are **descendant-only** (context
node excluded, like Nokogiri) and in document order. `select_all` is capped
at `NODE_SET_MAX`, and every entry point shares one per-call work `Budget`
of 50M steps (XPath's `max_eval_ops`), charged by every compound tested,
`:has()` candidate, sibling counted and fieldset ancestor climbed.
Exceeding either raises `Makiri::Error`.

The budget counts a plain chain too, so what keeps ordinary queries far
below it is that no step is repeated needlessly:

- `step_chain` does not retry a left part that failed at every ancestor (or
  every preceding sibling) from a further one (`Fail`, Blink's
  `SelectorChecker` statuses); Lexbor's exhaustive backtracking is
  exponential in the chain.
- `:nth-child(... of S)` remembers the ranks it counted, as the plain
  `:nth-*` family remembers positions.

### Sibling positions

**A walking query remembers the sibling positions it has counted**
(`css_match::positions::Positions`, one memo per `:nth-*` kind). The scan
is charged to the budget, and counted afresh per candidate it made
`:nth-child` over a wide list quadratic in that budget: `tr:nth-child(odd)`
over ~4,500 rows raised "exceeded its work budget". A walk records every
counted sibling it passes and stops at the first known one, so each list is
walked once per kind (`nth_over_a_wide_list_costs_work_linear_in_the_list`);
the memo is dropped and refilled past `POSITIONS_MAX` entries. `matches?`
keeps none.

### Errors found before matching

A malformed selector raises `Makiri::CSS::SyntaxError` from the parse step,
before matching starts. `:current()` and the column combinator (`||`) are
constructs this matcher cannot evaluate (deliberately, and because Lexbor's
own traversal can't run the latter either). They raise "could not be run",
never answer a silent empty result. `:lexbor-contains()` never gets this far:
the parser rejects it as an unknown function.

**Both these constructs and the 64-compound chain cap are decided by
`css_match::compile`, over the WHOLE selector tree, before any node is
matched.** Every entry point (`select_all`/`select_first`/`matches_any`)
compiles first, and `matches?` on a non-element compiles too. Found lazily
mid-match, they depended on evaluation order:

- the old `collect_compounds` returned `None` for an over-cap chain exactly
  as for an empty one, so `:not(<65 compounds>)` lost its only alternative
  and matched EVERY element;
- `p, x || y` answered the `<p>`s because `p` matched first.

`compile` walks nested lists on an explicit work list, not recursion. A
recursive first version turned `:is()` nested 2000 deep into a
`SystemStackError` in a 128 KiB Fiber, which wedged the shared CSS engine
for the rest of the process (`spec/native_stack_guard_spec.rb` pins the
Fiber case).

### Form state and links

**Form state and links follow the HTML Standard.** `:disabled` / `:enabled`
/ `:checked` / `:read-write` are the Standard's definitions (fieldset
inheritance, first `legend` element child, `option`/`optgroup`, the `input`
types `readonly` applies to, `contenteditable`), which Lexbor only
approximates. `:any-link` / `:link` are an HTML `a`/`area` or an SVG `a`
with an `href`, and `:empty` ignores comments and processing instructions,
as browsers have them (M2, M9, M10 in `docs/parser_differences.ja.md`;
`compat::as_lexbor` runs Lexbor's).

### A per-thread engine

**The CSS engine is per thread** (`css_cache`'s `thread_local!`). A query
never releases the GVL, so each thread's engine is reached by one query at a
time, whichever Ractor the thread is in. Being the thread's alone, it needs
to be neither `Send` nor `Sync` - its kept stacks hold DOM node handles,
which are neither. A re-entrant second borrow on one thread is
`ParseError::Busy` (`try_borrow_mut`), never a second `&mut`.

The reference builds' engines (`css_parser`'s and the OLD `selectors`'s)
stay process-global in a `crate::gvl::GvlCell`: a `Mutex` (`try_lock`, so a
re-entrant borrow is `Busy`, not a deadlock), keyed by a `Gvl` token that
`Gvl::exclusive()` makes outside Ruby. The shipping build has no such cell.
Both used to be `UnsafeCell`s that only the GVL made sound.

## Serialization

`Node#{to_html,to_s,outer_html}` and `#inner_html` are one iterative walk
over the arena (`write`, in `html/dom/serialize.rs`), byte for byte what
Lexbor's serializers write but for the documented departures (R1, R2). The
`html_diff` fuzz target (`tree_diff::check_serialization`) holds it there:

- a `<template>` writes its CONTENTS, not its own children, as the HTML
  Standard's fragment serializing does;
- text is written raw only inside the elements the Standard names
  (`raw_text`);
- the void test reads the tag id (`shape`).

`pretty: true` is the same walk in Lexbor's pretty layout (not the
Standard's; it quotes text nodes).

### The output buffer

Output goes to a `Sink` chunk by chunk. The shipping one is a
`ceiling_buf::Buf`, pre-reserved only for the document or its root element
(sizing reads the whole arena, so a subtree grows from empty), and copied
into one UTF-8 Ruby String at the end. That is faster than appending to a
Ruby String, whose growth is GC-tracked.

The ceiling (`ceiling`) is 64 KiB + 32x the bytes the document owns + nodes
x 4 x the longest name. A name is stored once however many nodes write it,
so 50 elements named by 20,000 bytes need the name term.

**The per-chunk append is the hot path** (a chunk is a few bytes, ~20 per
element). Profile these three before touching any of them:

- `Buf::append`'s inline fast path: a chunk that fits the room needs no
  ceiling check, since the room never passes the ceiling;
- its `memcpy`-free copy of <= 16 bytes (`copy_small`: two fixed-size
  copies, safe Rust over a `Vec` kept initialised to its room);
- the 64-byte first room (`FIRST_ROOM`).

The first two are what put `to_html` ahead of `nokolexbor`. The third is
what keeps a subtree's buffer, which grows from empty, ahead of the libc
buffer it replaced.

### Fragments, templates and text

A `DocumentFragment` serializes as its children. `#inner_html` and
`#inner_html=` special-case an HTML `<template>` to its contents fragment,
as the WHATWG DOM special-cases `innerHTML` alone. So `inner_html`,
`inner_html=`, `#to_html` and `content_fragment` agree, while
`children`/`content=` keep the element's own (empty) children.

`Node#text`/`#content` (`html_node::read::content`) is served from the text
index, with a direct iterative walk for nodes it does not cover
(fragments). For a Document it returns the **root element's** text (the
DOM's null is not what callers want).

## Mutation

Mutation is `glue/html_node/mutate.rs` over `html/dom/edit.rs`.

### Detach, never destroy

Tree edits (`add_child`/`<<`, `add_previous_sibling`/`before`,
`add_next_sibling`/`after`, `remove`/`unlink`, `replace`) **detach, never
destroy**: the arena owns node memory, and live Ruby wrappers may alias a
removed node. A move is detach-then-insert.

### Attributes follow the DOM's algorithms

**Every HTML attribute lookup and write is the DOM's algorithm**: a lookup
by qualified name, lower-cased on an HTML element
(`HtmlElement::attr_by_name` / `get_attribute`); a write by namespace +
local name (`HtmlDocMut::set_attribute` / `set_attribute_ns`). So
`svg_a["href"]` does not find `xlink:href`.

The `id`/`class` shortcuts that back CSS `#id` / `.class` move only through
`Document::link_attribute` and a `ShortcutRule` each caller names (the
parser's Lexbor rule, the DOM algorithms' `Take`, a copy's).

There is NO rename (`name=` was removed on both representations): the DOM
has none.

### Copies and fragments

Copies - `clone_node`, `import_node`, an insert from another document, a
fragment's import - are `html/dom/copy.rs` (`HtmlDocMut::import`,
`import_xml`). `DocumentFragment.parse(html)` (own backing doc) and
`Document#fragment(html)` (bound to a doc) parse in a throwaway document
(`html::fragment`) and import each child; inserting a fragment splices its
**children**.

### The insertion rules

The four structural verbs are one `bridge::html::insert(this, node, Place)`.
The rules - the WHATWG DOM's "ensure pre-insertion validity" - are written
ONCE, in `crate::dom_rules::check`, with the DOM's other rules both sides
must answer alike (`places_itself`, `text_content`, `removable`). They are
generic over a small `Tree` trait that both the arena (`HtmlTree`: a
template's contents reach it through `host`, an attribute has no tree
parent) and the XML arena implement. Each side keeps only its linking (and
XML its namespace resolution), and checks a fragment as one node before any
link changes. The placing is `HtmlDocMut::place`.

### The edit path

**Every edit goes through `bridge::html::edit` and then
`HtmlEdit::with_tree` (or `with_attributes` / `with_data`), which drops the
indexes** (`HtmlParsed::edit`) and lends the document as `&mut HtmlDocMut`
for a closure that runs no Ruby. So no mutator calls it, and none can forget
to on an error path. The factories, copies and fragment imports reach the
document through `bridge::html::new_node` (`HtmlNewNodes`, which keeps the
indexes: a detached node is in none). A `RawNode` is a reference INTO a
document and becomes a handle only through `HtmlDoc::node` / `checked` (or
`HtmlDocMut::node` inside an edit).

`edit` and `with_tree` are separate on purpose. `edit` checks (frozen), so
those errors come first, and every argument is converted BETWEEN the two,
because a conversion can allocate, and so raise, and must not do so once
the indexes are dropped. The XML side is the same pair, `begin_edit` and
`Editing::with_arena`.

Both take the receiver `&mut` for the edit's life. So a read of its document
through it (`HtmlSelf::read`, `XmlSelf::read`) still held when the edit
begins - one the edit's compaction or arena growth would invalidate - is a
compile-time borrow error; a read the edit itself needs goes through it
(`Editing::read`). Any other read still held (a copy of the receiver,
another node's) is the write guard's to refuse, at runtime (`bridge::dom`).

### All-or-nothing `inner_html=`

`inner_html=`/`outer_html=` are all or nothing: `stage_fragment_in` imports
into a DETACHED fragment first, and only then are the old nodes swapped out.
`rake oom`'s `html_inner_html` scenario checks the document is unchanged
after every injected failure.

## Ruby surface niceties

Node classes take the WHATWG DOM interface names: Document, Element, Attr,
Text, Comment, CDATASection, ProcessingInstruction, DocumentType,
DocumentFragment. They are mapped by DOM node type in `wrap_html_node` /
`wrap_xml_node` - one per representation, since the leaf classes are
`Makiri::HTML::*` and `Makiri::XML::*`. `CDATA` and `DTD` are
Nokogiri-compatible aliases, defined in Ruby (`lib/makiri/compat_aliases.rb`)
at all three scopes; there is no `Attribute`.

Conveniences:

- `Node#{root,ancestors,path}` (path round-trips through `#at_xpath`).
- `Node#{attributes,to_h}`.
- `Node#{search,at}` auto-detect CSS or XPath: an expression that starts
  `/ ./ .. .// ( @` or contains `::` ⇒ XPath, else CSS.
- `Document#{body,head,encoding,meta_encoding}` (`encoding` is the one the
  document was read in, "UTF-8" for one made).
- `NodeSet#{|,+,&,-}` are identity-dedup, encounter-order (**not**
  doc-order). `#{css,xpath,search}` run per node and union (returning
  `self` when empty, so an empty set stays a NodeSet). Also
  `#{last,at,remove}`.
- `Element.new(name, doc)` / `Text.new(content, doc)` delegate to
  `Document#create_*` (they override `.new`, since the allocator is
  undef'd).
