# frozen_string_literal: true

# The XML node readers that share a name with an HTML one share its meaning,
# and read a node's name fields only for the kinds that have names.
RSpec.describe "Makiri::XML node readers" do
  let(:doc) do
    Makiri::XML(%(<!DOCTYPE r PUBLIC "-//P//EN" "s.dtd"><r a="1" b="2"><x/>t<y/><!--c--><?pi d?></r>))
  end
  let(:doctype) { doc.children.first }
  let(:root) { doc.root }

  describe "a DOCTYPE" do
    it "answers its ids from #public_id / #system_id, and has no prefix or namespace" do
      expect([doctype.public_id, doctype.system_id]).to eq(["-//P//EN", "s.dtd"])
      expect(doctype.prefix).to be_nil
      expect(doctype.namespace_uri).to be_nil
      expect(doctype.local_name).to be_nil
    end

    it "has no value of its own: #value is its (empty) text content" do
      expect(doctype.value).to eq("")
    end
  end

  describe "element navigation" do
    it "skips non-element siblings and children" do
      expect(root.first_element_child.name).to eq("x")
      expect(root.last_element_child.name).to eq("y")
      expect(root.first_element_child.next_element.name).to eq("y")
      expect(root.last_element_child.previous_element.name).to eq("x")
      expect(root.last_element_child.next_element).to be_nil
      expect(root.elements.map(&:name)).to eq(%w[x y])
    end

    it "is nil where there is no element" do
      leaf = root.first_element_child
      expect(leaf.first_element_child).to be_nil
      expect(leaf.last_element_child).to be_nil
    end
  end

  describe "attributes and names" do
    it "reads keys, values and the HTML-surface aliases" do
      expect(root.keys).to eq(%w[a b])
      expect(root.values).to eq(%w[1 2])
      expect(root.get_attribute("a")).to eq("1")
      expect(root.attr("b")).to eq("2")
      expect([root.node_name, root.type, root.tag_name]).to eq(["r", 1, "r"])
    end

    it "answers #target for a processing instruction only" do
      pi = root.children.find { |n| n.node_type == 7 }
      expect(pi.target).to eq("pi")
      expect(root.target).to be_nil
    end

    it "gives an attribute its value from #value, and any other node its text" do
      expect(root.attribute_nodes.first.value).to eq("1")
      expect(root.value).to eq("t")
    end
  end

  describe "Makiri::XML::Namespace" do
    let(:ns_doc) { Makiri::XML(%(<a xmlns="urn:d" xmlns:p="urn:p"><p:b/></a>)) }

    it "is a value object, printable as its URI" do
      b = ns_doc.root.first_element_child
      expect(b.namespace).to eq(Makiri::XML::Namespace.new("p", "urn:p"))
      expect(b.namespace.to_s).to eq("urn:p")
      expect(b.namespace.hash).to eq(Makiri::XML::Namespace.new("p", "urn:p").hash)
      expect(ns_doc.root.namespace.prefix).to be_nil
      expect(b.namespace.inspect).to eq('#<Makiri::XML::Namespace prefix="p" href="urn:p">')
    end

    it "collects every declaration, in scope and in the document" do
      b = ns_doc.root.first_element_child
      expect(b.namespaces).to eq("xmlns" => "urn:d", "xmlns:p" => "urn:p")
      expect(ns_doc.collect_namespaces).to eq("xmlns" => "urn:d", "xmlns:p" => "urn:p")
      expect(ns_doc.root.namespace_definitions.map(&:prefix)).to eq([nil, "p"])
    end
  end
end
