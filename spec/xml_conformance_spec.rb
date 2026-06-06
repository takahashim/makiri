# frozen_string_literal: true

require "spec_helper"

# XPath 1.0 conformance fixes for Makiri::XML, found by the differential vs
# Nokogiri (spec/conformance/xml_xpath_diff.rb, `rake conformance:xpath_xml`).
RSpec.describe "Makiri::XML XPath conformance" do
  describe "node identity (== / eql? / hash / pointer_id / #path)" do
    let(:doc) { Makiri::XML("<r><a><b/></a><a><b/></a></r>") }

    it "treats two wrappers of the same node as equal" do
      one = doc.at_xpath("//a")
      two = doc.at_xpath("//a")
      expect(one).to eq(two)
      expect(one).to eql(two)
      expect(one.hash).to eq(two.hash)
      expect(one.pointer_id).to eq(two.pointer_id)
      expect(doc.at_xpath("//r/a[1]")).not_to eq(doc.at_xpath("//r/a[2]"))
    end

    it "computes #path (which depends on node equality)" do
      expect(doc.xpath("//b").map(&:path)).to eq(["/r/a[1]/b", "/r/a[2]/b"])
    end

    it "dedupes a NodeSet and works as a Set/Hash key" do
      bs = doc.xpath("//b")
      expect((bs | bs).length).to eq(2)
      expect(bs.to_a.uniq.length).to eq(2)
      expect(bs.to_a.to_set.size).to eq(2)
    end
  end

  describe "CDATA sections are character data (string-value / text())" do
    let(:doc) do
      Makiri::XML("<doc><a><![CDATA[x < y]]></a><b>p<![CDATA[q]]>r</b></doc>")
    end

    it "includes CDATA in an element's string-value" do
      expect(doc.xpath("string(//a)")).to eq("x < y")
      expect(doc.xpath("string(//b)")).to eq("pqr")
    end

    it "agrees with Node#text" do
      expect(doc.xpath("string(//b)")).to eq(doc.at_xpath("//b").text)
    end

    it "selects CDATA content via text()" do
      expect(doc.xpath("//a/text()").map(&:text)).to eq(["x < y"])
    end
  end

  describe "adjacent character data is coalesced (libxml2 / XPath data model)" do
    it "merges adjacent CDATA sections into one node" do
      doc = Makiri::XML("<r><![CDATA[a]]><![CDATA[b]]><![CDATA[c]]></r>")
      expect(doc.root.children.length).to eq(1)
      expect(doc.root.children.first).to be_a(Makiri::XML::CDATASection)
      expect(doc.xpath("//text()").map(&:text)).to eq(["abc"])
      expect(doc.xpath("string(//r)")).to eq("abc")
    end

    it "keeps text and CDATA as distinct nodes (only same-type merges)" do
      doc = Makiri::XML("<r>x<![CDATA[y]]>z</r>")
      kinds = doc.root.children.map { |c| c.class.name.split("::").last }
      expect(kinds).to eq(%w[Text CDATASection Text])
      expect(doc.xpath("count(//text())")).to eq(3.0)
      expect(doc.root.text).to eq("xyz")
    end
  end

  describe "name()/local-name() of a processing instruction (target)" do
    let(:doc) { Makiri::XML("<doc><meta><?render fast?></meta></doc>") }

    it "returns the PI target for both name() and local-name()" do
      # A PI's expanded-name is (null, target), so name() == local-name() == the
      # target. name() used to mis-route PIs through the element accessor and
      # return "" (found by the XML XPath differential).
      expect(doc.xpath("name(//meta/processing-instruction())")).to eq("render")
      expect(doc.xpath("local-name(//meta/processing-instruction())")).to eq("render")
      expect(doc.xpath("namespace-uri(//meta/processing-instruction())")).to eq("")
      expect(doc.xpath("string(//meta/processing-instruction())")).to eq("fast")
    end
  end

  describe "namespace declarations are namespace nodes, not attributes" do
    let(:doc) do
      Makiri::XML(%(<e xmlns:p="urn:p" xmlns="urn:d" id="1"><c p:k="v"/></e>))
    end
    let(:ns) { { "p" => "urn:p", "d" => "urn:d" } }

    it "excludes xmlns declarations from the XPath attribute axis" do
      # <e> has xmlns:p, xmlns (ns decls) and id (a real attribute) -> @* is [id].
      expect(doc.xpath("count(//d:e/@*)", ns)).to eq(1.0)
      expect(doc.xpath("string(//d:e/@id)", ns)).to eq("1")
      expect(doc.xpath("//d:e/@xmlns", ns).length).to eq(0) # unprefixed @xmlns: not on the axis
      # a genuine prefixed attribute IS on the axis
      expect(doc.xpath("string(//d:c/@p:k)", ns)).to eq("v")
    end

    it "still exposes xmlns as DOM attributes (Node#attribute_nodes / #[])" do
      expect(doc.root.attribute_nodes.map(&:name)).to include("xmlns:p", "xmlns", "id")
      expect(doc.root["xmlns:p"]).to eq("urn:p")
    end
  end
end
