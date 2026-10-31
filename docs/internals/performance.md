# Performance

**Makiri beats Nokogiri/libxml2 on every `rake bench` row.** Measured
against Nokogiri: parse ~4.6×, css ~10×, at_css ~7000×, `//tag` ~4×,
`//*[@id=…]` ~8×, `[@attr='v']` ~4.3×, attribute axis ~3×, serialize ~6×,
full-text extraction ~3.5×. **traverse** (children walk) used to be the one
row that only met Nokogiri, within measurement error; as of the v0.10.0 bench
it beats it too.

The `css` and `at_css` figures were remeasured after HTML CSS matching moved
from Lexbor's engine to `css_match`, and again after the per-candidate
allocation fix and the move to Lexbor's own loop (see "CSS matching" below).
The `~12×` / `~9400×` figures an earlier revision of `CLAUDE.md` quoted were
the OLD engine's.

## How precise the numbers are

Treat these as indicative, not precise. Two consecutive runs on the same
machine put full-text extraction at 2.9× and 3.5×, and threaded parse scaling
at 2.4× and 1.7×; benchmark-ips reports ±18-27% on the heavier rows. The
numbers are good for catching a *regression in kind* (a row falling to parity
or below), not for defending a second decimal place.

## Threads and Ractors

Parsing scales across threads (~1.7-2.4× on 8 cores) because it releases the
GVL. XPath does not scale across threads, by design, because it holds the GVL
(see below). It does scale across Ractors, each of which has a GVL of its
own: on an M-series Mac's 4 performance cores, an XPath+CSS loop ran 3.8x the
throughput in 4 Ractors, against 0.9x in 4 threads.

## Decisions not to regress

### The GVL

**Parsing releases the GVL; XPath evaluation does NOT**
(`bridge::html::document`, `glue/query.rs`).

Parse copies the source to a C buffer, then runs `parse_html` under
`rb_thread_call_without_gvl2`. Releasing the GVL there is safe because a
freshly parsed document is not yet shared, so it can't race anything. The `2`
variant is deliberate. The plain one raises a pending interrupt (`Timeout`,
`Thread#raise`) as it returns: a longjmp over the Rust frames holding the
parsed document and the source copy, which leaked both on every interrupted
parse. `bridge::gvl::without_gvl` delivers an interrupt only when the body
never started, under `protect`.

Two checks keep Ruby out of the released body:

- **The `Send` bound.** It keeps the GVL token and every Ruby value out of the
  body. The token is `&Ruby`, which only a method's trampoline and `Init_`
  make, every callee being handed it, so nothing in the body can make one.
- **`rake dylint`'s `ruby_in_gvl_released_body`.** A Ruby-layer call that
  needs no token is not a type error (an `Error` made, for example, which
  reads its class only when raised). So the lint still holds the body's
  closure to calls into the engine or a library, by resolved path. A call
  through a function value, whose target no one can see, is refused too;
  hoist it out of the body.

What neither can see is a function pointer the engine calls into Ruby's C.

**XPath holds the GVL for the whole evaluation, by design**
(`xpath::ctx::Session::evaluate` is a plain GVL-held call). The engine and DOM
are not thread-safe against concurrent mutation, and holding the GVL makes
that safe *by construction*. The GVL serialises all C code of its Ractor's
threads, and a Document and every context over it belong to one Ractor. So an
XPath walk never runs in parallel with a tree mutation, with another
`evaluate` on the same context, or with a `register_variable` /
`register_namespace` / `node=` on the same context. No locking is needed, and
none is used.

An earlier version released the GVL for XPath, and scaled queries ~3.3×
across threads. The locking required to make a GVL-released walk safe against
mutation of a shared document was judged not worth the verification burden.
Single-thread XPath speed is unaffected by holding the GVL. **Do not
reintroduce a GVL-released XPath path** without the full document/context
locking story. Parse still scales (~2× on 8 cores). Verify with `bench`'s
threaded rows and the `GC.stress` `spec/threading_spec.rb`, which now also
asserts that shared-document XPath+mutation and shared-context evaluate are
crash-free under the GVL.

