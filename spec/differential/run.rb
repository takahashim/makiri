# frozen_string_literal: true

# Differential runner: compare this build's answers against a recorded baseline.
#
# Every probe under probes/ prints one line per (fixture, operation) plus a
# branch-count summary, so two builds can be diffed byte for byte. The baselines
# under baseline/ were RECORDED FROM THE C BUILD, which is the point: the C is
# being retired, and once it is gone these files are the only surviving record
# of what it answered.
#
# That matters more than it sounds. This port's cheapest and most effective
# regression check was "build both, diff" - it caught an ASCII-8BIT string, a
# quoted error message and a lost NUL terminator that the 1001-example suite all
# passed over. Recording the C's answers keeps that check working after the C
# itself is deleted.
#
#   bundle exec rake diff           # compare this build against the baseline
#   bundle exec rake diff:record    # rebuild C-only and re-record
#
# A probe's own branch counts are part of the comparison, so a probe that stops
# exercising a branch fails rather than silently agreeing about less.

require "fileutils"

ROOT = File.expand_path("../..", __dir__)
PROBES = File.join(__dir__, "probes")
BASELINE = File.join(__dir__, "baseline")

# The configuration a baseline was recorded under, written into its header so a
# file recorded from the wrong build is visible rather than merely wrong.
BASELINE_CONFIG = "C only (no MAKIRI_RUST_* flags)"

def probe_names
  Dir[File.join(PROBES, "*.rb")].sort.map { |p| File.basename(p, ".rb") }
end

def run_probe(name)
  out = IO.popen(
    [RbConfig.ruby, "-I#{File.join(ROOT, "lib")}", File.join(PROBES, "#{name}.rb")],
    err: [:child, :out],
    &:read
  )
  [out, $?.success?]
end

def baseline_path(name) = File.join(BASELINE, "#{name}.txt")

def header
  "# Recorded from: #{BASELINE_CONFIG}\n" \
    "# Regenerate with `bundle exec rake diff:record`. See spec/differential/run.rb.\n"
end

def record
  FileUtils.mkdir_p(BASELINE)
  probe_names.each do |name|
    out, ok = run_probe(name)
    unless ok
      warn "diff:record: probe #{name} failed:\n#{out}"
      exit 1
    end
    File.write(baseline_path(name), header + out)
    puts format("recorded %-14s %d lines", name, out.lines.size)
  end
  puts "diff:record: #{probe_names.size} baselines recorded from #{BASELINE_CONFIG}"
end

def compare
  names = probe_names
  if names.empty?
    warn "diff: no probes under #{PROBES}"
    exit 1
  end

  failures = []
  names.each do |name|
    path = baseline_path(name)
    unless File.exist?(path)
      failures << "#{name}: no baseline (run `rake diff:record`)"
      next
    end

    out, ok = run_probe(name)
    unless ok
      failures << "#{name}: the probe itself failed:\n#{out}"
      next
    end

    want = File.read(path).lines.reject { |l| l.start_with?("# ") }.join
    if out == want
      puts format("ok       %-14s %d lines", name, out.lines.size)
      next
    end

    # Show the first few differing lines - a whole-file diff of 8000 lines
    # buries the one that matters.
    diff = want.lines.zip(out.lines).each_with_index.reject { |(a, b), _| a == b }
    shown = diff.first(5).map do |(a, b), i|
      "    line #{i + 1}:\n      baseline #{a.inspect}\n      this     #{b.inspect}"
    end
    failures << "#{name}: #{diff.size} of #{[want.lines.size, out.lines.size].max} lines differ\n" +
                shown.join("\n")
  end

  if failures.empty?
    puts "diff: OK - #{names.size} probes match the #{BASELINE_CONFIG} baseline"
    exit 0
  end

  warn "diff: FAILED"
  failures.each { |f| warn "  - #{f}" }
  exit 1
end

case ARGV[0]
when "record" then record
when nil, "compare" then compare
else
  warn "usage: run.rb [compare|record]"
  exit 2
end
