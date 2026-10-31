# frozen_string_literal: true

# Keep the parts of the extension that are meant to be ordinary Rust that way.
#
# The compiler holds the default: `lib.rs` denies `unsafe_code`, so a file that
# wants unsafe must say `#![allow(unsafe_code)]` and that line shows up in
# review. This script pins the result, the way it already pins
# `static mut` - what the compiler cannot do is notice that an island grew.
#
# Why the count check runs over EVERY file rather than only the islands: an
# `allow` propagates to child modules, so the one on `falloc/mod.rs` covers all
# of `falloc/`. A new file added there could use unsafe with the compiler silent
# AND carry no allow of its own, so a check that only looked at files carrying
# an allow would miss it. Counting everything and demanding an exact match does
# not.

ROOT = File.expand_path("..", __dir__)
RUST = File.join(ROOT, "ext/makiri/rust/src")
# Every source file the gates read. `lint_probe.rs` is left out: it is
# compiled only under `--cfg makiri_lint_probe`, by `rake dylint`, and holds
# on purpose the very uses the lints there must report.
# The binding layer is a crate of its own (`ext/makiri/rust/nupur`), read as
# if it were still `src/nupur/`: its files are keyed `nupur/<path>` below.
NUPUR = File.join(ROOT, "ext/makiri/rust/nupur/src")
RUST_FILES = (Dir.glob(File.join(RUST, "**", "*.rs")) + Dir.glob(File.join(NUPUR, "**", "*.rs"))).sort
                .reject { |p| p.end_with?("/lint_probe.rs") }.freeze

# A file's key in the tables: its path under `src/`, or `nupur/<path>` for
# the binding layer's.
def rust_rel(path)
  return "nupur/#{path.delete_prefix("#{NUPUR}/")}" if path.start_with?("#{NUPUR}/")

  path.delete_prefix("#{RUST}/")
end

# Every file's unsafe count, exact. A new file with unsafe fails here even when
# a parent module's `allow` kept the compiler quiet; a file that loses its last
# unsafe fails until the entry goes, so a reduction lands with the gate that
# records it. The count is `unsafe` followed by `{`, `fn`, `impl`, `trait` or
# `extern`, outside comment lines - the one definition, kept here.
#
# Kept SORTED (the gate fails otherwise), so `--fix` and a hand edit agree and
# two entries for one file cannot both stand: a Ruby Hash literal takes the last
# duplicate silently. Moving a boundary DOWN to `bridge/` raises the number here
# (the unsafe did not disappear, it moved); what ratchets is the total, and the
# summary prints the `glue/` + `xpath/` subtotal that must reach 0.
UNSAFE_ISLANDS = {
  "falloc/mod.rs" => 1,
  "init.rs" => 1,
  "lexbor/abi.rs" => 4,
  "lexbor/adapter/arena_bytes.rs" => 14,
  "lexbor/adapter/html/attrs.rs" => 19,
  "lexbor/adapter/html/build.rs" => 14,
  "lexbor/adapter/html/mod.rs" => 66,
  "lexbor/adapter/html/serialize.rs" => 18,
  "lexbor/adapter/post_parse.rs" => 8,
  "lexbor/adapter/source_loc.rs" => 1,
  "lexbor/adapter/tree_guard.rs" => 7,
  "lexbor/chunks.rs" => 2,
  "lexbor/conformance_tests.rs" => 24,
  "lexbor/css_engine.rs" => 10,
  "lexbor/css_parser.rs" => 20,
  "lexbor/css_tokens.rs" => 10,
  "lexbor/fragment.rs" => 7,
  "lexbor/res_tests.rs" => 3,
  "lexbor/selector_port_tests.rs" => 1,
  "lexbor/selectors.rs" => 10,
  "lexbor/serialize.rs" => 1,
  "lexbor/stylesheet_diff.rs" => 5,
  "lexbor/tests.rs" => 3,
  "no_unwind.rs" => 2,
  "nupur/abi.rs" => 1,
  "nupur/encoding.rs" => 6,
  "nupur/gvl.rs" => 3,
  "nupur/no_unwind.rs" => 2,
  "nupur/raw/check.rs" => 4,
  "nupur/raw/data.rs" => 4,
  "nupur/raw/define.rs" => 22,
  "nupur/raw/exception.rs" => 5,
  "nupur/raw/iterate.rs" => 6,
  "nupur/raw/mod.rs" => 1,
  "nupur/raw/protect.rs" => 13,
  "nupur/raw/string.rs" => 11,
  "nupur/raw/symbol.rs" => 3,
  "nupur/raw/values.rs" => 21,
  "nupur/rb/global.rs" => 2,
  "nupur/rb/kept.rs" => 2,
  "nupur/rb/method.rs" => 17,
  "nupur/rb/object.rs" => 5,
  "nupur/rb/ruby.rs" => 5,
  "nupur/rb/value.rs" => 18,
  "nupur/rb/value/header.rs" => 3,
  "nupur/stack.rs" => 1,
  "nupur/string/make.rs" => 5,
  "nupur/string/read.rs" => 2,
  "nupur/typed/callbacks.rs" => 10,
  "nupur/typed/data_type.rs" => 12,
  "nupur/typed/external.rs" => 1,
  "nupur/typed/mod.rs" => 13,
  "nupur/utf8_copy.rs" => 1,
}.freeze

