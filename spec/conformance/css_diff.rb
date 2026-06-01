# frozen_string_literal: true
#
# CSS Selectors differential conformance: run the same selector against the
# same document in Makiri (Lexbor's selector engine) and in Nokogiri::HTML5
# (which compiles CSS to XPath and runs libxml2), then compare the matched
# node sets.
#
# Unlike the XPath differential, the *matching* engine here is Lexbor's
# (mature, upstream-tested) rather than original Makiri code, so this primarily
# exercises (a) Makiri's glue — descendant-only scope, document order,
# comma-list de-duplication, error mapping — and (b) where the two engines'
# SUPPORTED-SELECTOR vocabularies differ. Those vocabulary differences are
# tallied, not scored as bugs:
#   * lexbor-only  : Makiri matches, Nokogiri raises (e.g. Level-4 :is/:where).
#   * nokogiri-only: Nokogiri matches, Makiri raises (jQuery extensions such as
#                    :contains, :gt, :lt, :eq, :first — non-standard; Makiri,
#                    being standards-only, deliberately does not support them).
#   * agree-reject : both raise (pseudo-elements, genuinely invalid selectors).
# A real divergence is both engines succeeding with a DIFFERENT node set.
#
# Nokogiri is a bench-only dependency, so run this OUTSIDE the bundle (the rake
# task does this for you):
#   rake conformance:css
#   ruby -Ilib spec/conformance/css_diff.rb
#   ruby -Ilib spec/conformance/css_diff.rb --max-diffs 40 --verbose

require "optparse"

$LOAD_PATH.unshift File.expand_path("../../lib", __dir__)
require "makiri"

begin
  require "nokogiri"
rescue LoadError
  abort "nokogiri is required for the CSS differential (run via `rake conformance:css`)"
end

require_relative "../fuzz/fixtures"
require_relative "css_corpus"

opts = { max_diffs: 25, verbose: false }
OptionParser.new do |o|
  o.banner = "Usage: ruby spec/conformance/css_diff.rb [options]"
  o.on("--max-diffs N", Integer, "show at most N divergences (default 25)") { |v| opts[:max_diffs] = v }
  o.on("--verbose", "show every divergence") { opts[:verbose] = true }
  o.on("-h", "--help") { puts o; exit }
end.parse!(ARGV)

# All fixtures get an explicit doctype: CSS class/id case-sensitivity depends on
# the document's quirks mode, and we want the standard (no-quirks) behaviour.
DOCS = (FuzzFixtures.all + CSSCorpus.extra_docs).map do |name, html|
  html = "<!DOCTYPE html>#{html}" unless html.lstrip.downcase.start_with?("<!doctype")
  { name: name, makiri: Makiri::HTML(html), nokogiri: Nokogiri::HTML5(html) }
end

# Canonical node key: absolute path with any namespace prefix stripped per step
# (Makiri renders foreign element paths without the "svg:" prefix Nokogiri uses).
def node_key(node)
  node.path.split("/").map { |seg| seg.sub(/\A[\w-]+:/, "") }.join("/")
end

def run(node, sel)
  [:ok, node.css(sel).map { |n| node_key(n) }.sort]
rescue Makiri::CSS::SyntaxError, Nokogiri::CSS::SyntaxError, Nokogiri::XML::XPath::SyntaxError
  [:raise]
rescue StandardError => e
  [:raise, e.class.name]
end

stats = Hash.new(0)
diffs = []

DOCS.each do |doc|
  CSSCorpus::SELECTORS.each do |sel|
    m = run(doc[:makiri], sel)
    n = run(doc[:nokogiri], sel)
    stats[:pairs] += 1

    if m[0] == :raise && n[0] == :raise
      stats[:agree_reject] += 1
    elsif m[0] == :ok && n[0] == :raise
      stats[:lexbor_only] += 1
    elsif m[0] == :raise && n[0] == :ok
      stats[:nokogiri_only] += 1
    elsif m[1] == n[1]
      stats[:agree] += 1
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
puts "CSS Selectors differential (Makiri/Lexbor vs Nokogiri::HTML5/libxml2)"
puts "  documents          : #{DOCS.size}"
puts "  selectors          : #{CSSCorpus::SELECTORS.size}"
puts "  pairs evaluated    : #{stats[:pairs]}"
puts "  agree (node set)   : #{stats[:agree]}"
puts "  agree (both reject): #{stats[:agree_reject]}"
puts "  lexbor-only        : #{stats[:lexbor_only]}  (selectors Lexbor supports, Nokogiri rejects)"
puts "  nokogiri-only      : #{stats[:nokogiri_only]}  (jQuery extensions Makiri rejects by design)"
puts "  DIVERGE node set   : #{stats[:diverge]}"

exit(stats[:diverge].zero? ? 0 : 1)
