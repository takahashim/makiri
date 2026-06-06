# frozen_string_literal: true

require "spec_helper"

# Makiri's canonical node-class names are the WHATWG DOM interface names; the two
# that differ from libxml2/Nokogiri's spelling (CDATASection, DocumentType) also
# expose the Nokogiri name as an alias so ported is_a?/case checks resolve.
RSpec.describe "Nokogiri-compatible class aliases" do
  it "CDATA aliases CDATASection at every level (same class object)" do
    expect(Makiri::CDATA).to equal(Makiri::CDATASection)
    expect(Makiri::HTML::CDATA).to equal(Makiri::HTML::CDATASection)
    expect(Makiri::XML::CDATA).to equal(Makiri::XML::CDATASection)
  end

  it "DTD aliases DocumentType at every level (same class object)" do
    expect(Makiri::DTD).to equal(Makiri::DocumentType)
    expect(Makiri::HTML::DTD).to equal(Makiri::HTML::DocumentType)
    expect(Makiri::XML::DTD).to equal(Makiri::XML::DocumentType)
  end

  it "a CDATA node resolves under both the DOM and the Nokogiri name" do
    node = Makiri::XML("<r><![CDATA[x]]></r>").root.children.first
    expect(node).to be_a(Makiri::XML::CDATASection) # DOM name (also node.class)
    expect(node).to be_a(Makiri::XML::CDATA)        # Nokogiri alias
    expect(node.class.name).to eq("Makiri::XML::CDATASection")
  end

  it "a doctype node is a DocumentType under both names and the shared base" do
    html = Makiri::HTML("<!DOCTYPE html><html></html>").internal_subset
    xml  = Makiri::XML(%(<!DOCTYPE r SYSTEM "r.dtd"><r/>)).internal_subset
    [html, xml].each do |dt|
      expect(dt).to be_a(Makiri::DocumentType) # shared base, both HTML and XML
      expect(dt).to be_a(Makiri::DTD)          # Nokogiri alias
    end
    expect(xml).to be_a(Makiri::XML::DocumentType)
    expect(xml).to be_a(Makiri::XML::DTD)
    expect(xml.class.name).to eq("Makiri::XML::DocumentType")
  end
end
