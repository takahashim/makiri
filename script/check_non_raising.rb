# frozen_string_literal: true

# `rake dylint`'s audit of the table of Ruby's C functions that cannot raise
# (`NON_RAISING`, ext/makiri/rust/lints/makiri_lints/src/raising.rs), against
# what Ruby's own headers say of each: the bindings rb-sys generates keep the
# headers' doc comments and their `noreturn`. An entry fails when
#
# - no function of that name is in the bindings (a typo, or one Ruby
#   dropped),
# - its doc names an exception (`@exception`), or
# - it never returns (`-> !`): it leaves by a raise or a jump, always.
#
# The headers say less than the truth - an allocation's `NoMemoryError` is
# seldom written down - so passing is not a proof that a function cannot
# raise; failing is a proof that the table is wrong. The bindings read are
# the ones nupur compiles against: rb-sys's OUT_DIR, as cargo reports it.

require "json"
require "open3"

RUST = File.expand_path("../ext/makiri/rust", __dir__)
TABLE = File.join(RUST, "lints/makiri_lints/src/raising.rs")

def table
  src = File.read(TABLE)
  body = src[/^const NON_RAISING: &\[&str\] = &\[\n(.*?)^\];/m, 1] or
    abort "check_non_raising: no NON_RAISING table in #{TABLE}"
  body.scan(/^\s*"([a-z0-9_]+)",/).flatten
end

def bindings_file
  out, status = Open3.capture2("cargo", "check", "-p", "nupur", "--message-format=json", chdir: RUST)
  abort "check_non_raising: cargo check -p nupur failed" unless status.success?
  dirs = out.each_line.filter_map do |line|
    msg = JSON.parse(line)
    next unless msg["reason"] == "build-script-executed"
    next unless msg["package_id"].to_s.match?(%r{[/#]rb-sys[@#]|^rb-sys })

    msg["out_dir"]
  rescue JSON::ParserError
    nil
  end
  files = dirs.flat_map { |d| Dir[File.join(d, "bindings-*.rs")] }.uniq
  abort "check_non_raising: no rb-sys bindings found (#{dirs.inspect})" if files.empty?
  abort "check_non_raising: more than one set of bindings: #{files.inspect}" if files.size > 1
  files.first
end

# Each `pub fn` of the bindings: its doc lines and its signature.
def functions(path)
  fns = {}
  docs = []
  lines = File.readlines(path)
  i = 0
  while i < lines.size
    line = lines[i]
    if line =~ /^\s*#\[doc = (".*")\]\s*$/
      docs << line
    elsif (m = line.match(/^\s*pub fn ([A-Za-z0-9_]+)\s*\(/))
      sig = +line
      sig << lines[i += 1] until sig.include?(";") || i + 1 >= lines.size
      fns[m[1]] = { doc: docs.join, sig: sig }
      docs = []
    elsif !line.strip.start_with?("#[")
      docs = []
    end
    i += 1
  end
  fns
end

names = table
path = bindings_file
fns = functions(path)
failures = names.filter_map do |name|
  f = fns[name] or next "#{name}: not in the bindings"
  next "#{name}: its doc names an exception (@exception)" if f[:doc].include?("@exception")
  next "#{name}: it never returns (-> !)" if f[:sig].match?(/\)\s*->\s*!\s*;/)

  nil
end
unless failures.empty?
  puts "check_non_raising: NON_RAISING disagrees with Ruby's headers (#{File.basename(path)}):"
  failures.each { |f| puts "  #{f}" }
  abort
end
puts "check_non_raising: #{names.size} entries, none documented to raise (#{File.basename(path)})"
