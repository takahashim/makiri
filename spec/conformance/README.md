# Conformance & differential testing

Harnesses that check Makiri against external standards, complementing the
robustness fuzzer (`spec/fuzz/`, which only checks that Makiri never does worse
than raise its own error). These check that Makiri is *correct*, not just safe.

The `rake conformance:*` harnesses below need network/Nokogiri and live outside
the unit suite (their files are not `*_spec.rb`). Two normal specs (pure Ruby,
no Nokogiri) run under `rake spec`: `wpt_domxpath_spec.rb` ("3. XPath over HTML")
and `css_selectors_spec.rb` ("4. CSS Selectors").

```bash
rake conformance:html5     # WHATWG HTML5 parsing  vs html5lib-tests
rake conformance:xpath     # XPath 1.0 evaluation  vs Nokogiri (libxml2)
rake conformance:css       # CSS Selectors         vs Nokogiri::HTML5
rake conformance           # all three
rake conformance:xpath_xml # XML XPath 1.0         vs Nokogiri::XML
rake conformance:xmlconf   # XML well-formedness    vs the W3C XML Test Suite
rake conformance:xml_pbt   # XML tree (PBT)         vs Nokogiri::XML (generated docs)

# pass through options:
H5_ARGS="--file tests1.dat --verbose"        rake conformance:html5
XPATH_ARGS="--generate 8000 --seed 1"        rake conformance:xpath
CSS_ARGS="--verbose"                         rake conformance:css
XMLCONF_ARGS="--verbose --show-policy"       rake conformance:xmlconf
PBT_ARGS="--count 50000 --verbose"           rake conformance:xml_pbt
```

Makiri-only property-based tests (round-trip + metamorphic, with shrinking) run
in the normal suite: `spec/xml_pbt_spec.rb` (raise `PBT_COUNT` to do more).

