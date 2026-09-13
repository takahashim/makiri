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
  xpath/lex.rs
  xpath/number.rs
  xml/chars.rs
  xml/index.rs
  xml/qname.rs
].freeze

# These are the remaining C/Ruby ABI globals.  Each entry is deliberately
# exact: adding another static mut must come with a dedicated boundary type or
# an explicit review of its synchronisation proof.
STATIC_MUT_COUNTS = {
  "init.rs" => 1,
}.freeze

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

abort "unsafe-boundaries: #{errors.join("\nunsafe-boundaries: ")}" unless errors.empty?

puts "unsafe-boundaries: #{SAFE_FILES.length} safe modules; #{actual.values.sum} reviewed static mut declarations"
