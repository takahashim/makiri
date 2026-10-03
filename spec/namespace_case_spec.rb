# frozen_string_literal: true

require "spec_helper"

# A namespace URI is an opaque string to the DOM: `fooNamespace` is not
# `foonamespace`, and `HTTP://WWW.W3.ORG/1999/XHTML` is not the HTML namespace.
# Lexbor folds ASCII case when it interns a namespace, so an HTML document used
# to answer `foonamespace`; Makiri interns and looks namespaces up as written.
RSpec.describe "namespace URIs keep their case in an HTML document" do
  let(:html) { Makiri::HTML("<p></p>") }
  let(:xml) { Makiri::XML("<r/>") }

  it "keeps a created element's namespace as written" do
    expect(html.create_element_ns("fooNamespace", "prefix:elem").namespace_uri).to eq("fooNamespace")
    expect(html.create_element_ns("urn:X", "e").namespace_uri).to eq("urn:X")
  end

  it "tells namespaces that differ only in case apart" do
    upper = html.create_element_ns("urn:X", "e")
    lower = html.create_element_ns("urn:x", "e")
    expect([upper.namespace_uri, lower.namespace_uri]).to eq(%w[urn:X urn:x])
  end

  it "does not take a differently cased built-in URI for the built-in namespace" do
    e = html.create_element_ns("HTTP://WWW.W3.ORG/1999/XHTML", "div")
    expect(e.namespace_uri).to eq("HTTP://WWW.W3.ORG/1999/XHTML")
    expect(e.tag_name).to eq("div") # an HTML element's would be "DIV"
    expect(html.create_element_ns("http://www.w3.org/1999/xhtml", "div").tag_name).to eq("DIV")
  end

  it "keeps an attribute's namespace as written, and matches it exactly" do
    p = html.at_css("p")
    p.set_attribute_ns("attrNamespace", "a:x", "1")
    expect(p.attribute_nodes.first.namespace_uri).to eq("attrNamespace")
    p.remove_attribute_ns("ATTRNAMESPACE", "x")
    expect(p["a:x"]).to eq("1")
    p.remove_attribute_ns("attrNamespace", "x")
    expect(p["a:x"]).to be_nil
  end

  it "keeps the namespaces of a node imported from an XML document" do
    e = xml.create_loose_dom_element("prefix:elem", "prefix", "elem", "fooNamespace")
    e.set_attribute_ns("attrNamespace", "a:x", "1")
    e["plain"] = "2"
    copy = html.import_node(e, true)
    expect(copy.namespace_uri).to eq("fooNamespace")
    expect(copy.attribute_nodes.map { |a| [a.name, a.namespace_uri] })
      .to eq([["a:x", "attrNamespace"], ["plain", nil]])
  end

  it "keeps the namespaces of a node imported from another HTML document" do
    e = html.create_element_ns("fooNamespace", "q")
    e["k"] = "v"
    e.set_attribute_ns("attrNamespace", "a:y", "2")
    e << html.create_element_ns("childNamespace", "c")
    other = Makiri::HTML("<p></p>")
    [other.import_node(e, true), e.clone_node(true)].each do |copy|
      expect(copy.namespace_uri).to eq("fooNamespace")
      expect(copy.attribute_nodes.map { |a| [a.name, a.namespace_uri] })
        .to eq([["k", nil], ["a:y", "attrNamespace"]])
      expect(copy.child.namespace_uri).to eq("childNamespace")
    end
    other.at_css("p").add_child(e)
    moved = other.at_css("p").child
    expect([moved.namespace_uri, moved.child.namespace_uri]).to eq(%w[fooNamespace childNamespace])
  end

  it "keeps the namespace of an Attr imported on its own from another HTML document" do
    p = html.at_css("p")
    p.set_attribute_ns("attrNamespace", "a:x", "1")
    p.set_attribute_ns("HTTP://WWW.W3.ORG/1999/XHTML", "b:y", "2")
    other = Makiri::HTML("<p></p>")
    [["attrNamespace", "x"], ["HTTP://WWW.W3.ORG/1999/XHTML", "y"]].each do |ns, local|
      [other.import_node(p.attribute_node_ns(ns, local)), p.attribute_node_ns(ns, local).clone_node]
        .each { |copy| expect(copy.namespace_uri).to eq(ns) }
    end
  end

  it "leaves parsed content in the built-in namespaces" do
    doc = Makiri::HTML(%(<svg><a xlink:href="#x"/></svg><math></math>))
    expect(doc.at_css("svg").namespace_uri).to eq("http://www.w3.org/2000/svg")
    expect(doc.at_css("math").namespace_uri).to eq("http://www.w3.org/1998/Math/MathML")
    expect(doc.at_css("svg").child.attribute_nodes.first.namespace_uri).to eq("http://www.w3.org/1999/xlink")
  end
end
