# frozen_string_literal: true

# Keep the parts of the extension that are meant to be ordinary Rust that way.
#
# The compiler holds the default: `lib.rs` denies `unsafe_code`, so a file that
# wants unsafe must say `#![allow(unsafe_code)]` and that line shows up in
# review. This script pins the result, the way it already pins `rb_sys::` and
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

# Every file's unsafe count, exact. A new file with unsafe fails here even when
# a parent module's `allow` kept the compiler quiet; a file that loses its last
# unsafe fails until the entry goes, so a reduction lands with the gate that
# records it. The count is `unsafe` followed by `{`, `fn`, `impl`, `trait` or
# `extern`, outside comment lines - the one definition, kept here.
UNSAFE_ISLANDS = {
  "bridge/ruby.rs" => 11,
  "bridge/string.rs" => 18,
  "bridge/xml_decode.rs" => 7,
  "cbuf.rs" => 15,
  "cbuf/verify.rs" => 7,
  "css/mod.rs" => 1,
  "css/parser.rs" => 47,
  "dom_adapter/cross_import.rs" => 16,
  "dom_adapter/dom_index.rs" => 3,
  "dom_adapter/html.rs" => 90,
  "dom_adapter/post_parse.rs" => 9,
  "dom_adapter/source_loc.rs" => 4,
  "dom_adapter/text_index.rs" => 5,
  "dom_adapter/utf8_input.rs" => 1,
  "falloc/calloc_verify.rs" => 3,
  "falloc/cstr.rs" => 3,
  "falloc/inject.rs" => 3,
  "falloc/mod.rs" => 2,
  "falloc/raw.rs" => 4,
  "glue/abi.rs" => 9,
  "glue/css.rs" => 21,
  "glue/doc.rs" => 25,
  "glue/fragment.rs" => 10,
  "glue/html_node/mod.rs" => 8,
  "glue/html_node/mutate.rs" => 47,
  "glue/html_node/read.rs" => 19,
  "glue/lexbor_css.rs" => 11,
  "glue/node.rs" => 7,
  "glue/node_set.rs" => 16,
  "glue/serialize.rs" => 9,
  "glue/xml.rs" => 14,
  "glue/xml_node/mod.rs" => 8,
  "glue/xml_node/mutate.rs" => 22,
  "glue/xml_node/ns.rs" => 4,
  "glue/xml_node/read.rs" => 21,
  "glue/xml_node/serialize.rs" => 3,
  "glue/xpath.rs" => 25,
  "init.rs" => 7,
  "lexbor_abi.rs" => 5,
  "rust_tests.rs" => 6,
  "text.rs" => 4,
  "xpath/ctx.rs" => 10,
  "xpath/dom.rs" => 1,
  "xpath/dom_html.rs" => 4,
  "xpath/dom_xml.rs" => 1,
  "xpath/eval.rs" => 1,
  "xpath/msg.rs" => 3,
  "xpath/parse.rs" => 2,
  "xpath/tests.rs" => 4,
  "xpath/value.rs" => 1,
}.freeze

# Files whose safety is compiler-enforced. Checked by containment, so adding one
# needs no edit here; `forbid` cannot be overridden by an inner `allow`, which is
# why a module root whose children need unsafe is not on this list. `xml/mod.rs`
# is, and that fixes the whole `xml/` subtree as Ruby- and Lexbor-free.
FORBID_FILES = %w[
  css/build.rs css/lower.rs cutf8.rs
  cutf8/verify.rs falloc/calloc.rs falloc/verify.rs
  glue/xml_node/abi.rs xml/api.rs xml/arena.rs
  xml/chars.rs xml/index.rs xml/mod.rs
  xml/model.rs xml/mutate.rs xml/parse.rs
  xml/qname.rs xml/selftest.rs xml/serialize.rs
  xml/tree.rs xml/verify.rs xpath/abi.rs
  xpath/ast.rs xpath/ast_ops.rs xpath/attr_pred.rs
  xpath/axis.rs xpath/funcs.rs xpath/lex.rs
  xpath/limits.rs xpath/nodetest.rs xpath/number.rs
  xpath/order.rs xpath/runtime_abi.rs xpath/runtime_abi/cache.rs
  xpath/step_index.rs xpath/verify.rs
].freeze

