# Makiri robustness fuzzer

A grammar-aware fuzzer for Makiri's query surface (native XPath 1.0 engine and
the CSS selector glue). Makiri ships no second engine to diff against, so this
fuzzer targets **robustness**, not differential correctness: it generates and
mutates queries, runs them against fixture documents, and asserts Makiri never
does anything worse than raise one of its own exceptions.

A *finding* is therefore one of:

| category | meaning |
| --- | --- |
| `ok` | the query returned a value |
| `expected` | the query raised a `Makiri::Error` (incl. `XPath::SyntaxError`, `XPath::LimitExceeded`, `CSS::SyntaxError`) - malformed input rejected cleanly, **not** a finding |
| `unexpected` | a non-`Makiri::Error` Ruby exception leaked (contract violation) |
| `crash` | the worker died from a signal - only `--isolated` catches this |
| `timeout` | the query didn't return in time (missing budget / runaway) - only `--isolated` |

The high-value mode is **under AddressSanitizer** - that turns latent memory
errors (UAF, heap overflow) in the C engine into `crash` findings.

## Run

```sh
ruby -Ilib spec/fuzz/run.rb                 # 60 s, random seed, XPath
ruby -Ilib spec/fuzz/run.rb --time 300      # 5 minutes
ruby -Ilib spec/fuzz/run.rb --seed 42       # deterministic
ruby -Ilib spec/fuzz/run.rb --target css    # CSS instead of XPath
ruby -Ilib spec/fuzz/run.rb --target both
ruby -Ilib spec/fuzz/run.rb --isolated      # fork per query (crash/hang safe)

# Or via rake (defaults below; override with FUZZ_ARGS):
bundle exec rake fuzz
FUZZ_ARGS="--target both --time 120 --isolated" bundle exec rake fuzz
```

Exit status is non-zero when any new regression is saved.

### Under AddressSanitizer

```sh
MAKIRI_SANITIZE=address,undefined bundle exec rake fuzz:sanitize
```

This rebuilds the extension with the sanitizers, preloads the ASan runtime
(`DYLD_INSERT_LIBRARIES` / `LD_PRELOAD`) the same way `rake sanitize` does, then
runs the fuzzer. `--isolated` is implied so a sanitizer abort becomes a saved
`crash_*` finding instead of killing the loop.

## In-process vs isolated

The default in-process loop is fast but dies on a crash or hang. `--isolated`
forks a worker per query and kills it after `--query-timeout` seconds, so each
crash/hang becomes a single saved finding and the loop continues. Recommended
for runs longer than a few seconds and required to catch `crash`/`timeout`.

`regressions/last_input.txt` always records the in-flight query, so even an
in-process death leaves a trace.

## Outputs

* `spec/fuzz/regressions/<category>_<hash>/` per new finding:
  * `document.html` - the fixture
  * `query.txt` - `<target>` then the query
  * `note.md` - category + the exception / signal
* `spec/fuzz/regressions/last_input.txt` - most recent input (cleaned on a clean exit)

Findings are gitignored (the directory is kept via `.gitkeep`).

## Triage

Reproduce a finding directly:

```sh
ruby -Ilib -r makiri -e '
  doc = Makiri::HTML(File.read("spec/fuzz/regressions/<dir>/document.html"))
  target, query = File.read("spec/fuzz/regressions/<dir>/query.txt").split("\n", 2)
  p(target == "css" ? doc.css(query.strip) : doc.xpath(query.strip))
'
```

If it raises a non-`Makiri::Error` or crashes, it is a bug. After fixing,
re-run the fuzzer to confirm, and consider promoting the case to a permanent
spec.

## Layout

```
spec/fuzz/
  run.rb          driver (robustness checker)
  grammar.rb      grammar-aware XPath generator + byte-level mutator
  fixtures.rb     HTML fixtures
  seed_corpus.rb  seed XPath + CSS queries
  regressions/    findings (gitignored individually)
```
