# frozen_string_literal: true

# Verus over the Ruby-free core (ext/makiri/rust/verus/), at every length or
# size: the growth arithmetic, copied from falloc, the pointer tables, the byte
# buffer, the work budgets' one rule, the strict UTF-8 decoder with the XML
# Char scan, the XML character classes, the XPath Number gate, the
# word-at-a-time scans (the tokenizer's seek4, the line table), and the XML
# reference expander, and Nupur's C-integer conversions. The copies are checked
# against the source first - every line only Verus needs ends in `// [verus]`,
# and the rest of the body must be the source's, token for token - so a
# change to `grow_capacity` fails here until the proof is redone. VERUS names
# the binary (else `verus` on PATH); CI: verify.yml, at VERUS_VERSION.
VERUS_VERSION = "0.2026.10.04.426d8b0"
VERUS_COPIES = [
  *%w[capacity_for probe lookup PtrTable::with_keys PtrTable::insert PtrTable::get
      PtrMap::clear PtrMap::get PtrMap::insert PtrMap::set PtrMap::grow].map do |name|
    ["ext/makiri/rust/engine/src/ptr_table.rs", "ext/makiri/rust/verus/ptr_table.rs", name]
  end,
  ["ext/makiri/rust/falloc/src/lib.rs", "ext/makiri/rust/verus/ceiling_buf.rs", "grow_capacity"],
  ["ext/makiri/rust/engine/src/limits.rs", "ext/makiri/rust/verus/limits.rs", "admit"],
  *%w[decode1 decode_multibyte].map do |name|
    ["ext/makiri/rust/engine/src/utf8_decode.rs", "ext/makiri/rust/verus/utf8.rs", name]
  end,
  *%w[is_char all_code_points validate_chars].map do |name|
    ["ext/makiri/rust/engine/src/xml/chars/mod.rs", "ext/makiri/rust/verus/utf8.rs", name]
  end,
  *%w[is_char is_name_start is_name_char].map do |name|
    ["ext/makiri/rust/engine/src/xml/chars/mod.rs", "ext/makiri/rust/verus/xml_chars.rs", name]
  end,
  *%w[extent digits_end].map do |name|
    ["ext/makiri/rust/engine/src/xpath/number.rs", "ext/makiri/rust/verus/xpath_number.rs", name]
  end,
  *%w[word has_zero].map do |name|
    ["ext/makiri/rust/engine/src/swar.rs", "ext/makiri/rust/verus/swar.rs", name]
  end,
  ["ext/makiri/rust/engine/src/html/tokenizer/mod.rs", "ext/makiri/rust/verus/swar.rs", "seek4"],
  ["ext/makiri/rust/engine/src/swar.rs", "ext/makiri/rust/verus/source_lines.rs", "word"],
  *%w[newline_mask lines_build push_starts].map do |name|
    ["ext/makiri/rust/engine/src/source_lines.rs", "ext/makiri/rust/verus/source_lines.rs", name]
  end,
  *%w[scan_char_ref scan_reference walk expand_into Writer::put Writer::put_slice].map do |name|
    ["ext/makiri/rust/engine/src/xml/chars/expand.rs", "ext/makiri/rust/verus/xml_expand.rs", name]
  end,
  *%w[is_char Utf8Char::encode Utf8Char::as_bytes].map do |name|
    ["ext/makiri/rust/engine/src/xml/chars/mod.rs", "ext/makiri/rust/verus/xml_expand.rs", name]
  end,
  ["ext/makiri/rust/engine/src/ceiling_buf.rs", "ext/makiri/rust/verus/ceiling_buf.rs", "copy_small"],
  # nupur's copy of it, which writes a String in place (`utf8_str_filled`).
  ["ext/makiri/rust/nupur/src/string/make.rs", "ext/makiri/rust/verus/ceiling_buf.rs", "copy_small"],
  # Nupur's C-integer conversions, one copy per width of a C `long`.
  *%w[fixable_in posfixable_in fixnum_bits len capacity].map do |name|
    ["ext/makiri/rust/nupur/src/int.rs", "ext/makiri/rust/verus/int.rs", name]
  end,
  *%w[as_slice append append_growing raise_limit reserve steal content_limit
      content_limit_for grow_to].map do |name|
    ["ext/makiri/rust/engine/src/ceiling_buf.rs", "ext/makiri/rust/verus/ceiling_buf.rs", "Buf::#{name}"]
  end
].freeze
# The files `rake verus` verifies.
VERUS_PROOFS = %w[ptr_table.rs ceiling_buf.rs limits.rs utf8.rs xpath_number.rs xml_chars.rs swar.rs
                     source_lines.rs xml_expand.rs int.rs].freeze

