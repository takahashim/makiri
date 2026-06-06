# frozen_string_literal: true

RSpec.describe Makiri::NodeSet do
  let(:doc) { Makiri::HTML("<ul><li>a</li><li>b</li><li>c</li></ul>") }
  let(:lis) do
    ul = doc.root.element_children.find { |e| e.name == "body" }.first_element_child
    ul.element_children
  end

  it "reports length / size / empty?" do
    expect(lis.length).to eq(3)
    expect(lis.size).to eq(3)
    expect(lis).not_to be_empty
  end

  it "indexes with [] including negatives and out-of-range" do
    expect(lis[0].text).to eq("a")
    expect(lis[-1].text).to eq("c")
    expect(lis[99]).to be_nil
  end

  it "is Enumerable" do
    expect(lis.map(&:text)).to eq(%w[a b c])
    expect(lis.select { |li| li.text != "b" }.map(&:text)).to eq(%w[a c])
    expect(lis.first.text).to eq("a")
  end

  it "returns an Enumerator from #each without a block" do
    expect(lis.each).to be_a(Enumerator)
    expect(lis.each.to_a.map(&:text)).to eq(%w[a b c])
  end

  it "#[] supports ranges and (start, length) like Array" do
    expect(lis[1].text).to eq("b")            # Integer -> Node
    expect(lis[-1].text).to eq("c")
    expect(lis[5]).to be_nil
    expect(lis[0..1]).to be_a(Makiri::NodeSet) # Range -> NodeSet
    expect(lis[0..1].map(&:text)).to eq(%w[a b])
    expect(lis[1..].map(&:text)).to eq(%w[b c])
    expect(lis[0, 2].map(&:text)).to eq(%w[a b]) # (start, length) -> NodeSet
    expect(lis[1, 10].map(&:text)).to eq(%w[b c]) # clamped
    expect(lis[5..6]).to be_nil                # start out of range
    expect(lis[3..].length).to eq(0)           # empty NodeSet
  end

  it "#dup / #clone make an independent set over the same nodes" do
    copy = lis.dup
    expect(copy).to be_a(Makiri::NodeSet)
    expect(copy.length).to eq(3)
    expect(copy.map(&:text)).to eq(%w[a b c])
    expect(copy[0]).to eql(lis[0]) # same underlying nodes
    expect(copy).not_to equal(lis) # but a distinct set
    expect(lis.clone.map(&:text)).to eq(%w[a b c])
  end

  describe ".new" do
    it "builds a set from a document (or node) and a list of its nodes" do
      ns = Makiri::NodeSet.new(doc, lis.to_a)
      expect(ns).to be_a(Makiri::NodeSet)
      expect(ns.map(&:text)).to eq(%w[a b c])
      expect(Makiri::NodeSet.new(doc).length).to eq(0)              # empty
      expect(Makiri::NodeSet.new(lis[0], lis.to_a).length).to eq(3) # a node as the context
    end

    it "fails closed on a foreign-document / cross-representation node or a bad context" do
      foreign = Makiri::HTML("<p>x</p>").at_css("p")
      expect { Makiri::NodeSet.new(doc, [foreign]) }.to raise_error(ArgumentError)
      xnode = Makiri::XML("<x/>").root
      expect { Makiri::NodeSet.new(doc, [xnode]) }.to raise_error(ArgumentError)
      expect { Makiri::NodeSet.new("not a doc") }.to raise_error(TypeError)
      expect { Makiri::NodeSet.new(doc, "not an array") }.to raise_error(TypeError)
    end
  end
end
