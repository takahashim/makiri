# Conformance & differential testing

Two harnesses that check Makiri against external standards, complementing the
robustness fuzzer (`spec/fuzz/`, which only checks that Makiri never does worse
than raise its own error). These check that Makiri is *correct*, not just safe.

Neither is run by `rake spec` (the files are not `*_spec.rb`). Both need
network/Nokogiri and so live outside the unit suite.

```bash
rake conformance:html5     # WHATWG HTML5 parsing  vs html5lib-tests
rake conformance:xpath     # XPath 1.0 evaluation  vs Nokogiri (libxml2)
rake conformance           # both

# pass through options:
H5_ARGS="--file tests1.dat --verbose"        rake conformance:html5
XPATH_ARGS="--generate 8000 --seed 1"        rake conformance:xpath
```

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

### Findings (Makiri Ruby-API gaps surfaced by the suite, both since FIXED)

1. **`<template>` content (110 tests) — now exposed (FIXED).** Per the HTML
   spec a template's contents live in a separate "template contents"
   `DocumentFragment`, not as children of the `<template>` element (browsers
   behave the same: `template.children` is empty, `template.content` holds the
   nodes). Lexbor stores them there; Makiri now surfaces the fragment via
   `Element#content_fragment`, so `tpl.content_fragment.css("p")` works and the
   dump renders the html5lib `content` pseudo-node. CSS/XPath over the template
   element itself still do NOT descend into the content (matching the DOM, and
   unavoidable for CSS since it runs Lexbor's unpatched selector engine over the
   real tree) — query the fragment instead.
2. **Doctype public/system identifiers (21 tests) — now exposed (FIXED).**
   Lexbor parses `<!DOCTYPE html PUBLIC "..." "...">` correctly; Makiri now
   surfaces it via `Makiri::DocumentType#{public_id,external_id,system_id}` and
   `Document#internal_subset`, so these tests pass. This was a DOM API
   completeness matter (WHATWG DOM `DocumentType.publicId`/`systemId`), NOT an
   XPath conformance issue — XPath 1.0's data model has no doctype node type at
   all (root/element/text/attribute/namespace/PI/comment only), so doctype
   declarations stay unqueryable by XPath by design (Nokogiri/libxml2 likewise).

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
- The Nokogiri DOM is run through `remove_namespaces!` so the diff compares pure
  evaluation semantics under one namespace policy — see finding #1.

### Result

```
curated:                 972 pairs, 0 divergences
generated (8000, seed 1): 69723 pairs, 0 result/raise divergences
```

Buckets that are tallied, not scored as bugs:

- **`noko-strict`** — Makiri evaluates a *top-level* `position()`/`last()` (the
  document-root context position/size is 1) where libxml2 raises a syntax error.
  Makiri's behaviour is defensible per XPath 1.0.

### Findings

1. **Namespace policy differs (by design, but worth knowing).** Makiri's
   `namespace-uri()` correctly reports the HTML/SVG/MathML namespaces per the DOM
   spec, but its XPath **unprefixed name tests match by local-name regardless of
   namespace** — so `//svg`, `//rect`, `//circle` find foreign elements.
   Nokogiri::HTML5 keeps HTML in the null namespace but SVG/MathML in their
   foreign namespaces, so strict XPath 1.0 requires registering the namespace and
   writing `//svg:rect`; bare `//rect` matches nothing. Makiri's choice is
   HTML-ergonomic but is **not strict XPath 1.0 namespace semantics**. (The
   harness neutralises this with `remove_namespaces!` so it can diff everything
   else.)

2. **Attribute document-order bug (FIXED).** XPath 1.0 §5.1 orders a node
   before its attribute nodes (`element < its @attrs < its children`). For a
   union that mixes elements and attributes, e.g.
   `.//descendant-or-self::circle | //attribute::*`, Makiri returned an
   attribute *before* its owning element:

   ```
   was:  .../circle/@r, .../circle          # @r before its element — wrong
   now:  .../circle, .../circle/@r          # element before @r  — per spec
   ```

   Same node *set*, wrong document *order* — surfaced only when element and
   attribute nodes shared a result set. Root cause was the small-node-set
   fallback comparator in `mkr_xpath_value.c` (`doc_order_cmp`), which also
   disagreed with the indexed comparator used for larger sets. Fixed; guarded
   by `xpath_corpus.rb` (the element+attribute union cases) and a unit test in
   `spec/xpath_spec.rb`.

---

## Extending

- **Scripting-on tests (8):** these require a scripting flag that changes
  parsing (e.g. `<noscript>` handling); out of scope unless Makiri models it.
- **CSS Selectors:** a sibling `css_diff.rb` against `Nokogiri::HTML5#css`, or
  the WPT selector suite, would cover the third query surface.
- When the differential finds and you fix a divergence, add a minimal
  reproducing expression to `xpath_corpus.rb` so it stays fixed.
