# frozen_string_literal: true

# Keep the parts of the extension that are meant to be ordinary Rust that way.
#
# This is intentionally a small structural gate, not an attempt to parse Rust:
# `#![forbid(unsafe_code)]` is the compiler-enforced half.  The explicit list
# documents the safe island and makes a new raw-pointer escape visible in code
# review.  The static-mut check catches a different regression: a process-wide
# mutable singleton must have an ownership/serialisation argument, and new ones
# must not appear by accident.

ROOT = File.expand_path("..", __dir__)
RUST = File.join(ROOT, "ext/makiri/rust/src")

SAFE_FILES = %w[
  css/build.rs
  css/lower.rs
  xpath/attr_pred.rs
  xpath/axis.rs
  xpath/funcs.rs
  xpath/lex.rs
  xpath/nodetest.rs
  xpath/number.rs
  xpath/order.rs
  xpath/step_index.rs
  xml/chars.rs
  xml/index.rs
  xml/mutate.rs
  xml/qname.rs
  xml/selftest.rs
  xml/serialize.rs
  xml/tree.rs
].freeze

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
  "glue/css.rs" => 6,
  "glue/doc.rs" => 13,
  "glue/fragment.rs" => 1,
  "glue/html_node/mod.rs" => 3,
  "glue/html_node/mutate.rs" => 2,
  "glue/node.rs" => 7,
  "glue/node_set.rs" => 19,
  "glue/xml.rs" => 4,
  "glue/xml_node/mod.rs" => 3,
  "glue/xml_node/mutate.rs" => 2,
  "glue/xml_node/ns.rs" => 12,
  "glue/xml_node/serialize.rs" => 8,
  "glue/xpath.rs" => 28,
  "init.rs" => 5,
}.freeze

RAISING_API = /\b(?:rb_raise|rb_exc_raise|rb_jump_tag|rb_check_typeddata)\b/
RAISING_COUNTS = {
  "glue/abi.rs" => 1,              # the rb_raise declaration
  "glue/doc.rs" => 1,              # clone_node, a C-convention entry point
  "glue/fragment.rs" => 4,         # fragment parse failures
  "glue/node_set.rs" => 4,         # push from the C-convention entry point
  "glue/xml_node/mutate.rs" => 1,  # mkr_xml_mut_check
}.freeze

def rust_code(path)
  File.binread(path).lines.reject { |line| line.match?(%r{\A\s*//}) }.join
end

errors = []

SAFE_FILES.each do |relative|
  source = File.binread(File.join(RUST, relative))
  unless source.include?("#![forbid(unsafe_code)]")
    errors << "#{relative}: must retain #![forbid(unsafe_code)]"
  end
  if source.match?(/(^|\n)\s*(?:pub\s+)?unsafe\b/)
    errors << "#{relative}: contains unsafe code despite its safe-module contract"
  end
end

actual = Hash.new(0)
Dir.glob(File.join(RUST, "**", "*.rs")).sort.each do |path|
  count = File.binread(path).scan(/^\s*(?:pub\s+)?static\s+mut\b/).length
  next if count.zero?

  actual[path.delete_prefix("#{RUST}/")] = count
end

if actual != STATIC_MUT_COUNTS
  errors << "static mut boundary changed: expected #{STATIC_MUT_COUNTS.inspect}, got #{actual.inspect}"
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
  errors << "rb_sys:: outside bridge/ changed: expected #{RB_SYS_COUNTS.inspect}, got #{rb_sys.inspect}"
end
if raising != RAISING_COUNTS
  errors << "raising C API outside bridge/ changed: expected #{RAISING_COUNTS.inspect}, got #{raising.inspect}"
end

abort "unsafe-boundaries: #{errors.join("\nunsafe-boundaries: ")}" unless errors.empty?

puts "unsafe-boundaries: #{SAFE_FILES.length} safe modules; #{actual.values.sum} reviewed static mut declarations; " \
     "#{rb_sys.values.sum} rb_sys:: and #{raising.values.sum} raising C calls outside bridge/"
