# frozen_string_literal: true
#
# Robustness fuzzer for Makiri's query surface.
#
# Makiri has no second engine to diff against (that is the whole point - no
# libxml2), so this fuzzer targets *robustness* rather than differential
# correctness: it throws generated and mutated XPath / CSS at fixture
# documents and asserts that Makiri NEVER does anything worse than raise one of
# its own exceptions. Concretely, a finding is:
#
#   * unexpected - a non-Makiri Ruby exception leaked (a contract violation:
#     malformed input must surface as Makiri::Error, not TypeError/EncodingError/...);
#   * crash      - the worker died from a signal (segfault, abort) - only
#     catchable with --isolated;
#   * timeout    - the query did not return within --query-timeout (a missing
#     budget / runaway loop) - only catchable with --isolated.
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
require_relative "xml_corpus"
require_relative "mutate"

REGRESSIONS_DIR = File.expand_path("regressions", __dir__)
STATE_FILE      = File.join(REGRESSIONS_DIR, "last_input.txt")
FileUtils.mkdir_p(REGRESSIONS_DIR)

opts = { time: 60, seed: Random.new_seed, isolated: false, quiet: false,
         query_timeout: 5, target: :xpath }
OptionParser.new do |o|
  o.banner = "Usage: ruby spec/fuzz/run.rb [options]"
  o.on("--time SEC", Integer, "seconds to run (default 60)")          { |v| opts[:time] = v }
  o.on("--seed N", Integer, "RNG seed (default random)")              { |v| opts[:seed] = v }
  o.on("--target T", %i[xpath css both xml mutate xmlcss], "xpath (default), css, both, xml, mutate, xmlcss") { |v| opts[:target] = v }
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

# A single namespaced XML fixture for the :xmlcss target - the same generated CSS
# torrent the :css target throws at HTML is run against this through the native
# XPath-engine CSS path (mkr_css.c), exercising the AST builder for robustness.
XMLCSS_SRC = <<~XML
  <feed xmlns="urn:atom" xmlns:dc="urn:dc">
    <entry id="e1" class="lead big" dc:role="x"><title lang="en">A</title><link href="/p/1"/></entry>
    <entry id="e2" class="big"><title>B</title><sub><deep/></sub></entry>
    <dc:meta>m</dc:meta>
    <empty/>
  </feed>
XML
xmlcss_doc = (Makiri::XML(XMLCSS_SRC) rescue nil)

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
# Runs both the full-set and the first-match (#at_*) path so the engine's
# first-match short-circuit gets the same fuzz coverage as the full evaluator.
def run_inproc(doc, target, query)
  if target == :css
    doc.css(query); doc.at_css(query)
  else
    doc.xpath(query); doc.at_xpath(query)
  end
  [:ok, nil]
rescue Makiri::Error => e
  [:expected, e.class.name] # malformed input rejected cleanly - fine
rescue StandardError => e
  [:unexpected, "#{e.class}: #{e.message}"] # contract violation - a finding
end

def run_isolated(doc, target, query, timeout_sec)
  rd, wr = IO.pipe
  pid = fork do
    rd.close
    result =
      begin
        if target == :css
          doc.css(query); doc.at_css(query)
        else
          doc.xpath(query); doc.at_xpath(query)
        end
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

# --- XML parser fuzzing (target :xml) -------------------------------------
# Here the "query" IS the document: we parse hostile/mutated XML and assert the
# parser fails closed (Makiri::Error) rather than crashing or leaking a foreign
# exception. A returned Document is fine (the input happened to be well-formed).

def run_xml_inproc(input)
  Makiri::XML(input)
  [:ok, nil]
rescue Makiri::Error => e
  [:expected, e.class.name] # malformed/hostile input rejected cleanly - fine
rescue StandardError => e
  [:unexpected, "#{e.class}: #{e.message}"] # contract violation - a finding
end

def run_xml_isolated(input, timeout_sec)
  rd, wr = IO.pipe
  pid = fork do
    rd.close
    result =
      begin
        Makiri::XML(input)
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

# --- XML mutation fuzzing (target :mutate) --------------------------------
# Here the "query" is a seed: MutateFuzz.run applies a deterministic random edit
# sequence to a fresh document and checks the structural invariants (no cycle,
# link consistency, serialization terminates + is a stable fixed point). It
# returns [category, detail] directly.

def run_mutate_inproc(seed)
  MutateFuzz.run(seed)
rescue StandardError => e
  [:unexpected, "#{e.class}: #{e.message}"]
end

def run_mutate_isolated(seed, timeout_sec)
  rd, wr = IO.pipe
  pid = fork do
    rd.close
    result =
      begin
        MutateFuzz.run(seed)
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

  if opts[:target] == :xml
    target = :xml
    query  = XmlFuzz.next_input(rng)
    name   = "xml-input"
    html   = query
  elsif opts[:target] == :mutate
    target = :mutate
    query  = MutateFuzz.next_seed(rng)
    name   = "mutate"
    html   = "seed: #{query}\n# replay: ruby -Ilib -r makiri -r ./spec/fuzz/mutate -e 'p MutateFuzz.run(#{query})'"
  elsif opts[:target] == :xmlcss
    target = :css                       # same CSS execution path, against an XML doc
    query  = next_query(:css, rng)[1]
    name   = "xml-css"
    html   = XMLCSS_SRC
    doc    = xmlcss_doc
  else
    target, query = next_query(opts[:target], rng)
    name, html, doc = fixtures.sample(random: rng)
  end
  File.write(STATE_FILE, "fixture: #{name}\ntarget: #{target}\nquery: #{query.inspect}\n") rescue nil

  category, detail =
    if target == :xml
      opts[:isolated] ? run_xml_isolated(query, opts[:query_timeout])
                      : run_xml_inproc(query)
    elsif target == :mutate
      opts[:isolated] ? run_mutate_isolated(query, opts[:query_timeout])
                      : run_mutate_inproc(query)
    elsif opts[:isolated]
      run_isolated(doc, target, query, opts[:query_timeout])
    else
      run_inproc(doc, target, query)
    end
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
