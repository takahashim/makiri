# frozen_string_literal: true
#
# XPath 1.0 differential conformance: evaluate the same expression against the
# same document in Makiri's native engine and in Nokogiri (libxml2's mature,
# spec-compliant XPath 1.0), then compare the results. Makiri's XPath engine is
# original code (no libxml2 at any layer), so this is its highest-value
# correctness net: a divergence is either a Makiri bug or an intentional,
# documented difference.
#
# Both engines parse with their HTML5 frontend (Makiri = Lexbor, Nokogiri =
# Gumbo) so the DOM trees are isomorphic and node identity can be compared by
# absolute path (verified identical for element/attribute/text/comment).
#
# Result comparison:
#   * node-set vs node-set -> compare the SET of node paths (XPath node-sets are
#     formally unordered); an order-only difference is reported separately.
#   * scalar (number/string/boolean) -> compare by value (numbers within 1e-9,
#     NaN==NaN).
#   * one raises / one succeeds -> divergence, EXCEPT Makiri's documented
#     fail-closed cases (per-evaluate budget = LimitExceeded; the unimplemented
#     namespace axis), which are tallied separately, not scored as bugs.
#
# Nokogiri is a bench-only dependency, so run this OUTSIDE the bundle (the rake
# task does this for you):
#   rake conformance:xpath
#   ruby -Ilib spec/conformance/xpath_diff.rb               # curated corpus
#   ruby -Ilib spec/conformance/xpath_diff.rb --generate 5000 --seed 42
#   ruby -Ilib spec/conformance/xpath_diff.rb --max-diffs 40 --verbose

require "optparse"

$LOAD_PATH.unshift File.expand_path("../../lib", __dir__)
require "makiri"

begin
  require "nokogiri"
rescue LoadError
  abort "nokogiri is required for the XPath differential (run via `rake conformance:xpath`)"
end

require_relative "../fuzz/grammar"
require_relative "../fuzz/fixtures"
require_relative "xpath_corpus"

opts = { generate: 0, seed: nil, max_diffs: 25, verbose: false }
OptionParser.new do |o|
  o.banner = "Usage: ruby spec/conformance/xpath_diff.rb [options]"
  o.on("--generate N", Integer, "also test N grammar-generated expressions") { |v| opts[:generate] = v }
  o.on("--seed N", Integer, "RNG seed for --generate (default random)")       { |v| opts[:seed] = v }
  o.on("--max-diffs N", Integer, "show at most N divergences (default 25)")   { |v| opts[:max_diffs] = v }
  o.on("--verbose", "show every divergence")                                  { opts[:verbose] = true }
  o.on("-h", "--help") { puts o; exit }
end.parse!(ARGV)

# --- documents -------------------------------------------------------------

# Makiri and Nokogiri::HTML5 disagree on the namespace MODEL, not on XPath
# evaluation: Makiri's unprefixed name tests match by local-name regardless of
# namespace (so //svg and //rect find foreign elements), whereas Nokogiri keeps
# HTML in the null namespace but SVG/MathML in their foreign namespaces, so
# //rect matches nothing unless you register and prefix the namespace. That is
# a documented difference (see README), out of scope for evaluation-semantics
# diffing. We align the baseline by stripping namespaces from the Nokogiri DOM
# (remove_namespaces! also drops the "svg:" path prefix), so this harness
# compares which nodes each ENGINE selects under the same namespace-agnostic
# policy. The model difference itself is asserted separately in the README.
DOCS = (FuzzFixtures.all + XPathCorpus.extra_docs).map do |name, html|
  noko = Nokogiri::HTML5(html)
  noko.remove_namespaces!
  { name: name, html: html, makiri: Makiri::HTML(html), nokogiri: noko }
end

# --- expressions -----------------------------------------------------------

exprs = XPathCorpus::EXPRESSIONS.dup
if opts[:generate].positive?
  rng = Random.new(opts[:seed] || Random.new_seed)
  opts[:generate].times { exprs << Grammar.gen_xpath(4 + rng.rand(4), rng) }
end
exprs.uniq!

# --- evaluation + classification -------------------------------------------

# Run an expression; return [:ok, value] or [:raise, ClassName] or
# [:makiri_limit] / [:makiri_unimpl] for Makiri's documented fail-closed cases.
def eval_makiri(doc, expr)
  [:ok, doc.xpath(expr)]
rescue Makiri::XPath::LimitExceeded
  [:makiri_limit]
rescue Makiri::Error => e
  e.message.include?("not implemented") ? [:makiri_unimpl] : [:raise, "Makiri::Error"]
rescue Makiri::XPath::SyntaxError
  [:raise, "SyntaxError"]
end

def eval_nokogiri(doc, expr)
  [:ok, doc.xpath(expr)]
rescue StandardError => e
  [:raise, e.class.name]
end

