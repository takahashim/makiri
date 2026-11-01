# Verus proofs

This directory holds [Verus](https://github.com/verus-lang/verus) proofs
about the parts of Makiri's Ruby-free core whose correctness rests on
arithmetic or on a loop invariant. A test checks the cases it runs; these
proofs hold for every length and size. Several of them replace Kani
harnesses, which could check only short inputs or ran out of memory.

| Proof | Code it covers | What it proves |
| --- | --- | --- |
| `ptr_table.rs` | `engine/src/ptr_table.rs` (`PtrTable`, `PtrMap`) | Linear probing always stops; every operation, `grow` included, keeps a free slot; each table behaves as the map it stands for (`get` answers exactly what was put) |
| `ceiling_buf.rs` | `engine/src/ceiling_buf.rs` (`Buf`, `copy_small`), `falloc`'s `grow_capacity`, `nupur`'s `copy_small` | A `Buf` holds exactly what was appended, its room never passes the ceiling, and a failure changes nothing; `copy_small` copies exactly |
| `limits.rs` | `engine/src/limits.rs` (`admit`) | The one rule every work budget charges through: an admitted charge never passes the cap, so over any sequence of charges the count stays within it |
| `utf8.rs` | `engine/src/utf8_decode.rs`, `engine/src/xml/chars/mod.rs` | The strict UTF-8 decoder and the XML Char scan agree with `vstd::utf8`, Verus's own statement of UTF-8 |
| `xml_chars.rs` | `engine/src/xml/chars/mod.rs` | NameStartChar ⊆ NameChar ⊆ Char (XML 1.0 §2.2, §2.3), for every `u32` |
| `xpath_number.rs` | `engine/src/xpath/number.rs` (`extent`) | The lexer's Number gate matches XPath 1.0's grammar and reads the longest Number |
| `swar.rs` | `engine/src/swar.rs` (`word`, `has_zero`), the tokenizer's `seek4` | The word-at-a-time skip passes no byte it looks for and never runs past `end`, whatever the word |
| `source_lines.rs` | `engine/src/source_lines.rs` (`lines_build`) | The line table is exactly 0 and the offset past each newline, in order |
| `xml_expand.rs` | `engine/src/xml/chars/expand.rs` | A character reference answers its digits' exact value or is refused, never wrapped; `expand_into` writes no more than it reads, so never `Overflow` |
| `int.rs` | `nupur/src/int.rs` | At both widths of a C `long` (32 bits on Windows, 64 elsewhere): `fixable_in` / `posfixable_in` answer exactly a Fixnum's range, with the value; a Fixnum's bits carry the flag and shift back to the `long`; `len` and `capacity` never truncate |

Each file's module comment states its claims in full.

## Copies held to the source

Each proof works on a copy of the functions it covers. A line that only Verus
needs (a spec, an invariant, a proof block) ends in `// [verus]`.
`rake verus:sync` drops those lines and comments, and checks that the rest of
each copied body matches the source token for token. A change to a covered
function therefore fails `rake verus` until the copy, and the proof, are
redone. The copies are listed in `rakelib/verus.rake` (`VERUS_COPIES`).

A proof may hold a function more than once: `int.rs` copies each conversion
into one module per width of a C `long`, each naming the width with a type
alias, and every copy must match. One more thing is dropped before the
comparison: a `#[verifier::truncate]` and the parentheses around its cast.
The attribute tells Verus that the cast wraps, which is what `as` does, so
it changes nothing that runs.

## Vacuity checks

A proof whose assumptions are too strong proves anything, and still reports
success. `rake verus` checks for that in two ways:

- **What each proof trusts is pinned.** `VERUS_TRUSTED` records how many
  `#[verifier::external_body]`, `assume_specification`, `assume(` and
  `admit()` each file holds (`ptr_table.rs` and `ceiling_buf.rs` 4 each,
  `xml_expand.rs` 3, `source_lines.rs` and `int.rs` 1 each, the others
  0). A change to a count fails the task until the record is updated.
- **Canaries must fail.** Each `// [canary]` line marks a point the proved
  code reaches. The task makes a variant of the file with `assert(false)` at
  one mark at a time, and `canaries/<proof>` adds functions that call the
  proved code and then assert `false`. Every one of them must fail to
  verify; one that verifies means the proof's assumptions prove anything
  there. The canaries themselves may assume nothing.

## Running

```bash
bundle exec rake verus:sync   # the copies against the source; needs no Verus
VERUS=path/to/verus bundle exec rake verus
```

`rake verus` needs the Verus release `rakelib/verus.rake` pins
(`VERUS_VERSION`) and the Rust toolchain that release names. CI runs it in
`verify.yml`, on a change to the covered code and weekly. See
`docs/internals/verification.md` for how it fits with Miri and the
exhaustive tests.
