# Conformance & differential testing

This directory contains tests that check Makiri's correctness.
The tests here check that accepted input produces the expected DOM, XPath result, CSS result, or XML tree.

Most `rake conformance:*` tasks do not run as part of `rake spec`.
Some tasks fetch external test data. Others compare Makiri with Nokogiri/libxml2.
The pure Ruby specs listed below are part of the normal spec suite.

```bash
rake conformance:html5     # HTML tree construction vs html5lib-tests
rake conformance:xpath     # HTML XPath 1.0 vs Nokogiri::HTML5
rake conformance:css       # HTML CSS selectors vs Nokogiri::HTML5
rake conformance:xmlconf   # XML 1.0 well-formedness vs W3C XML Test Suite
rake conformance:xpath_xml # XML XPath 1.0 vs Nokogiri::XML
rake conformance:css_xml   # XML CSS selectors vs Nokogiri::XML
rake conformance           # all tasks above, excluding xml_pbt

rake conformance:xml_pbt   # generated XML trees vs Nokogiri::XML
```

Pass options through environment variables:

```bash
H5_ARGS="--file tests1.dat --verbose"        rake conformance:html5
XPATH_ARGS="--generate 8000 --seed 1"        rake conformance:xpath
XPATH_ARGS="--verbose"                       rake conformance:xpath_xml
CSS_ARGS="--verbose"                         rake conformance:css
CSS_XML_ARGS="--generate 5000 --seed 1"      rake conformance:css_xml
XMLCONF_ARGS="--verbose --show-policy"       rake conformance:xmlconf
PBT_ARGS="--count 50000 --verbose"           rake conformance:xml_pbt
```

Nokogiri is a bench-only dependency, so Nokogiri-based conformance tasks run
outside Bundler, like `rake bench`. Every conformance task depends on `compile`.

These conformance specs live in the normal suite:

- `spec/conformance/wpt_domxpath_spec.rb` - XPath over HTML, ported from WPT
  `domxpath/`.
- `spec/conformance/css_selectors_spec.rb` - the supported HTML CSS selector
  surface and Makiri's CSS glue rules.
- `spec/xml_conformance_spec.rb` - XML XPath regressions found by the XML
  differential.
- `spec/xml_pbt_spec.rb` - generated XML model round-trip and metamorphic
  properties. Set `PBT_COUNT` to run more cases. The default is 300.
- `spec/xml_css_pbt_spec.rb` - generated XML CSS self-consistency properties.
  Set `CSS_PBT_COUNT` to run more cases. The default is 300.

The expected result is zero real failures. `fail`, `error`, and `DIVERGE` counts
should stay at zero unless a case is explicitly listed in a documented policy or
vocabulary bucket. Some runners also print order-only differences. Treat those
as investigation signals, and follow the exit policy of that runner.

---

## 1. HTML5 parsing - `html5lib_runner.rb`

