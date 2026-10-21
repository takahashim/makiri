# frozen_string_literal: true
#
# Browsers as evidence for the CSS selector engines: run the same queries in
# Makiri (HTML and XML) and in Chromium, Firefox and WebKit (Playwright,
# DOMParser documents) and report where the answers differ.
#
# Only queries the three browsers answer alike are compared; a query they
# split on is reported as such and set aside. A difference is either listed in
# KNOWN - with the tag that names why (an item in
# docs/parser_differences.ja.md, or a decision still open in the CSS
# unification plan) - or reported as new. The run fails on a new difference
# and on a KNOWN entry that no longer differs, so the list stays the inventory
# of what Makiri answers differently, not a pile of stale exemptions.
#
# Like browser_diff.rb this is evidence, not an arbiter: the specifications
# decide, and a difference is classified by hand.
#
# Needs Node.js and `npm install` in spec/conformance/browser.
#
# Usage:
#   ruby -Ilib spec/conformance/css_browser_diff.rb             # compare, fail on new
#   ruby -Ilib spec/conformance/css_browser_diff.rb --verbose   # print every difference
#   ruby -Ilib spec/conformance/css_browser_diff.rb --browsers chromium

require "json"
require "open3"
require "optparse"

$LOAD_PATH.unshift File.expand_path("../../lib", __dir__)
require "makiri"
require_relative "css_browser_cases"

opts = { browsers: "chromium,firefox,webkit", verbose: false }
OptionParser.new do |o|
  o.on("--browsers LIST", "comma-separated (default chromium,firefox,webkit)") { |v| opts[:browsers] = v }
  o.on("--verbose", "print every difference, known ones included") { opts[:verbose] = true }
end.parse!(ARGV)

# Why a query differs, as rules: each names a tag, the documents and
# selectors it covers, and where (:all / :matches for the whole-document
# modes, or a context selector; nil for every place the document runs the
# selector). The tags are items of docs/parser_differences.ja.md or of
# notes' CSS unification plan; KNOWN expands them to one key per query, and
# a key that stops differing is reported as stale.
HTML_DOCS = %w[html-basic html-form html-foreign].freeze
XML_DOCS = %w[xml-plain xml-ns xml-xhtml].freeze
ALL_DOCS = HTML_DOCS + XML_DOCS
UNIMPLEMENTED = [":visited", ":target", ":default", ":indeterminate", ":lang(en)", ":lang(fr)",
                 ":dir(ltr)", ":scope", ":scope > *", ":defined"].freeze
RULES = [
  # Pseudo-classes the matcher does not implement, and pseudo-elements, which
  # a browser accepts and never matches.
  ["unimplemented", ALL_DOCS, UNIMPLEMENTED],
  ["unimplemented", %w[html-basic], [":scope > p"], "#d"],
  ["unimplemented", %w[xml-plain], [":scope > p"], "div"],
  ["pseudo-element", ALL_DOCS, ["::before", "p::before"]],
  ["C27", ALL_DOCS, ["[*|href]"]],
  # :link takes <link> (M2), and :read-write is Lexbor's (M9).
  ["M2", %w[html-form], [":link"]],
  ["M9", %w[html-form xml-xhtml], [":read-write"]],
  # :empty counts an empty CDATA section and a processing instruction (M10).
  ["M10", %w[xml-plain], [":empty", ":not(:empty)"]]
].freeze
KNOWN = RULES.each_with_object({}) do |(tag, docs, sels, where), h|
  places = where ? [where] : %i[all matches]
  docs.product(sels, places).each do |doc, sel, place|
    raise "two rules for #{[doc, sel, place].inspect}" if h.key?([doc, sel, place])

    h[[doc, sel, place]] = tag
  end
end.freeze

Query = Struct.new(:doc, :sel, :ctx, :mode, keyword_init: true) do
  def key = [doc, sel, ctx || mode]
  def label = ctx ? "#{sel.inspect} in #{ctx}" : "#{sel.inspect} (#{mode})"
end

def queries_for(doc)
  whole = (CssBrowserCases::SELECTORS + doc.fetch(:selectors, [])).flat_map do |sel|
    %i[all matches].map { |mode| Query.new(doc: doc[:name], sel: sel, ctx: nil, mode: mode) }
  end
  scoped = doc.fetch(:contexts, {}).flat_map do |ctx, sels|
    sels.map { |sel| Query.new(doc: doc[:name], sel: sel, ctx: ctx, mode: :all) }
  end
  whole + scoped
