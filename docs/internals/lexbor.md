# Lexbor: the pin and the lexbor-contains guard

## The pin

Pinned to **`05b5d37`** (`v3.0.0-66-g05b5d37`), which builds cleanly with our
`LEXBOR_BUILD_SHARED=OFF` config. **Normally we pin to a release tag**, but
this is an *untagged master* commit, taken deliberately. It is still
**vanilla, NEVER patched**: the constraint that relaxed is "release tag only",
not "no fork".

**v3.0.1 is NOT usable.** It is a maintenance release off v3.0.0 and does not
contain the **two CSS-selector fixes we upstreamed**, so moving to it would
REGRESS matching:

- `#369` (`3a2d595`): class/ID selectors match case-sensitively except in
  quirks mode, like browsers.
- `#371` (`940162d`): a prefix-less type selector no longer defaults to the
  universal namespace.

Check that again on the next tag.

Master also carries what Makiri needs:

- the **heap-overflow fix in the `:lexbor-contains()` parser** (reached via
  `Node#css`);
- the `#365` tokenizer size-limit (DoS) fix;
- new in this pin, the fix for an **out-of-bounds write in
  `lxb_dom_character_data_replace`** (`cf31487`), which `Node#content=` on a
  text/comment/PI node reaches through `lxb_dom_node_text_content_set`, plus
  UTF-16 surrogate rejection, exact `meta` charset matching, buffer-capacity
  fixes in the integer/hex conversion, and selector-destroy leaks.

## Processing instructions: the parsing change

**This pin changes parsing**, which no earlier bump did. `f441451` implements
the HTML Standard's new **processing-instruction tokens**:

- `<?php ...?>` in HTML input is a `ProcessingInstruction` node where it used
  to be a Comment;
- `#to_html` writes `<?target data?>`;
- `<?` at EOF is ignored (the spec's "eof-in-processing-instruction").

`Nokogiri::HTML5` still answers the old way; see NOKOGIRI_DIFFERENCES.md. The
differential baseline's 81 affected lines were edited with the bump.

`spec/conformance/html5lib_runner.rb` reads its tree-construction data from
WPT (`html/syntax/parsing/resources`, where the html5lib .dat files have lived
since June 2026), which carries the new expectations. What this Lexbor pin
still answers the old way is listed in `KNOWN_DIVERGENCES` there and in
`docs/parser_differences.ja.md`.

## The lexbor-contains guard

**`:lexbor-contains()` is not supported, and `lexbor::contains_guard` keeps
every one from the vendored CSS parser - it is not optional.**

Its argument parser is where the v3.0.0 heap overflow was, and a failed
argument parse left a pseudo-class the serializer crashed on. Deciding which
arguments were safe kept failing, so the guard renames EVERY
`lexbor-contains(` function, whatever its argument, and the parser rejects it
as an unknown pseudo-class: in a selector, a `CSS::SyntaxError`; in a
stylesheet, a `bad_style` rule. Its module doc states the two properties that
must hold and why; `lexbor/tests.rs` pins them against the real parser.

The guard finds the name on the tokens of Lexbor's OWN CSS syntax tokenizer
(`lexbor::css_tokens`), never with a scanner of ours. The hand scanner it
replaced ended strings where the tokenizer did not (CR, FF,
backslash-newline), so a `:lexbor-contains(#x)` after such a string reached
the parser unseen, and `parse_stylesheet` crashed serializing what it left
behind.

### Where the guard still runs

**Since the parser port, no build that ships runs Lexbor's CSS parser at
all.** Selectors and stylesheets are both parsed by `crate::selectors`, which
leaves `lexbor-contains` out of its function table and so rejects one as an
unknown function - the answer the guard gave.

The guard, `css_tokens`, `css_engine` and `css_parser` are compiled only into
the tests and the reference features, where Lexbor's parser is the oracle.
Both reference call sites still go through the guard - `css_engine::parse`
(selectors) and `stylesheet_diff::lexbor_dump` (stylesheets) - and a new one
must. The guard and its tests come out together or not at all, once no
reference runs Lexbor's parser.
