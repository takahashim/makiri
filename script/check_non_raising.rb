# frozen_string_literal: true

# `rake dylint`'s audit of the tables of Ruby's C functions that cannot raise
# (`NON_RAISING` and `REVIEWED`, ext/makiri/rust/lints/makiri_lints/src/raising.rs).
#
# Against the C API raise survey (https://takahashim.github.io/ruby-c-apis/:
# the call graph of the compiled C, over the latest release of each Ruby since
# 3.3 and HEAD, Linux and macOS), as vendored in raise_survey.json
# (`script/update_raise_survey.rb` refreshes it):
#
# - a NON_RAISING entry fails unless the survey finds no path to a longjmp
#   for it in any run (`no_path_all`);
# - a REVIEWED entry fails without a reason, and when the survey finds no
#   path for it any more (it belongs in NON_RAISING);
# - a function the survey does not know fails either table.
#
# And against what Ruby's own headers say of each, as the bindings rb-sys
# generates keep their doc comments and `noreturn`. An entry of either table
# fails when
#
# - no function of that name is in the bindings (a typo, or one Ruby dropped),
# - its doc names an exception (`@exception`), or
# - it never returns (`-> !`): it leaves by a raise or a jump, always.
#
# The headers say less than the truth, and the survey reads the call graph
# at its most conservative: a function with a path may still raise nothing
# (that is what REVIEWED is for), and one passing here is as sure as the call
# graph is - the survey's own limits apply (one config per OS, no extension
# callbacks). The bindings read are the ones nupur compiles against: rb-sys's
# OUT_DIR, as cargo reports it.

require "json"
require "open3"

RUST = File.expand_path("../ext/makiri/rust", __dir__)
TABLE = File.join(RUST, "lints/makiri_lints/src/raising.rs")

SURVEY = File.join(RUST, "lints/makiri_lints/raise_survey.json")

def tables
  src = File.read(TABLE)
  non_raising = src[/^const NON_RAISING: &\[&str\] = &\[\n(.*?)^\];/m, 1] or
    abort "check_non_raising: no NON_RAISING table in #{TABLE}"
  reviewed = src[/^const REVIEWED: &\[\(&str, &str\)\] = &\[\n(.*?)^\];/m, 1] or
    abort "check_non_raising: no REVIEWED table in #{TABLE}"
  [non_raising.scan(/^\s*"([a-z0-9_]+)",/).flatten,
   reviewed.scan(/^    \(\n\s*"([a-z0-9_]+)",\n(.*?)^    \),/m).to_h { |n, r| [n, r.scan(/"((?:[^"\\]|\\.)*)"/m).join.strip] }]
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

non_raising, reviewed = tables
names = non_raising + reviewed.keys
dups = names.tally.select { |_, n| n > 1 }.keys
survey = JSON.parse(File.read(SURVEY))
verdict = survey["functions"]
path = bindings_file
fns = functions(path)
failures = []
failures += dups.map { |n| "#{n}: in both tables, or twice" }
non_raising.each do |n|
  case verdict[n]
  when "no_path_all" then nil
  when nil then failures << "#{n}: not in the survey"
  else failures << "#{n}: the survey finds a path to a longjmp (#{verdict[n]}); review it and move it to REVIEWED with the reason, or take it out"
  end
end
reviewed.each do |n, reason|
  failures << "#{n}: no reason" if reason.empty?
  case verdict[n]
  when nil then failures << "#{n}: not in the survey"
  when "no_path_all" then failures << "#{n}: the survey finds no path any more; move it to NON_RAISING"
  end
end
names.each do |name|
  f = fns[name] or (failures << "#{name}: not in the bindings"; next)
  failures << "#{name}: its doc names an exception (@exception)" if f[:doc].include?("@exception") && !reviewed.key?(name)
  failures << "#{name}: it never returns (-> !)" if f[:sig].match?(/\)\s*->\s*!\s*;/)
end
unless failures.empty?
  puts "check_non_raising: the tables disagree with the survey or Ruby's headers:"
  failures.each { |f| puts "  #{f}" }
  abort
end
s = survey["survey"]
puts "check_non_raising: #{non_raising.size} entries with no path in the survey, " \
     "#{reviewed.size} reviewed (survey #{s['commit'][0, 7]}, releases #{s['releases'].join(', ')})"