end

def path(node)
  out = []
  while node.element?
    i = 0
    s = node
    i += 1 while (s = s.previous_element)
    out.unshift(i)
    node = node.parent
  end
  out
end

def makiri_answer(dom, q)
  if q.mode == :matches
    dom.xpath("//*").select { |e| e.matches?(q.sel) }.map { |e| path(e) }
  else
    root = q.ctx ? dom.at_css(q.ctx) : dom
    return { "error" => "NoContext" } unless root

    root.css(q.sel).map { |e| path(e) }
  end
rescue Makiri::CSS::SyntaxError
  { "error" => "SyntaxError" }
rescue Makiri::Error => e
  { "error" => e.class.name }
end

def show(answer)
  answer.is_a?(Hash) ? answer["error"] : "#{answer.size} #{answer.first(4).inspect}#{answer.size > 4 ? "..." : ""}"
end

docs = CssBrowserCases::DOCUMENTS
plan = docs.map { |d| queries_for(d) }
input = docs.zip(plan).map do |d, qs|
  { mime: d[:mime], src: d[:src], queries: qs.map { |q| { sel: q.sel, ctx: q.ctx, mode: q.mode } } }
end

dir = File.expand_path("browser", __dir__)
out, err, st = Open3.capture3("node", File.join(dir, "css_query.mjs"), opts[:browsers],
                              stdin_data: JSON.generate(input), chdir: dir)
abort "css_query.mjs failed:\n#{err}" unless st.success?
browsers = JSON.parse(out)
names = opts[:browsers].split(",")
puts names.map { |n| "#{n} #{browsers["#{n}_version"]}" }.join(", ")

counts = Hash.new(0)
new_diffs = []
known_seen = {}
docs.each_with_index do |d, di|
  dom = d[:mime] == "text/html" ? Makiri::HTML(d[:src]) : Makiri::XML(d[:src])
  plan[di].each_with_index do |q, qi|
    answers = names.map { |n| browsers[n][di][qi] }
    if answers.uniq.size > 1
      counts[:split] += 1
      known_seen[q.key] = :split
      puts "  browsers split: #{q.doc} #{q.label}: #{names.zip(answers).map { |n, a| "#{n}=#{show(a)}" }.join(" ")}" if opts[:verbose]
      next
    end
    expected = answers.first
    actual = makiri_answer(dom, q)
    tag = KNOWN[q.key]
    if actual == expected
      counts[:agree] += 1
      known_seen[q.key] = :agrees if tag
      next
    end
    if tag
      counts[:known] += 1
      known_seen[q.key] = :differs
      puts "  known (#{tag}): #{q.doc} #{q.label}: browsers=#{show(expected)} makiri=#{show(actual)}" if opts[:verbose]
    else
      counts[:new] += 1
      new_diffs << "#{q.doc} #{q.label}: browsers=#{show(expected)} makiri=#{show(actual)}"
    end
  end
end

stale = KNOWN.keys.select { |k| known_seen[k] == :agrees }
# A rule for a query the cases never run is a typo, not an exemption.
unrun = KNOWN.keys.reject { |k| known_seen.key?(k) || plan.flatten.any? { |q| q.key == k } }
puts "agree #{counts[:agree]}, known #{counts[:known]}, new #{counts[:new]}, browsers split #{counts[:split]}"
KNOWN.select { |k, _| known_seen[k] == :differs }.values.tally.sort.each { |tag, n| puts "  #{tag}: #{n}" }
unless new_diffs.empty?
  puts "NEW differences:"
  new_diffs.each { |l| puts "  #{l}" }
end
unless stale.empty?
  puts "KNOWN entries that no longer differ (remove them):"
  stale.each { |k| puts "  #{k.inspect} (#{KNOWN[k]})" }
end
unless unrun.empty?
  puts "KNOWN entries for queries the cases do not run:"
  unrun.each { |k| puts "  #{k.inspect} (#{KNOWN[k]})" }
end
exit(new_diffs.empty? && stale.empty? && unrun.empty? ? 0 : 1)