The Nokogiri baseline is **`Nokogiri::HTML5`** (Gumbo, WHATWG-compliant), never
`Nokogiri::HTML` (libxml2's non-conformant HTML4 parser). Nokogiri is a
bench-only dependency, so `conformance:xpath` runs outside the bundle, like
`bench`.

---

## 1. HTML5 parsing — `html5lib_runner.rb`

Runs the WHATWG [html5lib-tests](https://github.com/html5lib/html5lib-tests)
`tree-construction` suite through `Makiri::HTML(...)` and string-compares
Makiri's DOM (rendered by `html5lib_dump.rb`) against each test's expected tree.
Lexbor passes this suite upstream; running it through Makiri verifies the
post-parse layer (attr→owner index, source-location stamping, Ruby wrappers)
does not perturb the tree.

Test data is **not vendored**: the runner fetches a pinned master commit of
html5lib-tests via a sparse, blobless clone into `data/` (gitignored) on first
run. Bump `DATA_COMMIT` in the runner to update.

**Scope:** full-document AND fragment (`#document-fragment`) tests. Fragment
tests parse in their context element via the Nokogiri-compatible `context:`
(String for HTML / `"svg"` / `"math"`, or a Node for foreign non-root contexts
like an SVG `<desc>`). Only `#script-on` tests are skipped (scripting changes
parsing and is out of scope). A test exercising a DOM detail Makiri cannot
represent at all would be reclassified as *unsupported* rather than scored as a
failure; in practice that count is 0.

### Result

```
ran 1770  pass 1770  fail 0  error 0   (100.00% of ran)
skipped: 8 script
unsupported: 0
```

**Makiri's HTML5 parsing matches html5lib-tests exactly** — every full-document
and fragment test its Ruby API can represent passes. The only tests not run are
the 8 scripting-on ones (out of scope).

---

## 2. XPath 1.0 — `xpath_diff.rb`

Makiri's XPath engine is original code (no libxml2 anywhere), so it has no
mature implementation backing it — making a differential against libxml2 (via
Nokogiri) its highest-value correctness net. Both sides parse with their HTML5
frontend, so the DOM trees are isomorphic and matched nodes are compared by
absolute path.

- **Corpus:** a curated, deterministic set (`xpath_corpus.rb`) covering every
  axis, the node tests, position/predicate semantics, the function library, and
  the operators — plus optional grammar-generated expressions
  (`--generate N --seed S`, reusing the fuzzer's grammar).
- **Comparison:** node-sets by set-of-paths (order tracked separately, since
  XPath node-sets are formally unordered); scalars by value (numbers within
  1e-9, NaN==NaN).
- The Nokogiri DOM is used **unmodified**: Makiri's strict-by-default namespace
  matching now lines up with Nokogiri::HTML5 (`//path` matches nothing on both),
  so no `remove_namespaces!` neutralisation is needed.

### Result

```
curated:                  ~1089 pairs, 0 divergences
generated (8000, seed 1): ~69700 pairs, 0 result/raise divergences
```

Buckets that are tallied, not scored as bugs:

- **`noko-strict`** — Makiri evaluates a *top-level* `position()`/`last()` (the
  document-root context position/size is 1) where libxml2 raises a syntax error.
  Makiri's behaviour is defensible per XPath 1.0.
- **`ns-repr`** — `namespace-uri()` of an HTML element is the XHTML URI in
  Makiri (DOM-correct, what browsers report) but `""` in Nokogiri::HTML5, which
  drops the HTML namespace. A representation difference where Makiri is the more
  correct side; not a bug.

### Findings

1. **Namespace matching — now strict by default (resolved).** Makiri's
   unprefixed element name tests resolve in the HTML namespace: `//div` matches,
   but `//svg` / `//path` do NOT — foreign content needs a registered prefix
   (`//svg:path`), exactly like browsers' `document.evaluate` and
   `Nokogiri::HTML5`. The old namespace-agnostic behaviour (where `//path` finds
   the SVG element) is available per-call/per-context via
   `namespace_matching: :lax`. This made the differential agree without
   `remove_namespaces!`; the only residual is the `ns-repr` bucket above.

---

## 3. XPath over HTML — `wpt_domxpath_spec.rb`

A hand-port of the evaluation-semantics subset of Web Platform Tests'
[`domxpath/`](https://github.com/web-platform-tests/wpt/tree/master/domxpath)
suite — the browser `document.evaluate` tests, the de-facto reference for XPath
over the HTML DOM. Unlike the harnesses above this is a plain `*_spec.rb` (pure
Ruby, no Nokogiri), so it runs under `rake spec`.

Ported: element/attribute name tests (incl. namespaces, non-ASCII names),
attribute parent axis, numeric/boolean operators, lexical structure (quotes &
whitespace), node-set operators, predicates, document order, and the string
functions. Out of scope: the DOM Level 3 XPath *API* (XPathEvaluator / resolver
/ XPathResult), Shadow DOM, cross-realm, crash tests.

Browser behaviours Makiri intentionally does not match are `pending` with a
reason (a pending example that starts passing fails the run): ASCII case-folding
of HTML name tests (Makiri and Nokogiri::HTML5 are case-sensitive), and hiding
`xmlns` from attribute tests.

---

## 4. CSS Selectors — `css_diff.rb` + `css_selectors_spec.rb`

Makiri's `Node#css` is backed by **Lexbor's selector engine** (mature,
upstream-tested), not original code, so unlike XPath the matching itself is not
the main risk. These check (a) Makiri's glue — descendant-only scope, document
order, comma de-duplication, error mapping — and (b) where Lexbor's and
Nokogiri's *supported-selector vocabularies* differ.

- `css_diff.rb` (`rake conformance:css`): differential vs `Nokogiri::HTML5#css`
  over a standard-selector corpus (`css_corpus.rb`). On standard selectors the
  two agree exactly; the differences are vocabulary only and are bucketed:
  `lexbor-only` (Level-4 `:is`/`:where` Nokogiri rejects), `nokogiri-only`
  (jQuery extensions `:contains`/`:gt`/`:eq`/`:first` — non-standard, Makiri
  rejects by design), `agree-reject` (pseudo-elements / invalid).
- `css_selectors_spec.rb`: a resident pure-Ruby spec pinning the supported
  surface (type/universal/class/id, all combinators, every attribute operator,
  structural pseudo-classes, `:not`/`:is`/`:where`/`:has`, grouping) and the
  glue semantics, plus the deliberate non-support of jQuery extensions.

### Findings

1. **jQuery/Nokogiri CSS extensions are not supported (by design).**
   `:contains`, `:gt`, `:lt`, `:eq`, `:first`, `:last`, … are not standard CSS;
   Lexbor rejects them. Use XPath (`xpath("//p[contains(.,'x')]")`) or
   Enumerable (`css('li')[1]`) instead.
2. **Type selectors are case-insensitive; class/id are not quirks-aware.**
   Lexbor matches type selectors ASCII-case-insensitively (CSS-correct for HTML;
   `LI` matches `<li>` — Nokogiri::HTML5 is wrongly case-sensitive here), but it
   matches class/id case-INsensitively regardless of the document's quirks mode,
   whereas a no-quirks document should treat them case-sensitively (browsers /
   Nokogiri::HTML5 do). That is Lexbor's behaviour (not patched); recorded as a
   `pending` in `css_selectors_spec.rb`.

