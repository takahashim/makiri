# Makiri Design Document

This is the English version of the design document; the Japanese version,
`docs/design_doc.ja.md`, is authoritative.

## 1. Basic Policy

### Goals

* Provide a Nokogiri-compatible API for common HTML5 scraping.
* Use Lexbor for fast HTML5 parsing.
* Implement an XPath 1.0 engine without libxml2.
* Use Lexbor for CSS parsing and matching where possible.
* Support source locations and basic DOM mutation.
* Make XPath evaluation resistant to memory exhaustion and DoS attacks.
* Use UTF-8 internally.

  * HTML parsing is lenient and replaces invalid bytes with U+FFFD.
  * Program APIs require valid UTF-8 and fail with `Makiri::Error`.
  * XML always rejects NUL.
* Parsing releases the GVL and can run in parallel.
* XPath evaluation keeps the GVL to avoid unsafe concurrent access.
* Internal errors should not crash the whole host process.
* Test robustness with fuzzing and curated differential specs.

### Out of Scope

* Full XML support, including XML 1.1 and DTD processing.
* XSLT, Schema, RelaxNG, XPointer, and XInclude.
* Streaming parsing.
* SAX or pull-parser APIs.
* Full libxml2 API compatibility.
* Drop-in compatibility with every Nokogiri method.

### Things We Intentionally Avoid

* Forking Lexbor. Makiri handles bugs in its adapter layer where possible and
  reports them upstream.
* Treating libxml2 compatibility as the main goal. When behavior conflicts
  with HTML5 or XPath specifications, Makiri follows the specification and
  documents the difference.

## 2. Ruby API Surface

Top-level API:

```ruby
Makiri.HTML(source)
Makiri::HTML(source)
Makiri.parse(source)
Makiri.XML(source)
Makiri::XML(source)
```

Main classes:

```text
Makiri::Node
├── Makiri::Document
├── Makiri::DocumentFragment
├── Makiri::Element
├── Makiri::Attr
├── Makiri::Text
├── Makiri::CDATASection
├── Makiri::Comment
├── Makiri::ProcessingInstruction
└── Makiri::DocumentType

Makiri::NodeSet
Makiri::XPathContext
Makiri::XML::Builder
```

Node class names follow WHATWG DOM interface names. Nokogiri-compatible names
such as `CDATA` and `DTD` are aliases. Common methods include navigation,
XPath, CSS selectors, text access, serialization, mutation, and source
location.

Input errors are subclasses of `Makiri::Error`:

```text
Makiri::Error
├── Makiri::XPath::SyntaxError
│   └── Makiri::XPath::LimitExceeded
├── Makiri::CSS::SyntaxError
├── Makiri::XML::SyntaxError
└── Makiri::XML::LimitExceeded

Makiri::InternalError
```

`InternalError` is separate from `StandardError` because it represents an
internal invariant failure, not a normal input error.

`XPathContext` caches parsed XPath expressions. XPath evaluation keeps the
GVL, so operations on the same context and document are serialized.

## 3. UTF-8 Text-Input Contract

Makiri uses UTF-8 internally, and treats input in one of two ways.

HTML parsing and fragment parsing are lenient:

* invalid bytes are replaced with U+FFFD
* parsing continues

Program APIs are strict:

* strings must be valid UTF-8
* invalid input raises `Makiri::Error`
* Makiri does not silently repair API input

NUL is rejected in names, namespaces, selectors, XPath expressions, and
engine input. HTML text, comments, and attribute values may contain NUL when
DOM behavior requires it. XML always rejects NUL.

## 4. Memory Model

- DOM nodes are allocated in a document-level arena.
- Removing a node only detaches it. It does not destroy the node while Ruby
  wrappers may still reference it.
- Ruby wrappers keep their document alive.
- Indexes and caches belong to the document and are invalidated after
  mutation.
- Temporary XPath caches belong to one evaluation so nested evaluations
  cannot affect outer evaluations.
- Allocation failure in the engine must become an error instead of calling
  `abort`.
- Parsing releases the GVL after copying input outside the Ruby heap.
- XPath, CSS, serialization, and normal DOM access keep the GVL to prevent
  unsafe concurrent access.
- Memory safety is tested with `GC.stress` and GC compaction.

## 5. Build System

- Build decisions are managed in one configure step.
- Lexbor is used without local patches or forks.
- Prefer release tags. Use a commit pin only when Makiri needs an unreleased
  upstream fix.
