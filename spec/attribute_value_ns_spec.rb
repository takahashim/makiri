# frozen_string_literal: true

require "spec_helper"

# `attribute_value_ns(ns, local)` is the DOM's `getAttributeNS`: the value of
# the attribute keyed by (namespace, local name) - the key `set_attribute_ns`
# and `remove_attribute_ns` use - where `[]` matches the qualified name.
RSpec.describe "Node#attribute_value_ns" do
  xlink = "http://www.w3.org/1999/xlink"

  shared_examples "getAttributeNS" do
    it "reads a no-namespace attribute with nil or an empty namespace" do
      expect(element.attribute_value_ns(nil, "href")).to eq("plain")
      expect(element.attribute_value_ns("", "href")).to eq("plain")
    end

    it "reads a namespaced attribute by its URI and local name, not its qualified name" do
      expect(element.attribute_value_ns(xlink, "href")).to eq("linked")
      expect(element.attribute_value_ns(xlink, "xlink:href")).to be_nil
    end

    it "does not take a namespaced attribute for a no-namespace one" do
      expect(element.attribute_value_ns(nil, "only")).to be_nil
      expect(element.attribute_value_ns(xlink, "only")).to eq("ns")
    end

    it "answers nil for an unknown namespace, a missing name and a non-element" do
      expect(element.attribute_value_ns("urn:never", "href")).to be_nil
      expect(element.attribute_value_ns(nil, "missing")).to be_nil
      expect(text.attribute_value_ns(nil, "href")).to be_nil
    end

    it "agrees with what set_attribute_ns wrote and remove_attribute_ns removes" do
      element.set_attribute_ns("urn:k", "k:v", "1")
      expect(element.attribute_value_ns("urn:k", "v")).to eq("1")
      element.remove_attribute_ns("urn:k", "v")
      expect(element.attribute_value_ns("urn:k", "v")).to be_nil
    end

    it "refuses an invalid local name" do
      expect { element.attribute_value_ns(nil, "a\0b") }.to raise_error(Makiri::Error)
    end
  end

  context "HTML" do
    let(:doc) { Makiri::HTML(%(<svg><a href="plain" xlink:href="linked">t</a></svg>)) }
    let(:element) do
      a = doc.at_xpath("//svg:a", "svg" => "http://www.w3.org/2000/svg")
      a.set_attribute_ns(xlink, "xlink:only", "ns")
      a
    end
    let(:text) { element.child }

    include_examples "getAttributeNS"
  end

  context "XML" do
    let(:doc) do
      Makiri::XML(%(<r xmlns:xlink="#{xlink}"><a href="plain" xlink:href="linked" xlink:only="ns">t</a></r>))
    end
    let(:element) { doc.root.child }
    let(:text) { element.child }

    include_examples "getAttributeNS"

    it "reads a namespace declaration in the XMLNS namespace" do
      expect(doc.root.attribute_value_ns("http://www.w3.org/2000/xmlns/", "xlink")).to eq(xlink)
    end
  end
end
