# frozen_string_literal: true
#
# Differential property-based testing: generate well-formed XML documents (by
# construction, with an explicit model) and require that Makiri's native parser
# and Nokogiri (libxml2, strict mode) produce the SAME tree — same accept/reject
# and same canonical dump. A divergence is shrunk to a minimal counterexample; it
# is either a Makiri bug or a documented modelling difference.
#
# Generation is by construction valid, so this explores the *valid* input space
# (semantics / tree modelling) — complementing the W3C xmlconf suite (the
# well-formedness boundary) and the fixed XPath corpus. Nokogiri is a bench-only
# dependency, so run OUTSIDE the bundle — the rake task does this:
#   rake conformance:xml_pbt
#   ruby -Ilib spec/conformance/xml_pbt_diff.rb --count 20000 --verbose

require "optparse"

$LOAD_PATH.unshift File.expand_path("../../lib", __dir__)
require "makiri"

begin
  require "nokogiri"
rescue LoadError
  abort "nokogiri is required for the XML PBT differential (run via `rake conformance:xml_pbt`)"
end

require_relative "xml_pbt"

opts = { count: 5000, seed: 0, max_depth: 5, max_children: 4, max_diffs: 8, verbose: false }
OptionParser.new do |o|
  o.banner = "Usage: ruby spec/conformance/xml_pbt_diff.rb [options]"
  o.on("--count N", Integer, "documents to generate (default 5000)") { |v| opts[:count] = v }
  o.on("--seed N", Integer, "base seed (default 0)") { |v| opts[:seed] = v }
  o.on("--max-depth N", Integer, "max element nesting (default 5)") { |v| opts[:max_depth] = v }
  o.on("--max-diffs N", Integer, "show at most N divergences (default 8)") { |v| opts[:max_diffs] = v }
  o.on("--verbose", "show every divergence") { opts[:verbose] = true }
  o.on("-h", "--help") { puts o; exit }
end.parse!(ARGV)

# Nokogiri in STRICT mode (no error recovery, no network) — so it accepts/rejects
# like a conforming parser, comparable to Makiri.
def parse_noko(xml)
  Nokogiri::XML(xml) { |c| c.strict.nonet }
end

# The dumps of a document under each parser, or :reject / :error sentinels.
def dumps(xml)
  m = begin
    XmlPbt.dump_parsed(Makiri::XML(xml), XmlPbt::Makiri)
  rescue Makiri::XML::SyntaxError
    :reject
  end
  n = begin
    nd = parse_noko(xml)
    nd.errors.empty? ? XmlPbt.dump_parsed(nd, XmlPbt::Nokogiri) : :reject
  rescue Nokogiri::XML::SyntaxError
    :reject
  end
  [m, n]
end

def diverges?(doc)
  m, n = dumps(XmlPbt.serialize(doc))
  m != n
end

diverge = []
opts[:count].times do |i|
  doc = XmlPbt.gen_document(Random.new(opts[:seed] + i),
                            max_depth: opts[:max_depth], max_children: opts[:max_children])
  next unless diverges?(doc)

  diverge << XmlPbt.shrink(doc) { |d| diverges?(d) }
end

shown = 0
diverge.each do |doc|
  break if shown >= opts[:max_diffs] && !opts[:verbose]

  shown += 1
  xml = XmlPbt.serialize(doc)
  m, n = dumps(xml)
  puts "\n#{'=' * 72}"
  puts "DIVERGE (#{xml.bytesize} bytes)"
  puts "  xml:      #{xml}"
  puts "  makiri:   #{m}"
  puts "  nokogiri: #{n}"
end
puts "\n... #{diverge.length - shown} more not shown (--verbose)" if !opts[:verbose] && diverge.length > shown

puts "\n#{'=' * 72}"
puts "XML PBT differential vs Nokogiri (libxml2, strict)"
puts "  documents generated : #{opts[:count]}  (seed #{opts[:seed]}, depth #{opts[:max_depth]})"
puts "  diverged            : #{diverge.length}"
puts "  agree               : #{opts[:count] - diverge.length}"
exit(diverge.empty? ? 0 : 1)
