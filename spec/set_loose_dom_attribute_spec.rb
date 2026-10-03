# frozen_string_literal: true

require "spec_helper"

# `set_loose_dom_attribute(name, value)` is the DOM's `setAttribute` under one
# name for both representations: the whole name is the local name, colons and
# all, and the attribute is in no namespace - where `set_attribute_ns(nil,
# "v-on:click")` is a NamespaceError, as the DOM says. On an HTML element of an
# HTML document the name is lower-cased first; nowhere else.
RSpec.describe "Element#set_loose_dom_attribute" do
  def described(element)
    element.attribute_nodes.map { |a| [a.name, a.local_name, a.prefix, a.namespace_uri] }
  end

  shared_examples "setAttribute" do |lower:|
    it "makes a no-namespace attribute named by the whole name" do
      element.set_loose_dom_attribute("v-on:click", "a")
      element.set_loose_dom_attribute("xmlns", "b")
      element.set_loose_dom_attribute("Xlink:Href", "c")
      href = lower ? "xlink:href" : "Xlink:Href"
      expect(described(element)).to eq(
        [["v-on:click", "v-on:click", nil, nil], ["xmlns", "xmlns", nil, nil], [href, href, nil, nil]],
      )
    end

    it "changes the attribute with that qualified name rather than adding one" do
      element.set_loose_dom_attribute("v-on:click", "a")
      expect(element.set_loose_dom_attribute("v-on:click", "z")).to eq("z")
      expect(element.attribute_nodes.size).to eq(1)
      expect(element.attribute_value_ns(nil, "v-on:click")).to eq("z")
    end

    it "refuses a name the DOM refuses, with ArgumentError" do
      expect { element.set_loose_dom_attribute("a b", "1") }.to raise_error(ArgumentError)
    end
  end

  context "on an HTML element in an HTML document" do
    let(:element) { Makiri::HTML("<div></div>").at_css("div") }

    include_examples "setAttribute", lower: true

    it "is the same as []=" do
      element["V-On:Click"] = "x"
      element.set_loose_dom_attribute("v-on:click", "y")
      expect(described(element)).to eq([["v-on:click", "v-on:click", nil, nil]])
    end
  end

  context "on a foreign element in an HTML document" do
    let(:element) { Makiri::HTML("<p></p>").create_element_ns("http://www.w3.org/2000/svg", "svg") }

    include_examples "setAttribute", lower: false
  end

  context "on an element of an XML document" do
    let(:element) { Makiri::XML("<r/>").root }

    include_examples "setAttribute", lower: false
  end
end
