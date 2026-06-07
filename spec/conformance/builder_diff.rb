# frozen_string_literal: true
#
# XML Builder differential conformance: run the SAME builder program through
# Makiri::XML::Builder and Nokogiri::XML::Builder, then compare the trees they
# build. A divergence is either a Makiri bug or an intentional, documented
# difference.
#
# Nokogiri is built with `namespace_inheritance: false` so the two share the same
# namespace model: a child does NOT inherit a prefixed ancestor's namespace
# (Makiri has no prefix-inheritance; an unprefixed child binds only to the
# in-scope DEFAULT xmlns, which both do). The corpus uses the block-with-argument
# and instance_eval forms, attribute/text/namespace arguments, `xml["prefix"]`
# prefixed elements, the `tag.class.id!` attribute short-cuts, raw-XML `<<`, and
# nested blocks - the surface the two builders are meant to share.
#
# The trees are compared by a canonical structural dump keyed on each node's
# (LOCAL name, namespace URI) plus its non-xmlns attributes (also by local name +
# URI) and content - prefix spellings and which ancestor declared a namespace do
# not matter, only the effective namespaces do. (Makiri keeps xmlns declarations
# as DOM attributes while Nokogiri does not, so the dump filters them out and
# compares the resolved namespaces instead.)
#
# Nokogiri is a bench-only dependency, so run OUTSIDE the bundle (the rake task
# does this):
#   rake conformance:builder
#   ruby -Ilib spec/conformance/builder_diff.rb --verbose

require "optparse"

$LOAD_PATH.unshift File.expand_path("../../lib", __dir__)
require "makiri"

begin
  require "nokogiri"
rescue LoadError
  abort "nokogiri is required for the Builder differential (run via `rake conformance:builder`)"
end

opts = { verbose: false }
OptionParser.new do |o|
  o.banner = "Usage: ruby spec/conformance/builder_diff.rb [options]"
  o.on("--verbose", "show every divergence in full") { opts[:verbose] = true }
  o.on("-h", "--help") { puts o; exit }
end.parse!(ARGV)

# --- the corpus: one builder program per entry --------------------------------
# Each :build is a proc run as the builder block. An arity-1 proc drives the
# block-with-argument form (xml.foo); an arity-0 proc the instance_eval form.
CORPUS = [
  { name: "nesting + attrs + text",
    build: ->(xml) { xml.root(id: "1") { xml.a("hello", href: "/x"); xml.b { xml.c } } } },
  { name: "default namespace inherited by children",
    build: ->(xml) { xml.feed("xmlns" => "urn:a") { xml.entry { xml.title("Hi") } } } },
  { name: "prefixed element via []",
    build: ->(xml) { xml.feed("xmlns:dc" => "urn:dc") { xml.entry { xml["dc"].id_("42"); xml.title("T") } } } },
  { name: "namespaced attribute",
    build: ->(xml) { xml.feed("xmlns:dc" => "urn:dc") { xml.entry("dc:id" => "9") } } },
  { name: "text / cdata / comment helpers",
    build: ->(xml) { xml.root { xml.text "plain "; xml.cdata "a < b"; xml.comment " note " } } },
  { name: "attribute short-cuts (object.classy.thing!)",
    build: ->(xml) { xml.root { xml.object.classy.thing! } } },
  { name: "short-cut with content + nested block",
    build: ->(xml) { xml.root { xml.item.flagged("body") { xml.child } } } },
  { name: "trailing-underscore tags",
    build: ->(xml) { xml.root { xml.id_("10"); xml.type_("t") } } },
  { name: "symbol-key attributes",
    build: ->(xml) { xml.root { xml.a(href: "/x", rel: "next") } } },
  { name: "deep nesting",
    build: ->(xml) { xml.a { xml.b { xml.c { xml.d("x") } } } } },
  { name: "mixed content",
    build: ->(xml) { xml.p { xml.text "a"; xml.b("bold"); xml.text "c" } } },
  { name: "self-declaring prefixed root",
    build: ->(xml) { xml["foo"].root("xmlns:foo" => "bar") { xml["foo"].child } } },
  { name: "raw XML <<",
    build: ->(xml) { xml.root { xml << "<child a='1'>t</child><other/>" } } },
  { name: "instance_eval form",
    # A proc (like a real `do ... end` block), NOT a lambda: instance_eval passes
    # the receiver as an arg, which a lenient proc ignores and a lambda rejects.
    build: proc { root { child("x") } } },
].freeze

# Canonical structural dump, by (local name, namespace URI) + non-xmlns
# attributes + content. Duck-typed over the common Makiri / Nokogiri node API.
def dump(node)
  if node.element?
    local = node.name.sub(/\A.*:/, "")
    ns    = node.namespace&.href.to_s
    attrs = node.attribute_nodes
                .reject { |a| a.name == "xmlns" || a.name.start_with?("xmlns:") }
                .map { |a| "#{a.name.sub(/\A.*:/, '')}|#{a.namespace&.href}=#{a.value}" }
                .sort
    "E[#{local}|#{ns}]{#{attrs.join(',')}}(#{node.children.map { |c| dump(c) }.join})"
  elsif node.cdata?
    "CD[#{node.content}]"
  elsif node.text?
    "TX[#{node.content}]"
  elsif node.comment?
    "CM[#{node.content}]"
  elsif node.processing_instruction?
    "PI[#{node.name}|#{node.content}]"
  else
    "??[#{node.class}]"
  end
end

def build_makiri(prog)
  [:ok, dump(Makiri::XML::Builder.new(&prog).doc.root)]
rescue StandardError => e
  [:raise, e.class.name]
end

def build_nokogiri(prog)
  [:ok, dump(Nokogiri::XML::Builder.new(namespace_inheritance: false, &prog).doc.root)]
rescue StandardError => e
  [:raise, e.class.name]
end

stats = Hash.new(0)
diffs = []

CORPUS.each do |entry|
  m = build_makiri(entry[:build])
  n = build_nokogiri(entry[:build])
  stats[:pairs] += 1

  if m[0] == :raise && n[0] == :raise
    stats[:agree_reject] += 1
  elsif m[0] == :raise
    stats[:makiri_raise] += 1
    diffs << [entry[:name], "Makiri raised #{m[1]}; Nokogiri built"]
  elsif n[0] == :raise
    stats[:nokogiri_raise] += 1
    diffs << [entry[:name], "Nokogiri raised #{n[1]}; Makiri built"]
  elsif m[1] == n[1]
    stats[:agree] += 1
  else
    stats[:diverge] += 1
    diffs << [entry[:name], "makiri=#{m[1]}\n        nokogiri=#{n[1]}"]
  end
end

diffs.each { |name, why| puts "DIVERGE [#{name}]\n        #{why}" }

puts "\n#{'=' * 72}"
puts "XML Builder differential (Makiri::XML::Builder vs Nokogiri::XML::Builder)"
puts "  programs evaluated   : #{stats[:pairs]}"
puts "  agree (tree)         : #{stats[:agree]}"
puts "  agree (both raise)   : #{stats[:agree_reject]}"
puts "  makiri-only raise    : #{stats[:makiri_raise]}"
puts "  nokogiri-only raise  : #{stats[:nokogiri_raise]}"
puts "  DIVERGE tree         : #{stats[:diverge]}"

exit((stats[:diverge] + stats[:makiri_raise] + stats[:nokogiri_raise]).zero? ? 0 : 1)
