# frozen_string_literal: true
#
# The CSS engines on one document read as HTML and as XML: the fixture the
# CSS unification (notes/css_engine_unification_plan.ja.md) is measured
# against, before and after each step, with the same answers on both sides.
#
# Each row is the MIN CPU time per call over ROUNDS rounds of a fixed number
# of calls (benchmark-ips swings 15-25% on this kind of machine; the minimum
# is what is repeatable). Two conditions:
#   warm - the same selector string every call, so a cached parse is reused
#   cold - a distinct string every call (the selector and a numbered
#          comment, which parses to the same selector), so no cache can serve it
#
# Usage:
#   ruby -Ilib bench/bench_css.rb                  # print the table
#   ruby -Ilib bench/bench_css.rb --json out.json  # also save it
#   ruby -Ilib bench/bench_css.rb --compare a.json # print the change against a saved run
#   ROUNDS=9 ruby -Ilib bench/bench_css.rb

require "json"
require "optparse"
require "rbconfig"
require "makiri"

opts = {}
OptionParser.new do |o|
  o.on("--json PATH", "save the results") { |v| opts[:json] = v }
  o.on("--compare PATH", "compare with a saved run") { |v| opts[:compare] = v }
end.parse!(ARGV)

ROUNDS = Integer(ENV.fetch("ROUNDS", "7"))
LISTS = 2000

body = (1..LISTS).map do |i|
  %(<ul class="l"><li class="item r#{i % 5}" data-id="#{i}"><a href="x">#{i}</a></li><li><span></span></li></ul>)
end.join
HTML_DOC = Makiri::HTML("<!DOCTYPE html><html><body>#{body}</body></html>")
XML_DOC = Makiri::XML("<html><body>#{body}</body></html>")

# [label, selector, method, calls per round]
ROWS = [
  ["css li", "li", :css, 40],
  ["css ul li.item", "ul li.item", :css, 20],
  ["css li > a", "li > a", :css, 40],
  ["css [data-id]", "[data-id]", :css, 40],
  ["css .item", ".item", :css, 20],
  ["css li:nth-child(2)", "li:nth-child(2)", :css, 20],
  ["css li:not(.r1)", "li:not(.r1)", :css, 20],
  ["css ul:has(> li > a)", "ul:has(> li > a)", :css, 20],
  ["at_css li", "li", :at_css, 20_000],
  ["at_css a[href]", "a[href]", :at_css, 20_000],
  ["matches? li.item", "li.item", :matches, 20_000]
].freeze

def cpu
  Process.clock_gettime(Process::CLOCK_PROCESS_CPUTIME_ID)
end

def run(doc, sel, meth, calls, cold)
  target = meth == :matches ? doc.at_css("li") : doc
  serial = 0
  best = Float::INFINITY
  ROUNDS.times do
    # Numbered across rounds too, so no round finds the last one's strings.
    sels = cold ? Array.new(calls) { "#{sel}/*#{serial += 1}*/" } : nil
    GC.start
    t = cpu
    calls.times do |k|
      s = cold ? sels[k] : sel
      case meth
      when :css then doc.css(s)
      when :at_css then doc.at_css(s)
      when :matches then target.matches?(s)
      end
    end
    best = [best, (cpu - t) / calls].min
  end
  best * 1e6
end

ROWS.each do |label, sel, meth, _|
  next if meth == :matches

  h = HTML_DOC.public_send(meth, sel)
  x = XML_DOC.public_send(meth, sel)
  same = meth == :css ? h.size == x.size : h.nil? == x.nil?
  abort "#{label}: HTML and XML disagree" unless same
end

rev = `git rev-parse --short HEAD 2>/dev/null`.strip
env = { "ruby" => RUBY_DESCRIPTION, "host" => RbConfig::CONFIG["host"], "makiri" => Makiri::VERSION,
        "rev" => rev, "rounds" => ROUNDS, "lists" => LISTS }
results = {}
puts env.map { |k, v| "#{k}: #{v}" }.join("\n")
puts format("%-24s %10s %10s %10s %10s", "", "html warm", "xml warm", "html cold", "xml cold")
# Every warm row before any cold one: the HTML cache stops caching while
# its hit rate is low, and a cold row's misses must not reach a warm row.
warm = ROWS.to_h do |label, sel, meth, calls|
  [label, { "html_warm" => run(HTML_DOC, sel, meth, calls, false), "xml_warm" => run(XML_DOC, sel, meth, calls, false) }]
end
ROWS.each do |label, sel, meth, calls|
  row = warm[label].merge(
    "html_cold" => run(HTML_DOC, sel, meth, calls, true),
    "xml_cold" => run(XML_DOC, sel, meth, calls, true)
  )
  results[label] = row
  puts format("%-24s %10.2f %10.2f %10.2f %10.2f", label, *row.values_at("html_warm", "xml_warm", "html_cold", "xml_cold"))
end
puts "(us per call, min CPU time over #{ROUNDS} rounds)"

if opts[:json]
  File.write(opts[:json], JSON.pretty_generate({ "env" => env, "results" => results }))
  puts "saved #{opts[:json]}"
end
if opts[:compare]
  base = JSON.parse(File.read(opts[:compare]))
  puts "change against #{opts[:compare]} (#{base.dig("env", "rev")}), new / old:"
  results.each do |label, row|
    old = base.dig("results", label) or next
    ratios = %w[html_warm xml_warm html_cold xml_cold].map { |k| format("%.2f", row[k] / old[k]) }
    puts format("%-24s %10s %10s %10s %10s", label, *ratios)
  end
end