**The price is that an evaluation does not see interrupts.** `Timeout`,
`Thread#raise` and a signal handler wait for it to return, and so does every
other Ruby thread; `rb_thread_check_ints` runs only on the GVL-released parse
path's retry. The op budget bounds the wait: about 0.3 s at the 50M default,
the worst a pathological `following::` / `preceding::` nesting reached. CSS
finished instantly on the same documents.

**A check inside the engine was weighed and not done**, because of what it
would change:

- A check that yields the GVL runs other threads while the evaluation holds
  its read guard, so their edits are refused as `Makiri::Error`, where today
  they queue behind the GVL.
- A check that restarts the evaluation instead is set off by the time-slice
  timer (`rb_thread_check_ints` cannot tell it from a `Timeout`), so an
  evaluation over ~100 ms with another thread running would never finish.
- The poll itself, `rb_thread_interrupted`, has a path to a longjmp in the
  raise survey.

magnus #104 is the same wait, for a worker that never shut down. If the window
ever matters, the lever is a smaller budget, not a poll.

A poll WAS built and withdrawn. `rb_thread_interrupted` reports pending
exceptions and traps, not the time-slice flag a waiting thread raises, so a
`Timeout` or `Thread#raise` from another thread still waited, and only signal
handlers were delivered.

### `//tag` from the element index

`//tag` is served from the element index (`xpath/step_index.rs`): a
document-rooted, predicate-free, unprefixed descendant name-test pushes the
tag bucket instead of walking. This is pure-HTML only (the `has_foreign`
guard), and each candidate is re-checked with the step's `CompiledTest`, so
the result is identical to the walk. Custom and unknown tag names fall
through. See the element index in `subsystems.md`.

### CSS matching: per query, not per candidate

HTML CSS matches over the typed adapter, not Lexbor's own matcher
(`css_match`, see `subsystems.md`), and **allocates per query, not per
candidate**.

Right after the switch it was ~6x slower than Lexbor's C matcher PER
CANDIDATE NODE. That was measured with a Ruby-free in-process probe on the
same parsed document, and the ratio was the same for `css`'s whole-tree scan
and `at_css`'s first match. For every element visited, the chain was
re-collected into a fresh `Vec` and `run` got a fresh frame stack, and every
descendant/`~` retry boxed a continuation. Lexbor parses once into an arena
and resets an object pool per candidate. The fix has the same shape
(`Compiled`/`Query`, `subsystems.md`): 0 allocations per extra candidate. The
compiled-selector cache was tried first and moved nothing, because
`at_css('#main')`'s parse was never the cost.

On the same Ruby-free probe, the fix took the time relative to the OLD engine
(new/old) as follows:

| Selector | Before the fix | After |
| --- | --- | --- |
| `ul li.item` | 4.5 | 3.1 |
| `:is(li, p) > a.link` | 5.1 | 2.7 |
| `li:has(> span.meta)` | 6.3 | 3.2 |
| `at_css('#main')` | 4.9 | 2.8 |
| `at_css('#bot p')` | 3.6 | 2.4 |

`matches?` on one candidate stays ~1.8, since the per-query compile is its
whole cost.

The gap left was frame dispatch: every compound of every chain went through
the continuation machine. With Lexbor's loop instead (`subsystems.md`), the
probe's new/Lexbor ratio went from 1.7-3.5 to 1.0-1.6 across `li`, `.item`,
`[data-id]`, `ul li.item`, `main a`, `li > a.link`, `:is()`, `:not()`,
`:has()` and both `at_css` rows. Reusing the tables and stacks across calls
(`Scratch`, `subsystems.md`) came after that. In `bench`, each step gave:

| Step | `css` (i/s) | `at_css` (i/s) |
| --- | --- | --- |
| Before the allocation fix | 2.36k | 1.35M |
| Allocation fix | 4.01k | 1.75M |
| Lexbor's loop | 7.95k | 2.02M |
| `Scratch` | 8.38k | 2.70M |

