# frozen_string_literal: true

require "spec_helper"
require_relative "conformance/xml_pbt"

# Property-based tests for the native XML reader (no oracle / metamorphic). The
# generator (spec/conformance/xml_pbt.rb) produces well-formed XML *by
# construction* together with an explicit model, so a parsed tree is expected to
# equal the model exactly. On failure the model is shrunk to a minimal
# counterexample. Differential PBT vs Nokogiri lives in
# spec/conformance/xml_pbt_diff.rb (`rake conformance:xml_pbt`).
#
# PBT_COUNT controls how many random documents each property runs (default 300;
# CI can raise it). Seeds are fixed (0..COUNT-1) so a failure is reproducible.
RSpec.describe "Makiri::XML property-based testing" do
  COUNT = Integer(ENV.fetch("PBT_COUNT", "300"))

  def model_elements(elem)
    1 + elem.children.sum { |c| c.is_a?(XmlPbt::Elem) ? model_elements(c) : 0 }
  end

  def model_count(node, klasses)
    klasses = Array(klasses)
    kids = node.is_a?(XmlPbt::Doc) ? (node.misc_before + [node.root] + node.misc_after) : node.children
    kids.sum { |c| (klasses.any? { |k| c.is_a?(k) } ? 1 : 0) + (c.is_a?(XmlPbt::Elem) ? model_count(c, klasses) : 0) }
  end

  it "round-trips every generated well-formed document (parse == model)" do
    failing = nil
    COUNT.times do |i|
      doc = XmlPbt.gen_document(Random.new(i), max_depth: 5, max_children: 4)
      next if XmlPbt.roundtrips?(doc)

      failing = XmlPbt.shrink(doc) { |d| !XmlPbt.roundtrips?(d) }
      break
    end
    expect(failing).to be_nil, -> { "round-trip mismatch\n#{XmlPbt.report(failing)}" }
  end

  it "is deterministic: parsing the same document twice yields the same tree" do
    bad = nil
    COUNT.times do |i|
      doc = XmlPbt.gen_document(Random.new(i + 1_000), max_depth: 5)
      xml = XmlPbt.serialize(doc)
      a = XmlPbt.dump_parsed(Makiri::XML(xml), XmlPbt::Makiri)
      b = XmlPbt.dump_parsed(Makiri::XML(xml), XmlPbt::Makiri)
      next if a == b

      bad = xml
      break
    end
    expect(bad).to be_nil, -> { "non-deterministic parse:\n#{bad}" }
  end

  it "metamorphic: count(//*) and count(//comment()/processing-instruction()) match the model" do
    bad = nil
    COUNT.times do |i|
      doc = XmlPbt.gen_document(Random.new(i + 2_000), max_depth: 5)
      parsed = Makiri::XML(XmlPbt.serialize(doc))
      checks = {
        "//*"                       => model_elements(doc.root),
        "//comment()"               => model_count(doc, XmlPbt::Comment),
        "//processing-instruction()" => model_count(doc, XmlPbt::Pi),
        # XPath has no CDATA node type: text() matches text AND CDATA nodes.
        "//text()"                  => model_count(doc, [XmlPbt::Text, XmlPbt::CData]),
      }
      bad = checks.find { |expr, want| parsed.xpath("count(#{expr})").to_i != want }
      break if bad
    end
    expect(bad).to be_nil, -> { "metamorphic mismatch: #{bad&.first}" }
  end

  it "to_xml round-trips: re-parsing the (default) serialization yields the same tree" do
    bad = nil
    COUNT.times do |i|
      parsed = Makiri::XML(XmlPbt.serialize(XmlPbt.gen_document(Random.new(i + 4_000), max_depth: 5)))
      want = XmlPbt.dump_parsed(parsed, XmlPbt::Makiri)
      got  = XmlPbt.dump_parsed(Makiri::XML(parsed.to_xml), XmlPbt::Makiri)
      next if want == got

      bad = parsed.to_xml
      break
    end
    expect(bad).to be_nil, -> { "to_xml did not round-trip to the same tree:\n#{bad}" }
  end

  it "pretty to_xml stays well-formed and preserves elements + character content" do
    # pretty-printing adds whitespace (so the tree gains text nodes on reparse),
    # but it must stay well-formed, keep every element, and never alter the text
    # inside a text-bearing element.
    bad = nil
    COUNT.times do |i|
      parsed = Makiri::XML(XmlPbt.serialize(XmlPbt.gen_document(Random.new(i + 5_000), max_depth: 5)))
      pretty = Makiri::XML(parsed.to_xml(pretty: true))
      ok = pretty.xpath("count(//*)") == parsed.xpath("count(//*)") &&
           pretty.root.text.gsub(/\s/, "") == parsed.root.text.gsub(/\s/, "")
      next if ok

      bad = parsed.to_xml(pretty: true)
      break
    end
    expect(bad).to be_nil, -> { "pretty to_xml lost content:\n#{bad}" }
  end

  it "metamorphic: XPath algebraic identities hold on generated documents" do
    bad = nil
    COUNT.times do |i|
      doc = XmlPbt.gen_document(Random.new(i + 3_000), max_depth: 5)
      d = Makiri::XML(XmlPbt.serialize(doc))
      # union with self is idempotent; double negation == boolean; (//*)[last()]
      # is the last in document order; count(//node()) >= count(//*).
      unless d.xpath("count(//* | //*)").to_i == d.xpath("count(//*)").to_i &&
             d.xpath("not(not(//*))") == d.xpath("boolean(//*)") &&
             d.xpath("count(//node()) >= count(//*)") == true &&
             d.xpath("(//*)[last()]").map(&:path) == [d.xpath("//*").to_a.last&.path].compact
        bad = XmlPbt.serialize(doc)
        break
      end
    end
    expect(bad).to be_nil, -> { "XPath identity broken:\n#{bad}" }
  end
end
