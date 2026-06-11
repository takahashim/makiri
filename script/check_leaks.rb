# frozen_string_literal: true

# Malloc-leak gate for the C extension (macOS only; run via `rake leaks`).
#
# ASan runs everywhere with detect_leaks=0 (Ruby and Lexbor are uninstrumented,
# so LeakSanitizer drowns in their noise) - which means plain leaks were never
# machine-checked. This gate fills that hole with macOS's `leaks` tool: it runs
# script/leaks_harness.rb (the public surface in a loop, INCLUDING rescued
# failure paths) under MallocStackLogging, then scans the leak report for
# allocation stacks that (a) pass through makiri.bundle and (b) repeat in
# proportion to the loop count - i.e. a leak per call, which is what a missing
# free on some path looks like. One-time Init allocations and Ruby's own
# at-exit/unscannable noise stay at 1-2 instances and are ignored.
#
# Found (and since fixed) by this harness: the transient fragment document
# leaked by every inner_html=/outer_html=, and the partially-built step leaked
# by every failing XPath parse.
#
#   ruby script/check_leaks.rb            # threshold = ITERATIONS / 4
#   LEAKS_ITERATIONS=200 rake leaks       # more iterations, sharper signal

require "rbconfig"
require "tempfile"

abort "check_leaks: the `leaks` tool is macOS-only" unless RUBY_PLATFORM.include?("darwin")

iterations = Integer(ENV.fetch("LEAKS_ITERATIONS", "120"))
threshold  = [iterations / 4, 10].max
harness    = File.expand_path("leaks_harness.rb", __dir__)
lib        = File.expand_path("../lib", __dir__)

out = Tempfile.create("makiri-leaks")
ok = system({ "MallocStackLogging" => "1", "LEAKS_ITERATIONS" => iterations.to_s },
            "leaks", "--atExit", "--",
            RbConfig.ruby, "-I#{lib}", harness,
            out: out.path, err: out.path)
report = File.read(out.path)
# `leaks` exits non-zero whenever ANY leak exists (Ruby itself always reports
# some at-exit noise), so the exit status is not the verdict - the scan below
# is. But the harness itself must have completed.
abort "check_leaks: harness did not complete:\n#{report[-2000..]}" unless report.include?("leaks harness done")
abort "check_leaks: no leak report produced (leaks tool failed?)" unless report.include?("STACK OF") || ok

offenders = report.split(/\n(?=STACK OF )/).filter_map do |stanza|
  next unless stanza.include?("makiri.bundle")

  instances = stanza[/STACK OF (\d+) INSTANCES?/, 1].to_i
  next if instances < threshold

  frames = stanza.lines.grep(/makiri\.bundle/).first(4).map(&:strip)
  [instances, frames]
end

if offenders.empty?
  puts "check_leaks: OK - no repeated (>= #{threshold}x) leak stacks through makiri.bundle " \
       "(#{iterations} iterations)"
else
  puts "check_leaks: FAILED - #{offenders.size} repeated leak stack(s) through makiri.bundle:"
  offenders.each do |instances, frames|
    puts "  #{instances}x:"
    frames.each { |f| puts "    #{f}" }
  end
  exit 1
end
