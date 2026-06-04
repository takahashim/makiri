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

  describe "fail-closed: unsupported surface raises NotImplementedError" do
    let(:doc) { Makiri::XML("<r><c/></r>") }

    it "rejects CSS selectors and points at #xpath" do
      expect { doc.css("c") }.to raise_error(NotImplementedError, /xpath/)
      expect { doc.at_css("c") }.to raise_error(NotImplementedError)
      expect { doc.root.css("c") }.to raise_error(NotImplementedError)
    end

    it "rejects serialization (read-only)" do
      %i[to_xml to_html to_s inner_html outer_html].each do |m|
        expect { doc.root.public_send(m) }.to raise_error(NotImplementedError)
      end
    end

    it "still supports #inspect for debugging" do
      expect(doc.root.inspect).to include("Makiri::XML::Element")
    end
  end
end
