# frozen_string_literal: true

require "spec_helper"

# Reading a parsed XML document from Ruby: XPath queries over the native XML
# engine instance (§2.5 monomorphization) and the Makiri::XML::* node read API.
RSpec.describe "Makiri::XML reading" do
  let(:doc) do
    Makiri::XML(<<~XML)
      <feed xmlns:atom="urn:atom">
        <entry id="1"><title>Hello</title><atom:rank>5</atom:rank></entry>
        <entry id="2"><title>World &amp; Co</title></entry>
        <!-- a comment -->
      </feed>
    XML
  end

  describe "the hierarchy" do
    it "is a Makiri::XML::Document and a Makiri::Document" do
      expect(doc).to be_a(Makiri::XML::Document)
      expect(doc).to be_a(Makiri::Document)
    end

    it "wraps element results as Makiri::XML::Element (is_a? Makiri::Element)" do
      el = doc.at_xpath("//entry")
      expect(el).to be_a(Makiri::XML::Element)
      expect(el).to be_a(Makiri::Element)
      expect(el.element?).to be true
    end

    it "does not inherit HTML-only readers (fail-closed by class)" do
      expect(doc).not_to respond_to(:quirks_mode)
      expect(doc.at_xpath("//entry")).not_to respond_to(:content_fragment)
    end
  end

  describe "#xpath / #at_xpath" do
    it "returns a NodeSet for a node-set" do
      set = doc.xpath("//entry")
      expect(set).to be_a(Makiri::NodeSet)
      expect(set.length).to eq(2)
    end

    it "returns the first node for #at_xpath" do
      expect(doc.at_xpath("//entry")["id"]).to eq("1")
    end

    it "evaluates predicates, positions and string/number/boolean results" do
      expect(doc.at_xpath('//entry[@id="2"]/title').text).to eq("World & Co")
      expect(doc.xpath("//entry[1]").length).to eq(1)
      expect(doc.xpath("count(//entry)")).to eq(2.0)
      expect(doc.xpath("//title/text()").map(&:text)).to eq(["Hello", "World & Co"])
      expect(doc.xpath("boolean(//entry)")).to be true
    end

    it "raises a Makiri error on a malformed expression" do
      expect { doc.xpath("//[") }.to raise_error(Makiri::XPath::SyntaxError)
    end
  end

  describe "node readers" do
    it "reads name / local_name / text / attributes" do
      e = doc.at_xpath("//entry")
      expect(e.name).to eq("entry")
      expect(e.local_name).to eq("entry")
      expect(e["id"]).to eq("1")
      expect(e.at_xpath("title").text).to eq("Hello")
    end

    it "preserves case and namespace URIs (no HTML lowercasing)" do
      rank = doc.at_xpath("//atom:rank", "atom" => "urn:atom") rescue nil
      # Phase 1 Document#xpath has no per-call ns registration yet; reach it via
      # the un-prefixed local match disabled, so verify the parsed node directly.
      r = doc.xpath("//*").find { |n| n.local_name == "rank" }
      expect(r.name).to eq("atom:rank")
      expect(r.prefix).to eq("atom")
      expect(r.namespace_uri).to eq("urn:atom")
    end

    it "navigates parent / children / siblings" do
      title = doc.at_xpath("//entry/title")
      expect(title.parent.name).to eq("entry")
      expect(doc.root.name).to eq("feed")
      expect(doc.root.children.select(&:element?).map(&:name)).to eq(%w[entry entry])
    end

    it "honours strict namespace matching for unprefixed tests" do
      # atom:rank is in urn:atom, so an unprefixed //rank must NOT match it.
      expect(doc.xpath("//rank")).to be_empty
      expect(doc.xpath("//title").length).to eq(2) # no-namespace elements match
    end
  end
end
