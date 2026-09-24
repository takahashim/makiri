# frozen_string_literal: true

require "spec_helper"

# Navigating to the same node twice gives the SAME Ruby object. Without that,
# everything that lives on a Ruby object was silently lost - `equal?` was false
# for one node, an instance variable set through one wrapper was gone through the
# next, a singleton method vanished, and `freeze` protected only the object you
# happened to be holding. `==` / `eql?` / `hash` were unaffected, because those
# are node identity, which is why it went unnoticed.
#
# The wrappers live in a cache on the Document (`bridge::wrapper::NodeCache`), so
# one stays alive while its document does. That is Nokogiri's cost for Nokogiri's
# behaviour; measured on a 50,000-node document, wrapping 1,000 nodes costs
# nothing and wrapping all 50,000 adds ~1.8 ms per minor GC (Nokogiri: ~2.0 ms).
RSpec.describe "node wrapper identity" do
  shared_examples "one wrapper per node" do
    it "returns the same object for the same node" do
      expect(node.call).to equal(node.call)
      expect(node.call.object_id).to eq(node.call.object_id)
    end

    it "keeps an instance variable set through another wrapper" do
      node.call.instance_variable_set(:@memo, 42)
      expect(node.call.instance_variable_get(:@memo)).to eq(42)
    end

    it "keeps a singleton method" do
      node.call.define_singleton_method(:shout) { "hi" }
      expect(node.call.shout).to eq("hi")
    end

    it "keeps freeze, and the mutators keep refusing" do
      node.call.freeze
      expect(node.call).to be_frozen
      expect { node.call["z"] = "1" }.to raise_error(FrozenError)
    end

    it "keeps freeze after every reference is dropped and the heap collected" do
      node.call.freeze
      GC.start
      GC.start
      expect(node.call).to be_frozen
    end

    it "still works as a Hash key and in a Set" do
      require "set"
      expect(Set[node.call, node.call].size).to eq(1)
      expect([node.call, node.call].uniq.size).to eq(1)
      expect({ node.call => :v }[node.call]).to eq(:v)
    end
  end

  context "HTML" do
    let(:doc) { Makiri::HTML("<div><p id='x'>t</p></div>") }
    let(:node) { -> { doc.at_css("p") } }
    include_examples "one wrapper per node"
  end

  context "XML" do
    let(:doc) { Makiri::XML(%(<r a="1"><c/></r>)) }
    let(:node) { -> { doc.root } }
    include_examples "one wrapper per node"
  end

  it "gives each node its own wrapper, not one shared wrapper" do
    doc = Makiri::XML("<r><a/><b/></r>")
    first, second = doc.root.children[0], doc.root.children[1]
    expect(first).not_to equal(second)
    expect(doc.root.children[0]).to equal(first)
    expect(doc.root.children[1]).to equal(second)
  end

  it "does not cache across documents" do
    a = Makiri::XML("<r/>")
    b = Makiri::XML("<r/>")
    expect(a.root).not_to equal(b.root)
  end

  it "leaves a Document its own wrapper, as it always was" do
    doc = Makiri::XML("<r/>")
    expect(doc.root.document).to equal(doc)
  end

  # content= on an element and delete(name) went through Lexbor calls that
  # DESTROY the nodes they remove. A wrapper still held for one pointed at freed
  # arena memory, and the next node allocated there came back under that
  # wrapper: a text node answering as an Element or an Attr (the invariant
  # check found both). Makiri detaches, never destroys - so a held wrapper
  # keeps its own node, and a new node gets its own wrapper.
  describe "nodes a mutation removes" do
    let(:doc) { Makiri::HTML("<div><p id=k><span>a</span><b>b</b></p></div>") }

    def churn(doc, parent)
      200.times do |i|
        parent.add_child(doc.create_text_node("t#{i}"))
        parent["a#{i}"] = "v"
      end
    end

    def expect_classes_agree(doc)
      want = { 1 => Makiri::Element, 2 => Makiri::Attr, 3 => Makiri::Text }
      doc.xpath("//node() | //@*").each do |n|
        expect(n).to be_a(want.fetch(n.node_type, Makiri::Node)), "#{n.class} for node type #{n.node_type}"
      end
    end

    it "keeps children replaced by content= alive for their wrappers" do
      para = doc.at_css("p")
      kids = para.children.to_a
      para.content = "x"
      churn(doc, para)
      expect(kids.map(&:name)).to eq(%w[span b])
      expect(kids.map(&:parent)).to eq([nil, nil])
      expect_classes_agree(doc)
    end

    it "keeps an attribute removed by delete alive for its wrapper" do
      para = doc.at_css("p")
      attr = para.attribute_nodes.first
      para.delete("id")
      churn(doc, para)
      expect(attr.name).to eq("id")
      expect(attr.value).to eq("k")
      expect(attr.parent).to be_nil
      expect_classes_agree(doc)
    end
  end
end