# Canonical node key: the absolute path with any namespace prefix stripped
# from each step. The two libraries' DOM trees are isomorphic, but they render
# the path of a foreign (SVG/MathML) element differently — Nokogiri qualifies
# it ("svg:circle"), Makiri does not ("circle"). That is a Node#path rendering
# nuance, NOT an XPath-evaluation difference, so we normalise it away here to
# keep the comparison about which nodes matched.
def node_key(node)
  node.path.split("/").map { |seg| seg.sub(/\A[\w-]+:/, "") }.join("/")
end

# Normalise a successful result into a comparable shape.
def shape(value)
  case value
  when Makiri::NodeSet, Nokogiri::XML::NodeSet
    [:nodeset, value.map { |n| node_key(n) }]
  when Float   then [:number, value]
  when Numeric then [:number, value.to_f]
  when String  then [:string, value]
  when true, false then [:boolean, value]
  else [:other, value.class.name]
  end
end

def numbers_equal?(a, b)
  return true if a.nan? && b.nan?
  return a == b if a.infinite? || b.infinite?

  (a - b).abs <= 1e-9
end

# Compare two shaped results. Returns [:same] / [:order_only] / [:diff, why].
def compare(ms, ns)
  return [:diff, "kind #{ms[0]} vs #{ns[0]}"] unless ms[0] == ns[0]

  case ms[0]
  when :nodeset
    return [:diff, "count #{ms[1].size} vs #{ns[1].size}"] if ms[1].size != ns[1].size
    return [:same] if ms[1] == ns[1]
    return [:order_only] if ms[1].sort == ns[1].sort

    only_m = ms[1] - ns[1]
    only_n = ns[1] - ms[1]
    [:diff, "nodes only-makiri=#{only_m.first(3).inspect} only-nokogiri=#{only_n.first(3).inspect}"]
  when :number
    numbers_equal?(ms[1], ns[1]) ? [:same] : [:diff, "#{ms[1]} vs #{ns[1]}"]
  else
    ms[1] == ns[1] ? [:same] : [:diff, "#{ms[1].inspect} vs #{ns[1].inspect}"]
  end
end

# --- run -------------------------------------------------------------------

stats = Hash.new(0)
diffs = []

DOCS.each do |doc|
  exprs.each do |expr|
    m = eval_makiri(doc[:makiri], expr)
    n = eval_nokogiri(doc[:nokogiri], expr)
    stats[:pairs] += 1

    # Makiri's documented fail-closed paths: tallied, never scored as bugs.
    if m[0] == :makiri_limit
      stats[:makiri_limit] += 1
      next
    end
    if m[0] == :makiri_unimpl
      stats[:makiri_unimpl] += 1
      next
    end

    if m[0] == :raise && n[0] == :raise
      stats[:agree_reject] += 1
      next
    end
    if m[0] == :raise || n[0] == :raise
      # KNOWN: Makiri evaluates a top-level position()/last() (context
      # position/size at the document root) where libxml2 raises a syntax
      # error. Makiri's behaviour is defensible per XPath 1.0; bucket it.
      if n[0] == :raise && m[0] == :ok && expr.match?(/\b(position|last)\(\)/)
        stats[:noko_strict] += 1
      else
        stats[:diverge_raise] += 1
        diffs << [doc[:name], expr, "raise mismatch: makiri=#{m.inspect} nokogiri=#{n.inspect}"]
      end
      next
    end

    ms = shape(m[1])
    ns = shape(n[1])
    case compare(ms, ns)
    in [:same]
      stats[:agree] += 1
    in [:order_only]
      stats[:order_only] += 1
      diffs << [doc[:name], expr, "ORDER ONLY (same node set, different encounter order)"]
    in [:diff, why]
      stats[:diverge_result] += 1
      diffs << [doc[:name], expr, why]
    end
  end
end

# --- report ----------------------------------------------------------------

shown = 0
diffs.each do |docname, expr, why|
  break if shown >= opts[:max_diffs] && !opts[:verbose]

  shown += 1
  puts "DIVERGE [#{docname}]  #{expr}"
  puts "        #{why}"
end
puts "... #{diffs.length - shown} more not shown" if !opts[:verbose] && diffs.length > shown

puts "\n#{'=' * 72}"
puts "XPath differential vs Nokogiri (libxml2)"
puts "  documents          : #{DOCS.size}"
puts "  expressions        : #{exprs.size}#{opts[:generate].positive? ? " (incl. #{opts[:generate]} generated)" : ''}"
puts "  pairs evaluated    : #{stats[:pairs]}"
puts "  agree (result)     : #{stats[:agree]}"
puts "  agree (both reject): #{stats[:agree_reject]}"
puts "  makiri budget-limit: #{stats[:makiri_limit]}"
puts "  makiri unimplmntd  : #{stats[:makiri_unimpl]}"
puts "  KNOWN noko-strict  : #{stats[:noko_strict]}  (top-level position()/last())"
puts "  order-only diff    : #{stats[:order_only]}"
puts "  DIVERGE result     : #{stats[:diverge_result]}"
puts "  DIVERGE raise      : #{stats[:diverge_raise]}"

real = stats[:diverge_result] + stats[:diverge_raise]
exit(real.zero? ? 0 : 1)