This runner uses the WHATWG
[html5lib-tests](https://github.com/html5lib/html5lib-tests)
`tree-construction` corpus. It parses each case with `Makiri::HTML(...)` or
`Makiri::DocumentFragment.parse(...)`. Then it dumps Makiri's DOM and compares
that dump with the expected html5lib tree.

The test data is not vendored. On the first run, the runner fetches a pinned
commit of `html5lib/html5lib-tests` into `spec/conformance/data/`. That directory
is gitignored. To update the corpus, update `DATA_COMMIT` in the runner.

Scope:

- Full-document tests use `Makiri::HTML`.
- Fragment tests use their `#document-fragment` context.
- HTML fragment contexts use `context: "tag"`.
- Foreign fragment contexts use a Makiri node context.
- `#script-on` tests are skipped because Makiri does not implement scripting,
  and scripting changes tree construction.
- If an expected node cannot be represented by Makiri's Ruby API, the case is
  counted as `unsupported`. The current corpus should keep this count at zero.

This runner mainly checks Makiri's Lexbor integration. It also checks that the
post-parse indexes, source-location stamping, and Ruby wrappers do not change
the HTML5 tree.

---

## 2. HTML XPath 1.0 - `xpath_diff.rb`

This runner evaluates the same XPath expression in Makiri and Nokogiri. Makiri
uses its original XPath engine. Nokogiri uses libxml2.

Both sides parse with an HTML5 parser: `Makiri::HTML` and `Nokogiri::HTML5`.
The trees should be isomorphic, so the runner compares node identity by using a
normalised absolute path.

Inputs:

- `xpath_corpus.rb` is the curated deterministic corpus. It covers axes, node
  tests, predicates, operators, functions, variables, namespaces, and error
  cases.
- `--generate N --seed S` adds grammar-generated expressions. This uses the same
  grammar as the fuzzer.

Comparison rules:

- Node-sets are compared by the set of normalised node paths.
- Order-only node-set differences are reported separately.
- Strings and booleans are compared by exact value.
- Numbers are compared with a `1e-9` tolerance. `NaN` is treated as equal to
  `NaN`.
- Makiri's fail-closed cases are tallied but not scored as bugs. This includes
  evaluation budget limits and the unimplemented namespace axis.

Documented non-bug buckets:

- `noko-strict`: libxml2 rejects some top-level `position()` and `last()` forms.
  Makiri evaluates them with document-root context position and size equal to 1.
- `ns-repr`: Makiri keeps HTML elements in the XHTML namespace, matching the
  browser DOM model. `Nokogiri::HTML5` reports the empty namespace.

HTML namespace matching is strict by default. An unprefixed element test resolves
in the HTML namespace. This means `//div` matches HTML elements, but `//svg` and
`//path` do not match SVG elements. Use a registered prefix for foreign content,
or pass `namespace_matching: :lax` for the legacy namespace-agnostic mode.

---

## 3. XPath over HTML - `wpt_domxpath_spec.rb`

This resident spec runs under `rake spec`. It is a hand-port of the
evaluation-semantics part of Web Platform Tests'
[`domxpath/`](https://github.com/web-platform-tests/wpt/tree/master/domxpath)
suite.

It covers:

- HTML and foreign element name tests.
- Namespace registration.
- Attribute tests.
- Attribute parent-axis behaviour.
- Numeric, boolean, and relational operators.
- Node-set operators, predicates, and ordering.
- String function semantics.
- XPath lexer rules for quotes and whitespace.

These browser-specific areas are out of scope:

- DOM Level 3 XPath API objects, such as `XPathEvaluator`, `XPathResult`, and
  resolver callbacks.
- Shadow DOM.
- Browser realm tests.
- Browser crash tests.

Known browser differences are marked `pending` with a reason. If a pending
example starts passing, RSpec fails the run. This keeps intentional differences
visible and also catches future behaviour changes.

---

## 4. HTML CSS selectors - `css_diff.rb` + `css_selectors_spec.rb`

Makiri uses Lexbor's selector engine for HTML CSS matching. This differential is
therefore mostly about Makiri's glue code and selector vocabulary differences.
The glue code controls scope, result order, de-duplication, and error mapping.
The vocabulary differences come from Lexbor and Nokogiri's CSS-to-XPath layer
supporting different selector sets.

`css_diff.rb` compares `Node#css` with `Nokogiri::HTML5#css`. It uses the
standard-selector corpus in `css_corpus.rb`. Fixtures are forced into no-quirks
mode because CSS class and id matching depends on quirks mode.

Vocabulary buckets are tallied but not scored as bugs:

- `lexbor-only`: standards selectors supported by Lexbor but rejected by
  Nokogiri. This includes some Selectors Level 4 forms.
- `nokogiri-only`: Nokogiri's jQuery-style extensions, such as `:contains`,
  `:gt`, `:lt`, `:eq`, `:first`, and `:last`. Makiri rejects these by design.
- `agree-reject`: both engines reject the selector. This is usually a
  pseudo-element or an invalid selector.

`css_selectors_spec.rb` pins Makiri's supported HTML CSS surface:

- type, universal, class, and id selectors.
- combinators.
- all attribute operators.
- structural pseudo-classes.
- `:not`, `:is`, `:where`, and `:has`.
- grouping.
- descendant-only context matching.
- document-order results.
- comma-list de-duplication.
- `at_css`.
- syntax-error mapping.

One Lexbor behaviour is recorded as pending. Class and id selectors currently
match case-insensitively even in no-quirks mode. Browsers and `Nokogiri::HTML5`
treat them case-sensitively in no-quirks mode.

---

## 5. XML well-formedness - `xmlconf_runner.rb`

This runner checks Makiri's native XML parser, `Makiri::XML`. Makiri does not use
libxml2. The runner compares Makiri's accept/reject result with the W3C XML
Conformance Test Suite, `xmlts20130923`.

The test data is not vendored. On the first run, the runner downloads the pinned
W3C zip into `spec/conformance/data/xmlconf/`. Use `--no-fetch` when the data
must already be present.

Makiri is an XML 1.0 parser. It is namespace-aware, non-validating, and
no-DTD-processing.

Scoring rules:

- `not-wf` tests must reject with `Makiri::XML::SyntaxError`.
- `valid` and `invalid` tests must accept when they do not depend on DTD-defined
  entities or validation.
- XML 1.1 and Namespaces 1.1 tests are skipped.
- non-namespace-mode tests are skipped.
- optional `error` cases are skipped.
- missing files are skipped.
- DTD-entity-dependent validation cases are skipped.
- Expected no-DTD differences are reported as `policy differences`, not
  failures. Use `--show-policy` to list them.

---

## 6. XML XPath 1.0 - `xml_xpath_diff.rb`

This runner compares Makiri's XML XPath engine with `Nokogiri::XML` and libxml2.
It uses documents and namespace maps from `xml_xpath_corpus.rb`.

The result comparison is the same as the HTML XPath differential. The node key is
different: XML node identity uses a namespace-free positional key, not rendered
`#path`. This avoids false differences when the two libraries print namespaced
paths in different ways.

Makiri's documented fail-closed paths are tallied. This includes evaluation
limits and the unimplemented namespace axis.

A real divergence is one of these:

- one side raises and the other side succeeds.
- both sides succeed but return different scalar values.
- both sides succeed but return different node sets.

After a fix, regressions found here should usually be pinned in
`spec/xml_conformance_spec.rb`.

---

## 7. XML CSS selectors - `xml_css_diff.rb`

This runner compares `Makiri::XML#css` with `Nokogiri::XML#css`.

Makiri lowers XML CSS selectors to its own XPath engine. Nokogiri lowers CSS to
libxml2 XPath. The curated corpus in `xml_css_corpus.rb` is the default gate, and
it should have zero real node-set divergences.

`--generate N --seed S` is for exploration. It is not part of the default gate.
Generated selectors can expose Makiri limitations, Nokogiri translation bugs, or
both.

Documented vocabulary buckets:

- `makiri-unsupported`: selectors Makiri rejects. Examples include `[a=v i]`,
  `*|attr`, some untyped of-type cases that cannot be represented safely in
  XPath 1.0, and jQuery extensions.
- `nokogiri-unsupported`: selectors Makiri supports and Nokogiri rejects.
- `agree-reject`: both engines reject the selector.

`spec/xml_css_pbt_spec.rb` is the resident Makiri-only companion. It checks XML
CSS self-consistency on generated XML documents with namespaces:

- bare and prefixed type selectors against a ground-truth walk.
- `at_css == css.first`.
- `matches?` membership.
- comma-list union semantics.

---

## 8. XML property testing - `xml_pbt.rb` + `xml_pbt_diff.rb`

`xml_pbt.rb` is the shared generator for well-formed XML documents. Each
generated document also has an explicit model.

It feeds two test layers:

- `spec/xml_pbt_spec.rb` runs in `rake spec`. It checks Makiri-only properties:
  parse equals the generated model, parsing is deterministic, `to_xml`
  round-trips, pretty serialization stays well-formed and preserves content, and
  basic XPath algebraic identities hold.
- `rake conformance:xml_pbt` runs `xml_pbt_diff.rb`. It compares Makiri's parsed
  tree and canonical XML output with `Nokogiri::XML` in strict, non-networked
  mode. Divergences are shrunk to minimal counterexamples.

`xml_pbt` is not included in the aggregate `rake conformance` target. It is a
scalable stress pass. Use `PBT_ARGS="--count ..."` to raise the document count
for release runs or investigations.