# Files whose safety is compiler-enforced. Checked by containment, so adding one
# needs no edit here, and kept SORTED (the gate fails otherwise) so `--fix` and a
# hand edit agree. `forbid` cannot be overridden by an inner `allow`, which is
# why a module root whose children need unsafe is not on this list.
#
# The subtree roots in FORBID_ROOTS make whole directories compiler-forbidden:
# `forbid` is inherited, so a new file under `glue/` or `xpath/` cannot slip in
# with an `allow`, however few its `unsafe`s.
FORBID_ROOTS = %w[bridge/mod.rs glue/mod.rs css_match/mod.rs selectors/mod.rs xml/mod.rs
                  xpath/mod.rs].freeze

FORBID_FILES = %w[
  bridge/dom.rs bridge/dom_error.rs bridge/html/attrs.rs
  bridge/html/document.rs bridge/html/edit.rs bridge/html/fragment.rs
  bridge/html/insert.rs bridge/html/mod.rs bridge/html/receiver.rs
  bridge/html/wrap.rs bridge/mod.rs bridge/node_set.rs
  bridge/node_wrap.rs bridge/ruby.rs bridge/stack.rs
  bridge/string/build.rs bridge/string/encoding.rs bridge/string/source.rs
  bridge/string/view.rs bridge/wrapper/accounting.rs bridge/wrapper/child_list.rs
  bridge/wrapper/document.rs bridge/wrapper/edit.rs bridge/wrapper/ivar.rs
  bridge/wrapper/node.rs bridge/wrapper/node_table.rs bridge/xml/attrs.rs
  bridge/xml/decode.rs bridge/xml/document.rs bridge/xml/edit.rs
  bridge/xml/error.rs bridge/xml/insert.rs bridge/xml/mod.rs
  bridge/xml/receiver.rs bridge/xml/strings.rs bridge/xml/wrap.rs
  bridge/xpath/context_object.rs bridge/xpath/mod.rs cbuf.rs
  child_index.rs conformance/mod.rs css_cache.rs
  css_match/mod.rs cutf8.rs cutf8/verify.rs
  doc_cell.rs dom_rules.rs engine_error.rs
  glue/css.rs glue/dom_names.rs glue/html_doc.rs
  glue/html_node/css.rs glue/html_node/mutate.rs glue/html_node/read.rs
  glue/html_node/serialize.rs glue/mod.rs glue/namespaces.rs
  glue/node.rs glue/node_set.rs glue/query.rs
  glue/xml_doc.rs glue/xml_node/css.rs glue/xml_node/mod.rs
  glue/xml_node/mutate.rs glue/xml_node/ns.rs glue/xml_node/read.rs
  glue/xml_node/serialize.rs glue/xpath_context.rs gvl.rs
  html/dom/mod.rs html/encoding/mod.rs html/mod.rs
  html/tokenizer/mod.rs html/tree/mod.rs html_dom.rs
  lexbor/contains_guard.rs limits.rs node_type.rs
  nupur/args.rs nupur/build.rs nupur/cname.rs
  nupur/describe.rs nupur/hash.rs nupur/int.rs
  nupur/msg.rs nupur/rb/class.rs nupur/rb/convert.rs
  nupur/rb/error.rs nupur/table.rs ptr_table.rs
  rust_tests.rs selectors/mod.rs selectors/stylesheet.rs
  selectors/text.rs source_lines.rs stack.rs
  text.rs token.rs utf8_input.rs
  xml/arena.rs xml/attr_key.rs xml/chars/expand.rs
  xml/chars/mod.rs xml/css.rs xml/css_tests.rs
  xml/dom_name.rs xml/encoding_names.rs xml/encoding_sniff.rs
  xml/index.rs xml/mod.rs xml/model.rs
  xml/mutate/attr.rs xml/mutate/copy.rs xml/mutate/edit.rs
  xml/mutate/factory.rs xml/mutate/insert.rs xml/mutate/mod.rs
  xml/mutate/ns.rs xml/ns_scope.rs xml/qname.rs
  xml/selftest.rs xml/serialize/bindings.rs xml/serialize/c14n.rs
  xml/serialize/mod.rs xml/serialize/out.rs xml/serialize/xml.rs
  xml/tree/cursor.rs xml/tree/decl.rs xml/tree/dtd.rs
  xml/tree/mod.rs xml/tree/scope.rs xml/verify.rs
  xml/xpath.rs xpath/abi.rs xpath/ast.rs
  xpath/ast_ops.rs xpath/attr_pred.rs xpath/axis.rs
  xpath/ctx.rs xpath/dom.rs xpath/eval.rs
  xpath/funcs/ext.rs xpath/funcs/mod.rs xpath/lex.rs
  xpath/limits.rs xpath/mod.rs xpath/nodetest.rs
  xpath/number.rs xpath/order.rs xpath/parse.rs
  xpath/step_index.rs xpath/str_cache.rs xpath/tests.rs
  xpath/value.rs xpath/verify.rs
].freeze

