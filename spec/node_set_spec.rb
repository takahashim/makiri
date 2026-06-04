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

  it "#dup / #clone make an independent set over the same nodes" do
    copy = lis.dup
    expect(copy).to be_a(Makiri::NodeSet)
    expect(copy.length).to eq(3)
    expect(copy.map(&:text)).to eq(%w[a b c])
    expect(copy[0]).to eql(lis[0]) # same underlying nodes
    expect(copy).not_to equal(lis) # but a distinct set
    expect(lis.clone.map(&:text)).to eq(%w[a b c])
  end
end
