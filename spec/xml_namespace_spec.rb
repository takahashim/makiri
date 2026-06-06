# frozen_string_literal: true

require "spec_helper"

# Namespaced XPath over XML (the strict-matching path RSS/Atom need) and the
# fail-closed guards for the surface XML does not support (CSS, serialization).
RSpec.describe "Makiri::XML namespaces & unsupported surface" do
  # A default-namespace document (like Atom): under strict matching an
  # unprefixed //entry does NOT match; a registered prefix is required.
  let(:atom) do
    Makiri::XML(<<~XML)
      <feed xmlns="http://www.w3.org/2005/Atom">
        <entry><title>Hi</title></entry>
        <entry><title>Yo</title></entry>
      </feed>
    XML
  end
  let(:ns) { { "a" => "http://www.w3.org/2005/Atom" } }

  describe "strict default-namespace matching" do
    it "does not match an unprefixed name test against a default-namespace node" do
      expect(atom.xpath("//entry").length).to eq(0)
    end
  end

  describe "#xpath / #at_xpath with a per-call namespaces hash" do
    it "selects default-namespace nodes via a registered prefix" do
      expect(atom.xpath("//a:entry", ns).length).to eq(2)
    end

    it "works for at_xpath and returns a single node" do
      node = atom.at_xpath("//a:title", ns)
      expect(node.text).to eq("Hi")
    end

    it "is rooted at the receiver node for a node-relative query" do
      first = atom.at_xpath("//a:entry", ns)
      expect(first.xpath(".//a:title", ns).map(&:text)).to eq(["Hi"])
    end

    it "accepts symbol keys / values too" do
      expect(atom.xpath("//a:entry", a: "http://www.w3.org/2005/Atom").length).to eq(2)
    end

    it "returns an empty set (not an error) when the prefix maps to a non-matching uri" do
      expect(atom.xpath("//a:entry", "a" => "urn:nope").length).to eq(0)
    end

    it "raises (and does not leak) on a malformed namespace value" do
      # An embedded NUL fails the engine-string validation; the per-call context
      # is freed before the raise (the suite runs under ASan to catch a leak).
      bad = "urn:" + 0.chr + "x"
      expect { atom.xpath("//a:entry", "a" => bad) }.to raise_error(Makiri::Error)
    end

    it "raises TypeError when namespaces is not a Hash" do
      expect { atom.xpath("//a:entry", "not a hash") }.to raise_error(TypeError)
    end
  end

  describe "Makiri::XPathContext over an XML node" do
    it "evaluates namespaced expressions after register_namespace" do
      ctx = Makiri::XPathContext.new(atom.root)
      ctx.register_namespace("a", "http://www.w3.org/2005/Atom")
      expect(ctx.evaluate("//a:entry").length).to eq(2)
      expect(ctx.evaluate("string(//a:entry[1]/a:title)")).to eq("Hi")
    end

    it "binds the context node for relative expressions" do
      first = atom.at_xpath("//a:entry", ns)
      ctx = Makiri::XPathContext.new(first)
      ctx.register_ns("a", "http://www.w3.org/2005/Atom")
      expect(ctx.evaluate("count(.//a:title)")).to eq(1.0)
    end
  end

  describe "CSS selectors (lowered to the native XPath engine)" do
    let(:doc) { Makiri::XML("<r><c/><c/></r>") }

    it "queries with #css / #at_css and tests #matches?" do
      expect(doc.css("c").length).to eq(2)
      expect(doc.at_css("c")).to eq(doc.root.children.first)
      expect(doc.root.css("c").length).to eq(2)
      expect(doc.at_css("c").matches?("r > c")).to be(true)
    end
  end

  describe "fail-closed: unsupported surface raises NotImplementedError" do
    let(:doc) { Makiri::XML("<r><c/></r>") }

    it "serializes via #to_xml / #to_s but rejects HTML serialization" do
      expect(doc.root.to_xml).to be_a(String)
      expect(doc.root.to_s).to eq(doc.root.to_xml)
      %i[to_html inner_html outer_html].each do |m|
        expect { doc.root.public_send(m) }.to raise_error(NotImplementedError)
      end
    end

    it "still supports #inspect for debugging" do
      expect(doc.root.inspect).to include("Makiri::XML::Element")
    end
  end

  describe "namespace introspection (Nokogiri-compatible)" do
    let(:doc) do
      Makiri::XML(%(<root xmlns="urn:d" xmlns:p="urn:a">) +
                  %(<p:child x="1" p:y="2"><inner/></p:child></root>))
    end
    let(:root)  { doc.root }
    let(:child) { doc.at_xpath("//p:child", "p" => "urn:a") }
    let(:inner) { doc.at_xpath("//d:inner", "d" => "urn:d", "p" => "urn:a") }

    it "#namespace returns the node's resolved namespace (or nil)" do
      expect(root.namespace.prefix).to be_nil           # default namespace
      expect(root.namespace.href).to eq("urn:d")
      expect(child.namespace.prefix).to eq("p")
      expect(child.namespace.href).to eq("urn:a")
      expect(inner.namespace.href).to eq("urn:d")        # inherits the default ns
      # a prefixed attribute has a namespace; an unprefixed one does not
      attrs = child.attribute_nodes
      expect(attrs.find { |a| a.name == "p:y" }.namespace.href).to eq("urn:a")
      expect(attrs.find { |a| a.name == "x" }.namespace).to be_nil
    end

    it "#namespace_definitions lists the xmlns declarations ON the element" do
      expect(root.namespace_definitions.map { |n| [n.prefix, n.href] })
        .to contain_exactly([nil, "urn:d"], ["p", "urn:a"])
      expect(child.namespace_definitions).to eq([]) # none declared here
    end

    it "#namespaces returns all in-scope declarations, inner scope winning" do
      expect(inner.namespaces).to eq("xmlns" => "urn:d", "xmlns:p" => "urn:a")
      nested = Makiri::XML(%(<a xmlns="urn:1"><b xmlns="urn:2"><c/></b></a>))
      c = nested.at_xpath("//x:c", "x" => "urn:2")
      expect(c.namespaces).to eq("xmlns" => "urn:2") # closer xmlns overrides
      # the predefined xml prefix is implicit, not a declaration -> not listed
      expect(Makiri::XML(%(<r xml:lang="en"/>)).root.namespaces).to eq({})
    end

    it "Document#collect_namespaces gathers every declaration in the document" do
      d = Makiri::XML(%(<a xmlns:p="urn:p"><b xmlns:q="urn:q"/></a>))
      expect(d.collect_namespaces).to eq("xmlns:p" => "urn:p", "xmlns:q" => "urn:q")
    end

    it "Namespace is a (prefix, href) value object" do
      ns = root.namespace_definitions.find { |n| n.prefix == "p" }
      expect(ns.href).to eq("urn:a")
      expect(ns.to_s).to eq("urn:a")
      expect(ns).to eq(child.namespace)        # value equality
      expect(ns.hash).to eq(child.namespace.hash)
    end
  end
end
