# frozen_string_literal: true

require "spec_helper"

# Makiri::XML::Document#dup copies the document node for node, not through its
# markup: what the original holds is what the copy holds, whether or not XML
# can write it, with no namespace resolved again.
RSpec.describe "Makiri::XML::Document#dup" do
  let(:source) do
    %(<?xml version="1.0" encoding="UTF-8"?>\n) +
      %(<!DOCTYPE r PUBLIC "" "s.dtd"><!--c--><?pi x?>) +
      %(<r xmlns:p="urn:p"><p:e p:a="1">t<![CDATA[x]]></p:e></r><!--after-->)
  end
  let(:doc) { Makiri::XML(source) }

  def shape(node)
    names = node.attribute_nodes.map { |a| [a.name, a.namespace_uri, a.value] }
    [node.class, node.name, (node.namespace_uri if node.element?), names, node.children.map { |c| shape(c) }]
  end

  it "keeps the top-level order, names, namespaces, doctype and data" do
    copy = doc.dup
    expect(copy.children.map { |c| shape(c) }).to eq(doc.children.map { |c| shape(c) })
    expect([copy.internal_subset.public_id, copy.internal_subset.system_id]).to eq(["", "s.dtd"])
    expect(copy.to_xml).to eq(doc.to_xml)
  end

  it "keeps a namespaced attribute's own name rather than inventing a prefix" do
    doc.root.set_attribute_ns("urn:a", "a", "v")
    attr = doc.dup.root.attribute_nodes.find { |a| a.namespace_uri == "urn:a" }
    expect([attr.name, attr.value]).to eq(%w[a v])
    expect(doc.dup.root.attribute_nodes.map(&:name)).not_to include(start_with("xmlns:ns"))
  end

  it "copies what the DOM holds and XML cannot write" do
    doc.root << doc.create_comment("a -- b")
    copy = doc.dup
    expect(copy.root.children.last.content).to eq("a -- b")
    expect { copy.to_xml }.to raise_error(Makiri::Error, /cannot hold/)
  end

  it "shares no node with the original, and starts its own versions" do
    doc.root["x"] = "1"
    copy = doc.dup
    expect([copy.tree_version, copy.attribute_version]).to eq([0, 0])
    copy.root["y"] = "2"
    copy.root.children.first.remove
    expect(doc.root["y"]).to be_nil
    expect(doc.root.children.size).to eq(1)
  end

  it "keeps the original's byte budget" do
    small = Makiri::XML::Document.parse("<r/>", max_bytes: 4096)
    copy = small.dup
    expect { copy.root << copy.create_text_node("y" * 5000) }.to raise_error(Makiri::XML::LimitExceeded)
  end

  # The parser stores a namespace URI once for every node in it; the copy shares
  # it the same way, so a document that fits its budget is copied within it.
  it "copies within the budget the original was parsed under" do
    xml = %(<r xmlns="urn:a-rather-long-default-namespace-uri" xmlns:p="urn:p-long-uri">) +
          (%(<e p:a="1"/>) * 2000) + "</r>"
    budget = (64..2000).map { |k| k * 1024 }.find do |b|
      Makiri::XML::Document.parse(xml, max_bytes: b)
    rescue Makiri::XML::LimitExceeded
      nil
    end
    doc = Makiri::XML::Document.parse(xml, max_bytes: budget)
    expect(doc.dup.to_xml).to eq(doc.to_xml)
  end

  it "copies a document still being built, before its root" do
    built = Makiri::XML::Document.new
    built << built.create_comment("first")
    expect(built.dup.children.map(&:content)).to eq(["first"])
    expect(Makiri::XML::Document.new.dup.children).to be_empty
  end
end
