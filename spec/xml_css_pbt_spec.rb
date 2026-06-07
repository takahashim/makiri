# frozen_string_literal: true

require "spec_helper"
require_relative "conformance/xml_pbt"

# Property-based tests for Makiri::XML CSS selectors - metamorphic / self-
# consistent, so there is no oracle to inherit a reference engine's quirks (the
# Nokogiri differential lives in spec/conformance/xml_css_diff.rb). Random
# well-formed documents come from the XmlPbt generator (namespaces included);
# the properties exercise the element-name index, the first-match short-circuit,
# #matches?, and the comma-list union law.
#
# Selectors stay namespace-unambiguous: a BARE type selector (bound to the root
# default namespace, like the engine does) and an explicit `x|local` + {x=>uri}.
# The `|local` no-namespace syntax is avoided here because Lexbor's parser does
# not distinguish it from `*|local` (see spec/xml_css_spec.rb for that pinned
# limitation), so it is not a sound oracle.
#
# CSS_PBT_COUNT controls how many random documents each property runs (seeds are
# fixed so a failure reproduces).
RSpec.describe "Makiri::XML CSS property-based testing" do
  CSS_PBT_COUNT = Integer(ENV.fetch("CSS_PBT_COUNT", "300"))

  # All element nodes of a parsed Makiri tree, document order (independent of the
  # engine's query paths - this is the ground truth).
  def all_elements(node, out = [])
    node.children.each do |c|
      next unless c.respond_to?(:node_type) && c.node_type == 1

      out << c
      all_elements(c, out)
    end
    out
  end

  def gen_doc(seed)
    Makiri::XML(XmlPbt.serialize(XmlPbt.gen_document(Random.new(seed), max_depth: 5, max_children: 4)))
  end

  # The URI a BARE type selector binds to: the root element's default-namespace
  # declaration, mirroring the engine's _css_namespaces. An xmlns="" declaration
  # (empty URI) IS the no-namespace, which elements report as a nil namespace_uri,
  # so normalise the empty string to nil to match.
  def root_default_ns(doc)
    href = doc.root&.namespace_definitions&.find { |n| n.prefix.nil? }&.href
    (href.nil? || href.empty?) ? nil : href
  end

  it "bare + prefixed name tests equal a ground-truth walk" do
    bad = nil
    CSS_PBT_COUNT.times do |i|
      doc = gen_doc(i)
      els = all_elements(doc)
      droot = root_default_ns(doc)

      bare_ok = els.map(&:local_name).uniq.all? do |local|
        truth = els.select { |e| e.local_name == local && e.namespace_uri == droot }
        doc.css(local).to_a == truth
      end
      px_ok = els.map { |e| [e.local_name, e.namespace_uri] }.uniq.reject { |_, u| u.nil? }.all? do |local, uri|
        truth = els.select { |e| e.local_name == local && e.namespace_uri == uri }
        doc.css("x|#{local}", "x" => uri).to_a == truth
      end
      next if bare_ok && px_ok

      bad = doc.to_xml
      break
    end
    expect(bad).to be_nil, -> { "css name test != walk:\n#{bad}" }
  end

  # Every distinct selector to drive the consistency properties below.
  def selectors_for(doc)
    els = all_elements(doc)
    sels = els.map(&:local_name).uniq.map { |l| [l, {}] }
    els.map { |e| [e.local_name, e.namespace_uri] }.uniq.reject { |_, u| u.nil? }.each do |l, u|
      sels << ["x|#{l}", { "x" => u }]
    end
    sels
  end

  it "#at_css returns css(sel).first for every selector" do
    bad = nil
    CSS_PBT_COUNT.times do |i|
      doc = gen_doc(i + 1_000)
      ok = selectors_for(doc).all? do |sel, ns|
        first = ns.empty? ? doc.at_css(sel) : doc.at_css(sel, ns)
        full  = ns.empty? ? doc.css(sel) : doc.css(sel, ns)
        first == full.first
      end
      next if ok

      bad = doc.to_xml
      break
    end
    expect(bad).to be_nil, -> { "at_css != css.first:\n#{bad}" }
  end

  it "#matches? agrees with membership in the document's result set" do
    bad = nil
    CSS_PBT_COUNT.times do |i|
      doc = gen_doc(i + 2_000)
      els = all_elements(doc)
      ok = selectors_for(doc).all? do |sel, ns|
        selected = (ns.empty? ? doc.css(sel) : doc.css(sel, ns)).to_a
        els.all? do |e|
          m = ns.empty? ? e.matches?(sel) : e.matches?(sel, ns)
          m == selected.include?(e)
        end
      end
      next if ok

      bad = doc.to_xml
      break
    end
    expect(bad).to be_nil, -> { "matches? disagrees with css membership:\n#{bad}" }
  end

  it "comma union equals the document-ordered, de-duplicated merge of its parts" do
    bad = nil
    CSS_PBT_COUNT.times do |i|
      doc = gen_doc(i + 3_000)
      els = all_elements(doc)
      names = els.map(&:local_name).uniq
      next if names.length < 2

      a, b = names.first(2)
      union   = doc.css("#{a}, #{b}").to_a
      a_nodes = doc.css(a).to_a
      b_nodes = doc.css(b).to_a
      expected = els.select { |e| (a_nodes + b_nodes).include?(e) } # impose document order + dedup
      next if union == expected

      bad = doc.to_xml
      break
    end
    expect(bad).to be_nil, -> { "comma union != ordered merge:\n#{bad}" }
  end
end
