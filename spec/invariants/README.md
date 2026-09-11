# Invariant checks

Randomised property checks over the tree, the indexes, serialization and the
text-input contract. They are not `*_spec.rb`, so `rake spec` skips them; run
them with `rake invariants` (or a file at a time, see below).

They exist because the properties here are awkward for an example-based spec:

* a resolved namespace URI is invisible to serialization, so comparing output
  strings walks straight past it;
* HTML parsing is not idempotent, so a round-trip comparison produces mostly
  spec-conformant noise;
* a stale index is only visible as a disagreement between two code paths;
* an escaping bug shows up as one extra attribute in a tree read back, not as a
  different string.

Each check states its own oracle in its header. Two of them use a different
oracle from the rest on purpose, and say why.

| file | what it holds to |
|---|---|
| `check_ns_reresolve.rb` | a mutated tree's namespaces equal what re-parsing its output computes |
| `check_import_clone.rb` | clone / import / adopt / fragment, and that a move keeps `namespace_uri` |
| `check_tree_invariants.rb` | the child list stays a tree after every edit, on both backends |
| `check_index_staleness.rb` | the attr / element / text indexes never outlive a mutation |
| `check_serialize_fixpoint.rb` | serialization adds nothing, loses nothing, and settles |
| `check_text_input.rb` | encodings, invalid UTF-8, and the NUL two-tier contract |

## Running them

```sh
bundle exec rake invariants                  # all of them, both backends
bundle exec rake invariants:sanitize         # the same under ASan + UBSan

# one at a time: [documents] [seed] [html|xml]
ruby -Ilib spec/invariants/check_index_staleness.rb 2000 42 xml
```

Every check takes a seed and is deterministic, so a failure replays exactly. A
non-zero exit means a property broke; the output names which one and prints the
edit sequence that got there.

Running under ASan is worth it for the ones that touch borrowed memory:
`check_index_staleness.rb` (the text index holds borrowed slices) and
`check_tree_invariants.rb` (a use-after-free usually precedes a broken link).

## Negative controls

A check that cannot fail is worth nothing, and two of these carry the
demonstration inline:

* `check_serialize_fixpoint.rb` undoes the escaping in Makiri's own output and
  shows the faithfulness property catching the injected attribute;
* `check_text_input.rb` feeds genuinely injected markup to its detector, in
  three shapes.

For the others the control is external: remove an `mkr_parsed_*_invalidate` call
from `mkr_invalidate_index` and rebuild, and `check_index_staleness.rb` fires on
the index you disabled. That is how the checks in this directory were verified
to have teeth in the first place.