With Lexbor's loop that was ~6.6x / ~2.9x nokolexbor (`css` / `at_css`) and
~10x / ~5000x Nokogiri; with `Scratch`, `at_css` was ~3.8x nokolexbor.
nokolexbor's own number swings run to run, so judge by makiri's absolute i/s.

Verify with `bench`'s `at_css`/`css` rows, the differential fuzzer,
`rake oom` and `FUZZ_ARGS="--target css" bundle exec rake fuzz:sanitize`.

### The compiled-selector cache

`Node#css`/`#at_css`/`#matches?` do not re-parse a repeated selector string
(`css_cache`). The cache is the OLD engine's adaptive `CachePolicy` (same
constants, same flush-on-cap/bypass-on-low-hit-rate/retest behavior), now
holding the parser port's owned ASTs, where it used to hold lists in a Lexbor
arena of its own.

It matters more than it did. The port parses at 1.5-2.5x Lexbor's time
(0.25-1.1 µs a selector), because it allocates its stacks per parse where
Lexbor reuses one parser. A cache hit hides that, so only the per-call paths
pay it: one-off selectors, ~5-10% on a uniquely-named `at_css`. XML's CSS
shares the cache, and has its own `Scratch` and `Kept`. Reusing the parser's
work space was measured in stage 9 and not taken (~4% on an uncached call;
`docs/rewrite_parser.ja.md`).

Verify with `spec/css_selector_cache_spec.rb` (flush, bypass/retest, and
reject-without-disturbing-the-cache) and the Rust-level `css_cache::tests`.

### `Node#text` from the text index

`Node#text` is served from the text index (`html/dom/index.rs`, see
`subsystems.md`): a per-document map, built lazily and dropped on edit, from a
node to its document-order run of text spans. It turns repeated text
extraction into a lookup and one pre-sized memcpy instead of a walk (~4×
libxml2).

The index is built once the walks it would replace add up to half the
document, so first and one-off calls walk: a first `doc.text` is 2x faster,
and a first small `li.text` goes from a whole-document build to nothing. Do
not regress to walking on the indexed path. Verify with `bench`'s "full
document text" row and `spec/text_index_spec.rb`, which asserts byte-identity
with a plain walk across subtrees and mutations.

### The hashed string-value cache

The string-value cache is hashed (`xpath/str_cache.rs`): a token-keyed
`PtrMap` index over an ordered store, so per-node predicate compares are O(1),
not the old O(n²) linear scan. The cache belongs to one evaluate
(`xpath::eval::Evaluation`), as do the op budget and, for XML, the
document-order index. HTML's document-order index is the document's
(`DocOrder`, in `subsystems.md`), which is safe because no evaluate can edit
the document it reads.

### Borrowed node string-values

Node string-values are borrowed when they are one slice (`xpath/value.rs`
`node_string_value` -> `Str`, `NodeText::Borrowed`). An attribute, a
text/CDATA/comment/PI node, and a container whose descendants hold one
non-empty text (`<li>item 5</li>`) are read in place; only a value joined from
several texts is built.

Leaves and attributes skip the cache, because a read is cheaper than a
lookup. A container's value is still cached, borrowed or built, because its
walk is charged to the op budget and a node-set comparison must not pay it
again. Not caching borrowed ones measured ~13% faster on `//li[. = 'x']`, but
would re-charge that walk per pair.

`max_cache_bytes` counts a borrowed entry by its length, and a borrow is held
to `max_string_bytes` exactly as a build is (`Buf::content_limit_for`).

### The `[@name]` predicate fast path

`[@name]` / `[@name='lit']` predicates take a direct-attribute fast path
(`match_attr_pred`/`attr_pred_matches` in `xpath/attr_pred.rs`): a
position-independent scan of the element's attributes with the axis's own name
test, instead of building a throwaway node-set per candidate. Anything else
falls through to the generic evaluator.

### `at_xpath`'s first-match path

`Node#at_xpath` short-circuits at the first match (`xpath/eval.rs`
`try_first_match`, entered via `evaluate_first`). `at_xpath` wants only
node-set[0]. So for the common "first descendant by name, with or without a
position-independent `[@a]`/`[@a='v']` predicate" shapes - `//x`, `//x[@a]`,
`//*[@a='v']`, `.//x`, `descendant::x[...]` (after the `//` peephole; one or
two steps) - it walks the subtree in **document order and stops at the first
match** instead of materialising and sorting the whole set. This is the XPath
analogue of at_css's `MATCH_FIRST`.

