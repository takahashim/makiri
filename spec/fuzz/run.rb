# frozen_string_literal: true
#
# Robustness fuzzer for Makiri's query surface.
#
# Makiri has no second engine to diff against (that is the whole point — no
# libxml2), so this fuzzer targets *robustness* rather than differential
# correctness: it throws generated and mutated XPath / CSS at fixture
# documents and asserts that Makiri NEVER does anything worse than raise one of
# its own exceptions. Concretely, a finding is:
#
#   * unexpected — a non-Makiri Ruby exception leaked (a contract violation:
#     malformed input must surface as Makiri::Error, not TypeError/EncodingError/…);
#   * crash      — the worker died from a signal (segfault, abort) — only
#     catchable with --isolated;
#   * timeout    — the query did not return within --query-timeout (a missing
#     budget / runaway loop) — only catchable with --isolated.
#
# Run it under AddressSanitizer (see the project's `rake sanitize` machinery)
# to turn latent memory errors into crash findings.
#
# Usage:
#   ruby -Ilib spec/fuzz/run.rb                       # 60 s, random seed, XPath
#   ruby -Ilib spec/fuzz/run.rb --time 300            # 5 minutes
#   ruby -Ilib spec/fuzz/run.rb --seed 42             # deterministic
#   ruby -Ilib spec/fuzz/run.rb --target css          # fuzz CSS instead
#   ruby -Ilib spec/fuzz/run.rb --target both
#   ruby -Ilib spec/fuzz/run.rb --isolated            # fork per query (crash/hang safe)

require "fileutils"
require "digest"
require "optparse"

$LOAD_PATH.unshift File.expand_path("../../lib", __dir__)
require "makiri"

require_relative "grammar"
require_relative "fixtures"
require_relative "seed_corpus"

REGRESSIONS_DIR = File.expand_path("regressions", __dir__)
STATE_FILE      = File.join(REGRESSIONS_DIR, "last_input.txt")
FileUtils.mkdir_p(REGRESSIONS_DIR)

opts = { time: 60, seed: Random.new_seed, isolated: false, quiet: false,
         query_timeout: 5, target: :xpath }
OptionParser.new do |o|
  o.banner = "Usage: ruby spec/fuzz/run.rb [options]"
  o.on("--time SEC", Integer, "seconds to run (default 60)")          { |v| opts[:time] = v }
  o.on("--seed N", Integer, "RNG seed (default random)")              { |v| opts[:seed] = v }
  o.on("--target T", %i[xpath css both], "xpath (default), css, both") { |v| opts[:target] = v }
  o.on("--isolated", "fork per query (crash/hang safe)")              { opts[:isolated] = true }
  o.on("--query-timeout SEC", Integer, "per-query timeout in --isolated (default 5)") { |v| opts[:query_timeout] = v }
  o.on("-q", "--quiet", "suppress per-finding output")               { opts[:quiet] = true }
end.parse!(ARGV)

rng = Random.new(opts[:seed])
puts "fuzz: seed=#{opts[:seed]} time=#{opts[:time]}s target=#{opts[:target]} isolated=#{opts[:isolated]}"

fixtures = FuzzFixtures.all.map do |name, html|
  [name, html, Makiri::HTML(html)]
rescue StandardError => e
  warn "skip fixture #{name}: #{e.message}"
  nil
end.compact
abort "no fixtures loaded" if fixtures.empty?

# --- query generation -----------------------------------------------------

def next_query(target, rng)
  target = %i[xpath css].sample(random: rng) if target == :both
  if target == :css
    rng.rand(100) < 70 ? Grammar.mutate(CSS_SEED.sample(random: rng), rng)
                        : CSS_SEED.sample(random: rng)
  else
    case rng.rand(100)
    when 0...60 then Grammar.mutate(SEED_CORPUS.sample(random: rng), rng)
    when 60...90 then Grammar.gen_xpath(rng.rand(2..5), rng)
    else SEED_CORPUS.sample(random: rng)
    end
  end
  .then { |q| [target, q] }
end