# The bodies of every `fn name` in `src` - `Type::name` for the ones in
# `impl ... Type` - as tokens: lines marked `// [verus]` and comments (line
# and block) dropped, from the brace that opens a body to the one closing it,
# and a trailing comma before a closing bracket dropped too - `cargo fmt`
# adds one when it breaks a call across lines, and it changes nothing. A
# proof may hold several copies of one function (`int.rs` holds one per width
# of a C `long`); each must be the source's.
#
# A `#[verifier::truncate]` and the parentheses it needs around its cast are
# dropped as well: the attribute tells Verus that the cast wraps, which is
# what `as` does in Rust, and changes nothing that runs.
def verus_bodies(src, name, all: false)
  text = src.lines.reject { |l| l.rstrip.end_with?("// [verus]") }
            .map { |l| l.sub(%r{//.*$}, "") }.join.gsub(%r{/\*.*?\*/}m, " ")
  type, name = name.include?("::") ? name.split("::", 2) : [nil, name]
  fn = /\bfn\s+#{Regexp.escape(name)}\b/
  ats = if type
          from = text.index(/^\s*impl\b[^{]*\b#{Regexp.escape(type)}\b\s*[<{]/) or return []
          [text.index(fn, from)].compact
        elsif all
          text.enum_for(:scan, fn).map { Regexp.last_match.begin(0) }
        else
          [text.index(fn)].compact
        end
  ats.filter_map { |at| verus_body_at(text, at) }
end

def verus_body_at(text, at)
  open = text.index("{", at) or return nil
  depth = 0
  close = (open...text.length).find do |i|
    depth += 1 if text[i] == "{"
    depth -= 1 if text[i] == "}"
    depth.zero?
  end
  return nil unless close

  toks = verus_drop_truncate(text[open..close].scan(/\w+|[^\s\w]/))
  toks.each_index.reject { |i| toks[i] == "," && ")]}".include?(toks[i + 1].to_s) }.map { |i| toks[i] }
end

VERUS_TRUNCATE = %w[# \[ verifier : : truncate \] (].freeze

def verus_drop_truncate(toks)
  out = []
  closes = [] # depths at which a dropped `(` closes
  depth = 0
  i = 0
  while i < toks.length
    if toks[i, VERUS_TRUNCATE.length] == VERUS_TRUNCATE
      depth += 1
      closes << depth
      i += VERUS_TRUNCATE.length
      next
    end
    t = toks[i]
    if t == "("
      depth += 1
    elsif t == ")"
      if closes.last == depth
        closes.pop
        depth -= 1
        i += 1
        next
      end
      depth -= 1
    end
    out << t
    i += 1
  end
  out
end

namespace :verus do
  desc "Check that the Verus copies match the source they were copied from"
  task :sync do
    VERUS_COPIES.each do |source, copy, name|
      want = verus_bodies(File.read(source), name).first or abort "verus: no fn #{name} in #{source}"
      got = verus_bodies(File.read(copy), name, all: true)
      abort "verus: no fn #{name} in #{copy}" if got.empty?
      next if got.all?(want)

      abort "verus: #{copy}'s #{name} is not #{source}'s - redo the copy and its proof"
    end
    puts "verus: the copies match their sources"
  end
end

# What each proof takes on trust, pinned: a `#[verifier::external_body]` (a
# body Verus does not read, its spec assumed), an `assume_specification`
# (std's spec assumed), an `assume(` or `admit(`. Each one is named in its
# file's module doc; a new one is a new axiom, and this count moves with it on
# purpose. The canaries may hold none.
VERUS_TRUSTED = { "ptr_table.rs" => 4, "ceiling_buf.rs" => 4, "limits.rs" => 0, "utf8.rs" => 0,
                  "xpath_number.rs" => 0, "xml_chars.rs" => 0,
                  "swar.rs" => 0, "source_lines.rs" => 1, "xml_expand.rs" => 3,
                  "int.rs" => 1 }.freeze
# Verus's `admit()` takes no argument, which tells it from a function named
# `admit` (`limits.rs`).
VERUS_TRUST = /#\[verifier::external(?:_body)?\]|\bassume_specification\b|\bassume\(|\badmit\(\s*\)/

def verus_trusted_count(src)
  src.gsub(%r{/\*.*?\*/}m, "").lines.map { |l| l.sub(%r{//.*$}, "") }.join.scan(VERUS_TRUST).length
end

# A proof is vacuous when what it assumes - an axiom, a precondition, a fact
# met along the way - contradicts itself: then every claim after it verifies.
# Verus has no `cover`, so `rake verus` checks the same way the dylint
# self-test does, by a check that must FAIL:
#
# - each `// [canary]` line of a proof marks a point the proved code reaches;
#   for each, the proof is verified again with an `assert(false)` there, which
#   must be the one thing that fails. (One variant per mark: a failed assert is
#   assumed after, so two in one run would hide the second.) This is what sees
#   a contradiction inside a body, which the body's callers cannot - they see
#   only its `ensures`;
# - verus/canaries/<proof> adds `canary_` functions that call the proved code
#   from outside and `assert(false)` on each outcome, so a precondition no
#   caller can meet, or `ensures` that contradict, is caught too.
def verus_assertions_failing(verus, path)
  out, = Open3.capture2e(verus, path)
  lines = out.scan(/^error: assertion failed\s*\n\s*--> [^\n]*:(\d+):/).flatten.map(&:to_i)
  errors = out[/verification results:: \d+ verified, (\d+) errors?/, 1]
  [lines, errors&.to_i, out]
end

def verus_canaries(verus, proof)
  dir = File.join(ROOT, "tmp/verus_canaries")
  FileUtils.mkdir_p(dir)
  src = File.read(proof)
  lines = src.lines
  marks = lines.each_index.select { |i| lines[i].strip == "// [canary]" }
  abort "verus: #{proof} has no `// [canary]` marks" if marks.empty?

  jobs = marks.map do |i|
    variant = lines.dup
    variant[i] = lines[i].sub("// [canary]", "assert(false); // [canary]")
    path = File.join(dir, "#{File.basename(proof, '.rs')}_mark#{i + 1}.rs")
    File.write(path, variant.join)
    [path, [i + 1]]
  end

  canary = File.read("canaries/#{proof}")
  at = src.rindex("} // verus!") or abort "verus: #{proof} has no `} // verus!` to put the canaries before"
  first = src[0...at].count("\n") + 1
  outside = File.join(dir, proof)
  File.write(outside, src[0...at] + canary + src[at..])
  want = (canary.lines.each_index.select { |i| canary.lines[i].include?("assert(false)") }).map { |i| first + i }
  jobs << [outside, want]

  queue = Queue.new
  jobs.each { |j| queue << j }
  failures = Queue.new
  Array.new(4) do
    Thread.new do
      while (job = (queue.pop(true) rescue nil))
        path, expected = job
        got, errors, out = verus_assertions_failing(verus, path)
        failures << [path, expected, got, out] unless got.sort == expected.sort && errors == expected.length
      end
    end
  end.each(&:join)
  return puts("verus: #{proof}: #{marks.length} marks and #{want.length} canaries, each failing where it must") if failures.empty?

  failures.size.times do
    path, expected, got, out = failures.pop
    puts out
    puts "verus: #{path}: expected assert(false) to fail at lines #{expected}, failed at #{got}"
  end
  abort "verus: #{proof}: a canary verified - an assumption the proof rests on proves anything there"
end

desc "Verify the Verus proofs (ext/makiri/rust/verus/; needs Verus #{VERUS_VERSION})"
task verus: "verus:sync" do
  require "open3"
  verus = ENV.fetch("VERUS", "verus")
  Dir.chdir("ext/makiri/rust/verus") do
    trusted = VERUS_PROOFS.to_h { |f| [f, verus_trusted_count(File.read(f))] }
    abort "verus: what the proofs take on trust changed: #{trusted} (recorded #{VERUS_TRUSTED})" \
      unless trusted == VERUS_TRUSTED
    untrusting = VERUS_PROOFS.reject { |f| verus_trusted_count(File.read("canaries/#{f}")).zero? }
    abort "verus: canaries may assume nothing: #{untrusting}" unless untrusting.empty?

    VERUS_PROOFS.each do |f|
      sh verus, f
      verus_canaries(verus, f)
    end
  end
  puts "verus: every proof verified, its canaries failed as they must; trusted #{VERUS_TRUSTED}"
end
