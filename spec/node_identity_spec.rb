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
end