# --- execution + categorization -------------------------------------------

# Returns [category, detail]. Categories: :ok, :expected, :unexpected.
def run_inproc(doc, target, query)
  target == :css ? doc.css(query) : doc.xpath(query)
  [:ok, nil]
rescue Makiri::Error => e
  [:expected, e.class.name] # malformed input rejected cleanly — fine
rescue StandardError => e
  [:unexpected, "#{e.class}: #{e.message}"] # contract violation — a finding
end

def run_isolated(doc, target, query, timeout_sec)
  rd, wr = IO.pipe
  pid = fork do
    rd.close
    result =
      begin
        target == :css ? doc.css(query) : doc.xpath(query)
        [:ok, nil]
      rescue Makiri::Error => e
        [:expected, e.class.name]
      rescue StandardError => e
        [:unexpected, "#{e.class}: #{e.message}"&.slice(0, 2000)]
      end
    wr.write(Marshal.dump(result))
    wr.close
    exit!(0)
  end
  wr.close

  unless IO.select([rd], nil, nil, timeout_sec)
    Process.kill("KILL", pid) rescue nil
    Process.wait(pid)
    rd.close
    return [:timeout, "no response in #{timeout_sec}s"]
  end

  data = rd.read
  Process.wait(pid)
  rd.close
  if $?.signaled? || !$?.exitstatus.zero?
    return [:crash, $?.signaled? ? "signal #{$?.termsig}" : "exit #{$?.exitstatus}"]
  end
  Marshal.load(data)
rescue StandardError
  [:crash, "corrupt worker response"]
end

def save_regression(name, html, target, query, category, detail)
  hash = Digest::SHA1.hexdigest("#{name}|#{target}|#{query}")[0, 10]
  dir  = File.join(REGRESSIONS_DIR, "#{category}_#{hash}")
  return false if File.exist?(dir)

  FileUtils.mkdir_p(dir)
  File.write(File.join(dir, "document.html"), html)
  File.write(File.join(dir, "query.txt"), "#{target}\n#{query}")
  File.write(File.join(dir, "note.md"),
             "category: #{category}\nfixture: #{name}\ntarget: #{target}\n\n```\n#{detail}\n```\n")
  true
end

# --- main loop -------------------------------------------------------------

stats = Hash.new(0)
saved = 0
t0    = Time.now
next_heartbeat = t0 + 10

while Time.now - t0 < opts[:time]
  now = Time.now
  if now >= next_heartbeat
    rate = (stats[:runs] / [now - t0, 0.01].max).round
    warn "  [heartbeat #{(now - t0).round}s] runs=#{stats[:runs]} (#{rate}/s) findings=#{saved}"
    next_heartbeat = now + 10
  end

  target, query = next_query(opts[:target], rng)
  name, html, doc = fixtures.sample(random: rng)
  File.write(STATE_FILE, "fixture: #{name}\ntarget: #{target}\nquery: #{query}\n") rescue nil

  category, detail =
    opts[:isolated] ? run_isolated(doc, target, query, opts[:query_timeout])
                    : run_inproc(doc, target, query)
  stats[category] += 1
  stats[:runs] += 1

  next if %i[ok expected].include?(category)

  if save_regression(name, html, target, query, category, detail)
    saved += 1
    puts "  [#{category}] #{target} #{query.inspect[0, 80]} -- #{detail}" unless opts[:quiet]
  end
end

File.delete(STATE_FILE) if File.exist?(STATE_FILE)
elapsed = Time.now - t0

puts
puts "ran #{stats[:runs]} queries in #{elapsed.round(1)}s (#{(stats[:runs] / [elapsed, 0.01].max).round}/s)"
puts "  ok=#{stats[:ok]} expected=#{stats[:expected]} " \
     "unexpected=#{stats[:unexpected]} crash=#{stats[:crash]} timeout=#{stats[:timeout]}"
puts "  new regressions saved: #{saved}"
puts "  see #{REGRESSIONS_DIR}/" if saved.positive?

exit(saved.positive? ? 1 : 0)