UNSAFE_USE = /\bunsafe\s*(?:\{|fn\b|impl\b|trait\b|extern\b)/

# These are the remaining C/Ruby ABI globals.  Each entry is deliberately
# exact: adding another static mut must come with a dedicated boundary type or
# an explicit review of its synchronisation proof.
STATIC_MUT_COUNTS = {}.freeze

# What this script checked besides unsafe is elsewhere now: the layer rules
# (Ruby-free engine, `rb_sys::` and raw `VALUE`s in the bridge, raising calls
# under protection, no infallible allocation) are `rake dylint`'s, on resolved
# paths. The reference Lexbor's own rules went with it being built only for
# the tests and fuzz targets: no build that ships compiles `lexbor/`, so
# nothing that ships can name it.

def rust_code(path)
  File.binread(path).lines.reject { |line| line.match?(%r{\A\s*//}) }.join
end

# `--fix` transcribes the two tables that move whenever code moves, so a hand
# count cannot disagree with the one this script does, and it writes them SORTED
# (the shape the gate also demands), so running it is idempotent. It deliberately
# does NOT touch STATIC_MUT_COUNTS: that records a boundary DECISION rather than
# a consequence of moving code, and a new process-wide mutable global is exactly
# the thing a person should have to think about. Rewriting it automatically
# would spend the ratchet it exists to hold.
FIX = ARGV.delete("--fix")
abort "usage: check_unsafe_boundaries.rb [--fix]" unless ARGV.empty?

# Only the entries that moved: a pinned table has a dozen rows, and printing
# both copies of it buries the one line that changed.
def table_diff(recorded, actual)
  keys = (recorded.keys | actual.keys).select { |k| recorded[k] != actual[k] }
  keys.sort.map { |k| "#{k} #{recorded[k] || 0} -> #{actual[k] || 0}" }.join(", ")
end

# Replace `NAME = <open> ... <close>` in this script's own source.
def rewrite_table!(source, name, open_tok, close_tok, body)
  head = "#{name} = #{open_tok}\n"
  from = source.index(head) or abort "unsafe-boundaries: cannot find #{name} to rewrite"
  to = source.index("#{close_tok}\n", from) or abort "unsafe-boundaries: #{name} is unterminated"
  source[0...from] + head + body + source[to..]
end

errors = []

unless File.binread(File.join(RUST, "lib.rs")).include?("#![deny(unsafe_code)]")
  errors << "lib.rs: must retain #![deny(unsafe_code)] - it is what makes the rest a ratchet"
end

unsafe_actual = Hash.new(0)
forbidding = []
RUST_FILES.each do |path|
  relative = rust_rel(path)
  source = File.binread(path)
  forbidding << relative if source.include?("#![forbid(unsafe_code)]")
  count = rust_code(path).scan(UNSAFE_USE).length
  unsafe_actual[relative] = count unless count.zero?
end

if FIX
  src = File.binread(__FILE__)
  before = src.dup
  src = rewrite_table!(src, "UNSAFE_ISLANDS", "{", "}.freeze",
                       unsafe_actual.sort.map { |f, n| %(  "#{f}" => #{n},\n) }.join)
  src = rewrite_table!(src, "FORBID_FILES", "%w[", "].freeze",
                       forbidding.sort.each_slice(3).map { |r| "  #{r.join(' ')}\n" }.join)
  if src == before
    # Nothing to say: the exit status and the summary below already report it.
  else
    File.binwrite(__FILE__, src)
    gained = unsafe_actual.reject { |k, v| UNSAFE_ISLANDS[k] == v }
    lost = UNSAFE_ISLANDS.reject { |k, v| unsafe_actual[k] == v }
    puts "unsafe-boundaries --fix: islands now #{gained.inspect} (were #{lost.inspect}); " \
         "#{forbidding.length} forbid files"
    puts "unsafe-boundaries --fix: review this file's diff - the numbers moved, and why is the commit"
  end
else
  if unsafe_actual != UNSAFE_ISLANDS
    gained = unsafe_actual.reject { |k, v| UNSAFE_ISLANDS[k] == v }
    lost = UNSAFE_ISLANDS.reject { |k, v| unsafe_actual[k] == v }
    errors << "unsafe islands changed: got #{gained.inspect}, recorded #{lost.inspect} " \
              "(`rake unsafe:fix` transcribes it)"
  end

  missing = FORBID_FILES - forbidding
  errors << "lost #![forbid(unsafe_code)]: #{missing.inspect}" unless missing.empty?
  # Two-way, like the islands: a new forbid file is recorded, so a later loss of
  # it is caught above rather than never having been known.
  unrecorded = forbidding - FORBID_FILES
  errors << "unrecorded #![forbid(unsafe_code)] (`rake unsafe:fix`): #{unrecorded.inspect}" \
    unless unrecorded.empty?

  # A whole directory is safe by one inherited attribute, so a stray `allow`
  # under it does not build; the gate only has to keep the root attribute there.
  missing_roots = FORBID_ROOTS.reject { |f| forbidding.include?(f) }
  errors << "subtree roots must keep #![forbid(unsafe_code)]: #{missing_roots.inspect}" \
    unless missing_roots.empty?

  # Both tables are kept sorted so `--fix` (which sorts) and a hand edit agree
  # and a duplicate key cannot hide: Ruby's Hash literal keeps the last silently.
  errors << "UNSAFE_ISLANDS must stay sorted (`rake unsafe:fix`)" \
    if UNSAFE_ISLANDS.keys != UNSAFE_ISLANDS.keys.sort
  errors << "FORBID_FILES must stay sorted (`rake unsafe:fix`)" \
    if FORBID_FILES != FORBID_FILES.sort
end

actual = Hash.new(0)
RUST_FILES.each do |path|
  count = File.binread(path).scan(/^\s*(?:pub\s+)?static\s+mut\b/).length
  next if count.zero?

  actual[rust_rel(path)] = count
end

if actual != STATIC_MUT_COUNTS
  errors << "static mut boundary changed: #{table_diff(STATIC_MUT_COUNTS, actual)}"
end

if FIX && !errors.empty?
  puts "unsafe-boundaries --fix: NOT rewritten, these record a decision rather than a count:"
  errors.each { |e| puts "  #{e}" }
end
abort "unsafe-boundaries: #{errors.join("\nunsafe-boundaries: ")}" unless errors.empty?

glue_xpath_unsafe = unsafe_actual
  .select { |f, _| f.start_with?("glue/", "xpath/") }
  .values.sum
puts "unsafe-boundaries: #{forbidding.length} forbid files; " \
     "#{unsafe_actual.values.sum} unsafe uses in #{unsafe_actual.length} islands " \
     "(glue+xpath #{glue_xpath_unsafe}); " \
     "#{actual.values.sum} reviewed static mut declarations"