- Generate Lexbor structure information and constants from upstream headers
  instead of copying them manually.
- The extension should export only the symbols required by Ruby. Lexbor
  symbols must not be re-exported.
- Allocation APIs that abort are not allowed.
- Platform-specific optimizations are enabled only when they work on that
  toolchain and benchmarks show a real improvement.

## 6. Testing

### 6.1 Unit / Integration (`rspec`, `spec/`)

Test:

* parsing
* node access
* navigation
* mutation
* serialization
* encoding
* XPath
* CSS
* source locations

### 6.2 Robustness Fuzz (`spec/fuzz/`)

Run with:

```text
rake fuzz
rake fuzz:sanitize
```

The fuzzer generates and mutates XPath and CSS expressions. Invalid input must
not cause crashes, hangs, or unexpected foreign exceptions.

Nokogiri is not used as a general oracle, because Lexbor and libxml2 have
different HTML5 semantics. Known differences are recorded in curated specs
instead.

### 6.3 Property / Differential (`spec/invariants/`, `spec/differential/`)

Check invariants such as:

* namespaces
* tree structure
* index validity
* serialization
* text-input rules

Also compare results against a fixed historical baseline. Do not regenerate
the baseline automatically; an intentional change updates it in the same
commit.

### 6.4 Memory Safety and Resources

* Run specs and fuzzing with AddressSanitizer.
* Use separate leak tests.
* Inject allocation failures to test every OOM path.
* Core components that do not depend on Ruby may also use formal verification.

### 6.5 Performance (`bench/`)

Use `rake bench` to compare Makiri with Nokogiri. Keep an optimization only
when benchmarks improve and all normal, fuzz, and sanitizer tests still pass.

### 6.6 CI

Run GitHub Actions across supported Ruby versions and operating systems. Run
sanitizer tests on Linux.

## 7. Compatibility Considerations

Lexbor follows HTML5 behavior more closely than libxml2 in some areas, and
Makiri follows specifications even when the result differs from Nokogiri.
Important differences include:

| Item                     | Makiri              | Nokogiri / Browser        |
| ------------------------ | ------------------- | ------------------------- |
| Implicit `<head>`        | Yes                 | No in libxml2             |
| `<title>` placement      | Inside `<head>`     | As-is in libxml2          |
| `node()` and DOCTYPE     | Excluded            | Included by libxml2       |
| Attribute-axis order     | Document order      | Reverse in libxml2        |
| XPath namespace matching | Strict by default   | Strict in Nokogiri::HTML5 |
| XPath namespace axis     | Not implemented     | Implemented by libxml2    |
| Invalid HTML bytes       | Replace with U+FFFD | Encoding guess in libxml2 |
| HTML name case folding   | Not performed       | Browsers perform it       |

Known differences are recorded in curated conformance tests.

Some behavior, such as HTML name case folding and the handling of `xmlns`, is
still under review.

## 8. Design Decision Notes

These decisions may change later.

* `HTML()` is the main implementation and `parse()` is an alias.
* Node identity is based on the underlying node, not the Ruby wrapper.
* XPath evaluation keeps the GVL.

### XPath and the GVL

Earlier versions released the GVL during XPath evaluation and achieved better
parallel performance. However, XPath evaluation could then race with DOM
mutation or concurrent changes to the same `XPathContext`, causing memory
corruption.

Keeping the GVL gives a simpler safety rule, and parsing still releases the
GVL and remains parallel. Do not reintroduce GVL-free XPath evaluation until
proper document and context locking exists.

### Lenient parsing, strict APIs

Input handling also follows a clear rule:

* parsing is lenient
* program APIs are strict

HTML parsing repairs invalid byte sequences. XPath, CSS, names, and other
control inputs must be valid.

## 9. Migration from Nokogiri (Unofficial)

Typical Nokogiri code:

```ruby
require 'nokogiri'

doc = Nokogiri::HTML(source)
doc.css('.foo').map(&:text)
```

should usually work as:

```ruby
require 'makiri'

doc = Makiri::HTML(source)
doc.css('.foo').map(&:text)
```

The goal is for about 90% of common HTML scraping code to work with a simple
`Nokogiri` → `Makiri` replacement. The remaining differences mainly come from
HTML5 conformance and Makiri's stricter input rules.
