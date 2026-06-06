# frozen_string_literal: true
#
# XML CSS-selector differential conformance: run the same selector against the
# same XML document in Makiri (CSS lowered to its native XPath engine - original
# code, no libxml2) and in Nokogiri::XML (which compiles CSS to XPath and runs
# libxml2), then compare the matched node sets. A divergence is either a Makiri
# bug or an intentional, documented difference.
#
# Both engines bind a bare type selector to the document's default namespace and
# take a {prefix => uri} map for `ns|el`; the same map is passed to both, so the
# inputs are identical. Vocabulary differences are tallied, NOT scored as bugs:
#   * makiri-unsupported : Makiri raises (the [a=v i] case flag, `*|attr`,
#                          untyped of-type, jQuery extensions); standards-only /
#                          XPath-1.0 limits, documented in the README.
#   * nokogiri-unsupported: Nokogiri raises where Makiri matches.
#   * agree-reject       : both raise (pseudo-elements, genuinely invalid).
# A real divergence is both engines succeeding with a DIFFERENT node set.
#
# Node identity uses the same namespace-free positional key as the XPath
# differential, so the two libraries' differing path rendering does not matter.
#
# Nokogiri is a bench-only dependency, so run OUTSIDE the bundle (the rake task
# does this):
#   rake conformance:css_xml
#   ruby -Ilib spec/conformance/xml_css_diff.rb --verbose

require "optparse"

$LOAD_PATH.unshift File.expand_path("../../lib", __dir__)
require "makiri"

begin
  require "nokogiri"
rescue LoadError
  abort "nokogiri is required for the XML CSS differential (run via `rake conformance:css_xml`)"
end

require_relative "xml_css_corpus"

opts = { max_diffs: 40, verbose: false }
OptionParser.new do |o|
  o.banner = "Usage: ruby spec/conformance/xml_css_diff.rb [options]"
  o.on("--max-diffs N", Integer, "show at most N divergences (default 40)") { |v| opts[:max_diffs] = v }
  o.on("--verbose", "show every divergence") { opts[:verbose] = true }
  o.on("-h", "--help") { puts o; exit }
end.parse!(ARGV)

DOCS = XmlCssCorpus::DOCS.map do |d|
  { name: d[:name], ns: d[:ns],
    makiri: Makiri::XML(d[:xml]), nokogiri: Nokogiri::XML(d[:xml]) }
end

# Namespace-free positional key (identical to the XPath differential): the
# 1-based-from-zero sibling index up to the root. The two DOMs are isomorphic, so
# corresponding nodes get the same key regardless of each library's path syntax.
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

def run(doc, sel, ns)
  set = (ns.empty? ? doc.css(sel) : doc.css(sel, ns))
  [:ok, set.map { |n| positional_key(n) }.sort]
rescue Makiri::CSS::SyntaxError, Nokogiri::CSS::SyntaxError, Nokogiri::XML::XPath::SyntaxError
  [:reject]
rescue Makiri::Error => e
  # An unbound prefix etc. is a clean rejection on the Makiri side.
  [:reject, e.class.name]
rescue StandardError => e
  [:raise, e.class.name]
end

stats = Hash.new(0)
diffs = []

DOCS.each do |doc|
  XmlCssCorpus.selectors_for(doc[:name]).uniq.each do |sel|
    m = run(doc[:makiri], sel, doc[:ns])
    n = run(doc[:nokogiri], sel, doc[:ns])
    stats[:pairs] += 1

    m_ok = m[0] == :ok
    n_ok = n[0] == :ok

    if !m_ok && !n_ok
      stats[:agree_reject] += 1
    elsif !m_ok && n_ok
      stats[:makiri_unsupported] += 1
    elsif m_ok && !n_ok
      stats[:nokogiri_unsupported] += 1
    elsif m[1] == n[1]
      stats[:agree] += 1
    elsif m[1].sort == n[1].sort
      stats[:order_only] += 1
      diffs << [doc[:name], sel, "ORDER ONLY (same set, different order)"]
    else
      stats[:diverge] += 1
      diffs << [doc[:name], sel,
                "only-makiri=#{(m[1] - n[1]).first(3).inspect} " \
                "only-nokogiri=#{(n[1] - m[1]).first(3).inspect}"]
    end
  end
end

shown = 0
diffs.each do |name, sel, why|
  break if shown >= opts[:max_diffs] && !opts[:verbose]

  shown += 1
  puts "DIVERGE [#{name}]  #{sel}\n        #{why}"
end
puts "... #{diffs.length - shown} more not shown" if !opts[:verbose] && diffs.length > shown

puts "\n#{'=' * 72}"
puts "XML CSS selector differential (Makiri native engine vs Nokogiri::XML/libxml2)"
puts "  documents            : #{DOCS.size}"
puts "  pairs evaluated      : #{stats[:pairs]}"
puts "  agree (node set)     : #{stats[:agree]}"
puts "  agree (both reject)  : #{stats[:agree_reject]}"
puts "  makiri-unsupported   : #{stats[:makiri_unsupported]}  ([a=v i] / *|attr / of-type / jQuery)"
puts "  nokogiri-unsupported : #{stats[:nokogiri_unsupported]}"
puts "  order-only diff      : #{stats[:order_only]}"
puts "  DIVERGE node set     : #{stats[:diverge]}"

exit((stats[:diverge] + stats[:order_only]).zero? ? 0 : 1)