The cost becomes O(position of first match): a front hit is ~µs (against
~280µs for a full evaluation), and trailing or absent matches fall back to a
full scan. The path reuses the step's `CompiledTest` and the `[@attr]`
matcher, so it is **byte-identical to `xpath(e).first`** (asserted by
`spec/at_xpath_first_spec.rb`).

Anything else (positional predicates, functions/variables, reverse axes,
unions, prefixed attribute predicates, longer paths) is not recognised
(`first_shape` answers `None`) and goes to the full evaluator; a prefixed name
test is recognised and resolves its prefix exactly as the step driver does.
Only `at_xpath` uses this path; `xpath` always builds the full set.

### The per-context compiled-AST cache

An `XPathContext` (`bridge/xpath/context_object.rs`) parses each expression
once and re-runs the cached AST (bounded by `AST_CACHE_MAX`). `Node#xpath`
uses a throwaway context and does not cache.

### GC accounting for every Document

Every Document reports its arena to the GC (`account_document`;
`new_html_document` / `new_xml_document` call its `account_data` through the
new Document's handle, before the Document is shown). The free callback takes
the report back, through `Payload::reported_external`.

Neither arena, HTML or XML, is an `xmalloc`, so without the report Ruby sees a
parsed Document as ~56 bytes, and memory pressure triggers NO collection.
`500.times { Makiri::HTML(html) }` ran with zero GCs, 2.2 GB RSS, and every
parse faulting fresh pages: the parse bench read 1.8× slower than nokolexbor
at ±43% variance, and nothing failed. With the report it is at parity. The
diagnostic is `GC.count` across a parse loop (must rise) and `minflt` per
parse (near 0 once warm).

**One way in.** A Document gets its arena ONLY through
`bridge::wrapper::new_html_document` / `new_xml_document`. The wrapper is
allocated first, because a Ruby allocation can raise, and a raise would leak a
parse result already held. Then the caller's closure builds the content
(`Box<HtmlParsed>` / `Box<XmlDoc>`), which is stored and reported in one step,
so a new parse entry cannot skip the report. `spec/gc_accounting_spec.rb` pins
both halves. The report is what the document OWNS
(`HtmlParsed::external_bytes`, the XML arena's `memsize`).

**Growth through mutation** is re-reported where an edit begins
(`account_growth`, from `bridge::html::edit` / `bridge::xml::begin_edit`),
once it reaches an eighth of the last report (and a floor). For HTML an O(1)
measure (`HtmlParsed::arena_chunks`) decides before the bytes are measured
again. A growth no edit follows - a factory's node, an imported copy, a parsed
fragment, an adoption's copy into the receiver's document - is reported right
after its loan instead (`lend_document`, `fragment_into`, `import_copy`, both
`insert`s). A document's last edit may go unreported, which under-reports: the
safe direction. Without growth reporting, 60 documents grown by `inner_html=`
ran with zero GCs to ~700 MB RSS; with it, ~200 MB.

### Node size and tree walks

Tree-walk speed was structurally capped by Lexbor's 96-byte node. The arena
DOM that ships is ~109 B a node all in (byte store and names included, against
Lexbor's ~180 B) and walks children ~13% faster. Nodeset-pool and prefetch
follow-ups were investigated and **not** shipped: with no remaining
slower-than-Nokogiri row, they would add lifetime and speculative complexity
for no measurable win.

Stage 9's measurements (arena against Lexbor's DOM, row by row, and why the
selector parser's work space is not reused) are in
`docs/rewrite_parser.ja.md`. Measure with min CPU time over repeated runs,
since benchmark-ips swings 15-25% on this kind of machine.

## Working on performance

Capture a `rake bench` baseline, change one thing, re-bench, and **ship only a
measurable win** that keeps `rake spec` + `rake fuzz:sanitize` green.
