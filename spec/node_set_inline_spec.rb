# frozen_string_literal: true

# A NodeSet holds its first few nodes in itself and moves to an allocation
# past them (`bridge::node_set`'s INLINE_NODES). Every operation must read the
# same nodes either side of that move, whichever way the set was filled.
RSpec.describe "NodeSet storage across its inline limit", :gc_compact do
  [0, 1, 7, 8, 9, 16, 17, 33].each do |n|
    context "with #{n} nodes" do
      let(:doc) { Makiri.HTML("<ul>#{(1..n).map { |i| "<li id='i#{i}'>#{i}</li>" }.join}</ul>") }
      let(:ids) { (1..n).map { |i| "i#{i}" } }

      it "reads the same nodes from children, css and xpath" do
        ul = doc.at_css("ul")
        expect(ul.children.map { |c| c["id"] }).to eq(ids)
        expect(doc.css("li").map { |c| c["id"] }).to eq(ids)
        expect(doc.xpath("//li").map { |c| c["id"] }).to eq(ids)
      end

      it "keeps them through the set operations and #each" do
        set = doc.css("li")
        expect((set | set).map { |c| c["id"] }).to eq(ids)
        expect((set + doc.xpath("//li")).size).to eq(2 * n)
        expect((set & set).size).to eq(n)
        expect((set - set).size).to eq(0)
        seen = []
        set.each { |c| seen << c["id"] }
        expect(seen).to eq(ids)
        expect(set.to_a.size).to eq(n)
        expect(set.last&.[]("id")).to eq(ids.last)
      end
    end
  end
end