UNSAFE_USE = /\bunsafe\s*(?:\{|fn\b|impl\b|trait\b|extern\b)/

# These are the remaining C/Ruby ABI globals.  Each entry is deliberately
# exact: adding another static mut must come with a dedicated boundary type or
# an explicit review of its synchronisation proof.
STATIC_MUT_COUNTS = {}.freeze

# Ruby's C API outside `bridge/` is a ratchet. The bridge is where raw VALUEs,
# typed data and the C calls that raise are meant to live (glue/mod.rs): a raise
# there becomes an `Err` before it can longjmp past a Rust destructor. The glue
# still calls `rb_sys::` directly for method registration, constants and the
# per-node hot paths that must not pay for magnus's `protect`, so each file's
# count is pinned here. A new direct call - above all a new raising one - fails
# until it moves behind the bridge or the count is raised in review; removing
# calls means lowering the count. `magnus::rb_sys` is magnus's own module and is
# not counted, and neither are comment lines.
RB_SYS_COUNTS = {
  "glue/abi.rs" => 4,
  "glue/css.rs" => 5,
  "glue/doc.rs" => 9,
  "glue/fragment.rs" => 1,
  "glue/html_node/mod.rs" => 2,
  "glue/node.rs" => 3,
  "glue/node_set.rs" => 13,
  "glue/xml.rs" => 2,
  "glue/xml_node/mod.rs" => 2,
  "glue/xml_node/serialize.rs" => 2,
  "glue/xpath.rs" => 26,
  "init.rs" => 4,
}.freeze

RAISING_API = /\b(?:rb_raise|rb_exc_raise|rb_jump_tag|rb_check_typeddata)\b/
RAISING_COUNTS = {}.freeze

def rust_code(path)
  File.binread(path).lines.reject { |line| line.match?(%r{\A\s*//}) }.join
end

# `--fix` transcribes the two tables that move whenever code moves, so a hand
# count cannot disagree with the one this script does. It deliberately does NOT
# touch RB_SYS_COUNTS, STATIC_MUT_COUNTS or RAISING_COUNTS: those record a
# boundary DECISION rather than a consequence of moving code, and a new direct
# `rb_sys::` call, a new process-wide mutable global or a new raising C call
# outside `bridge/` is exactly the thing a person should have to think about.
# Rewriting them automatically would spend the ratchet it exists to hold.
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
Dir.glob(File.join(RUST, "**", "*.rs")).sort.each do |path|
  relative = path.delete_prefix("#{RUST}/")
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
end

actual = Hash.new(0)
Dir.glob(File.join(RUST, "**", "*.rs")).sort.each do |path|
  count = File.binread(path).scan(/^\s*(?:pub\s+)?static\s+mut\b/).length
  next if count.zero?

  actual[path.delete_prefix("#{RUST}/")] = count
end

if actual != STATIC_MUT_COUNTS
  errors << "static mut boundary changed: #{table_diff(STATIC_MUT_COUNTS, actual)}"
end

rb_sys = Hash.new(0)
raising = Hash.new(0)
Dir.glob(File.join(RUST, "**", "*.rs")).sort.each do |path|
  relative = path.delete_prefix("#{RUST}/")
  next if relative.start_with?("bridge/")

  code = rust_code(path)
  count = code.scan(/(?<!magnus::)\brb_sys::/).length
  rb_sys[relative] = count unless count.zero?
  count = code.scan(RAISING_API).length
  raising[relative] = count unless count.zero?
end

if rb_sys != RB_SYS_COUNTS
  errors << "rb_sys:: outside bridge/ changed: #{table_diff(RB_SYS_COUNTS, rb_sys)}"
end
if raising != RAISING_COUNTS
  errors << "raising C API outside bridge/ changed: #{table_diff(RAISING_COUNTS, raising)}"
end

if FIX && !errors.empty?
  puts "unsafe-boundaries --fix: NOT rewritten, these record a decision rather than a count:"
  errors.each { |e| puts "  #{e}" }
end
abort "unsafe-boundaries: #{errors.join("\nunsafe-boundaries: ")}" unless errors.empty?

puts "unsafe-boundaries: #{forbidding.length} forbid files; " \
     "#{unsafe_actual.values.sum} unsafe uses in #{unsafe_actual.length} islands; " \
     "#{actual.values.sum} reviewed static mut declarations; " \
     "#{rb_sys.values.sum} rb_sys:: and #{raising.values.sum} raising C calls outside bridge/"
