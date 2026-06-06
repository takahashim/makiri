# frozen_string_literal: true
#
# XML XPath 1.0 differential conformance: evaluate the same expression against
# the same XML document in Makiri's native engine (Makiri::XML, original code,
# no libxml2) and in Nokogiri (libxml2's mature XPath 1.0), then compare. A
# divergence is either a Makiri bug or an intentional, documented difference.
#
# Each document carries the namespace prefix map its queries use; the same map
# is passed to both engines, so the inputs are identical. Result comparison and
# node identity mirror the HTML differential (spec/conformance/xpath_diff.rb):
#   * node-set -> compare the SET of node paths (prefix stripped per step, since
#     the two libraries render a path's prefixes differently); order-only diffs
#     are reported separately.
#   * scalar -> compared by value (numbers within 1e-9, NaN==NaN).
#   * Makiri's documented fail-closed cases (per-evaluate budget; the
#     unimplemented namespace axis) are tallied, not scored as bugs.
#
# Nokogiri is a bench-only dependency, so run OUTSIDE the bundle (the rake task
# does this):
#   rake conformance:xpath_xml
#   ruby -Ilib spec/conformance/xml_xpath_diff.rb --verbose

require "optparse"

$LOAD_PATH.unshift File.expand_path("../../lib", __dir__)
require "makiri"

begin
  require "nokogiri"
rescue LoadError
  abort "nokogiri is required for the XML XPath differential (run via `rake conformance:xpath_xml`)"
end

require_relative "xml_xpath_corpus"

opts = { max_diffs: 40, verbose: false }
OptionParser.new do |o|
  o.banner = "Usage: ruby spec/conformance/xml_xpath_diff.rb [options]"
  o.on("--max-diffs N", Integer, "show at most N divergences (default 40)") { |v| opts[:max_diffs] = v }
  o.on("--verbose", "show every divergence") { opts[:verbose] = true }
  o.on("-h", "--help") { puts o; exit }
end.parse!(ARGV)

DOCS = XmlXPathCorpus::DOCS.map do |d|
  { name: d[:name], ns: d[:ns], exprs: XmlXPathCorpus::STRUCTURAL + d[:exprs],
    makiri: Makiri::XML(d[:xml]), nokogiri: Nokogiri::XML(d[:xml]) }
end

# --- evaluation + classification -------------------------------------------

def run(doc_key, doc, expr, ns)
  value = ns.empty? ? doc.xpath(expr) : doc.xpath(expr, ns)
  [:ok, value]
rescue Makiri::XPath::LimitExceeded
  [:makiri_limit]
rescue Makiri::Error => e
  doc_key == :makiri && e.message.include?("not implemented") ? [:makiri_unimpl] : [:raise, "Makiri::Error"]
rescue Makiri::XPath::SyntaxError
  [:raise, "SyntaxError"]
rescue StandardError => e
  [:raise, e.class.name]
end

# Canonical node key: a NAME-FREE positional path. Node#path renders namespaced
# steps differently in the two libraries (Nokogiri emits "*[n]" for a default-
# namespace element where Makiri uses the local name, and labels a CDATA node
# "#cdata-section" vs "text()"), so we key by structural position instead - the
# 1-based-from-zero index among all preceding siblings, up to the root. The two
# DOMs are isomorphic, so corresponding nodes get the same key. Attributes key by
# their owner's path + local name.
def attribute_node?(n)
  %w[Nokogiri::XML::Attr Makiri::XML::Attr Makiri::HTML::Attr].include?(n.class.name)
end

def local_of(n)
  (n.respond_to?(:local_name) && n.local_name) || n.name.to_s.sub(/\A[\w-]+:/, "")
end

def positional_key(node)
  segs = []
  cur = node
  while cur.respond_to?(:parent) && (par = cur.parent)
    idx = 0
    sib = cur
    idx += 1 while (sib = sib.previous_sibling)
    segs.unshift(idx.to_s)
    cur = par
  end
  "/#{segs.join('/')}"
end

def node_key(node)
  return "#{positional_key(node.parent)}/@#{local_of(node)}" if attribute_node?(node)

  positional_key(node)
end

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
  doc[:exprs].uniq.each do |expr|
    m = run(:makiri, doc[:makiri], expr, doc[:ns])
    n = run(:nokogiri, doc[:nokogiri], expr, doc[:ns])
    stats[:pairs] += 1

    if m[0] == :makiri_limit then stats[:makiri_limit] += 1; next end
    if m[0] == :makiri_unimpl then stats[:makiri_unimpl] += 1; next end

    if m[0] == :raise && n[0] == :raise then stats[:agree_reject] += 1; next end
    if m[0] == :raise || n[0] == :raise
      stats[:diverge_raise] += 1
      diffs << [doc[:name], expr, "raise mismatch: makiri=#{m.inspect} nokogiri=#{n.inspect}"]
      next
    end

    case compare(shape(m[1]), shape(n[1]))
    in [:same]       then stats[:agree] += 1
    in [:order_only] then stats[:order_only] += 1
                          diffs << [doc[:name], expr, "ORDER ONLY (same set, different order)"]
    in [:diff, why]  then stats[:diverge_result] += 1
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
puts "XML XPath differential vs Nokogiri (libxml2)"
puts "  documents          : #{DOCS.size}"
puts "  pairs evaluated    : #{stats[:pairs]}"
puts "  agree (result)     : #{stats[:agree]}"
puts "  agree (both reject): #{stats[:agree_reject]}"
puts "  makiri budget-limit: #{stats[:makiri_limit]}"
puts "  makiri unimplmntd  : #{stats[:makiri_unimpl]}  (e.g. namespace axis)"
puts "  order-only diff    : #{stats[:order_only]}"
puts "  DIVERGE result     : #{stats[:diverge_result]}"
puts "  DIVERGE raise      : #{stats[:diverge_raise]}"

real = stats[:diverge_result] + stats[:diverge_raise]
exit(real.zero? ? 0 : 1)
