# frozen_string_literal: true

# #child_count / #child_at / #element_child_count / #element_child_at count and
# index a child list without building #children, through a per-document memo
# of the last position that every child-list edit invalidates.
RSpec.describe "child counting and indexing" do
  def html_list(doc)
    doc.at_css("#u")
  end

  shared_examples "an indexed child list" do
    it "agrees with #children and #element_children" do
      expect(list.child_count).to eq(list.children.size)
      expect(list.element_child_count).to eq(list.element_children.size)
      list.children.each_with_index { |c, i| expect(list.child_at(i)).to equal(c) }
      list.element_children.each_with_index { |c, i| expect(list.element_child_at(i)).to equal(c) }
    end

    it "answers nil past the end and for a negative or huge index" do
      expect(list.child_at(list.child_count)).to be_nil
      expect(list.element_child_at(list.element_child_count)).to be_nil
      expect(list.child_at(-1)).to be_nil
      expect(list.child_at(2**70)).to be_nil
      expect { list.child_at("0") }.to raise_error(TypeError)
    end

    it "agrees in any access order (front, back, memo)" do
      kids = list.children.to_a
      order = [3, 0, 4, 4, 1, 2, 4, 0, 3]
      order.each { |i| expect(list.child_at(i)).to equal(kids[i]) }
      kids.size.downto(0) { |i| expect(list.child_at(i)).to equal(kids[i]) }
    end

    it "has first_child / last_child" do
      expect(list.first_child).to equal(list.children.first)
      expect(list.last_child).to equal(list.children.last)
      leaf = list.element_child_at(0).child_at(0)
      expect([leaf.first_child, leaf.last_child, leaf.child_count]).to eq([nil, nil, 0])
    end

    it "follows every child-list edit, the memo included" do
      list.child_count
      list.child_at(3)
      list.element_child_count
      extra = list.element_child_at(1)
      extra.remove
      expect(list.child_count).to eq(list.children.size)
      expect(list.child_at(3)).to equal(list.children[3])
      expect(list.element_child_count).to eq(list.element_children.size)
      list.add_child(extra)
      expect(list.child_at(list.child_count - 1)).to equal(extra)
      expect(list.element_child_at(list.element_child_count - 1)).to equal(extra)
    end

    it "keeps the memos of two lists apart" do
      inner = list.element_child_at(0)
      expect(list.child_count).to eq(list.children.size)
      expect(inner.child_count).to eq(inner.children.size)
      expect(list.child_count).to eq(list.children.size)
      expect(list.element_child_count).to eq(list.element_children.size)
      expect(list.child_count).to eq(list.children.size)
    end

    it "keeps an outer list's position while inner lists are read" do
      # The DOM-diff shape: walk a list by index, reading each child's own
      # list in the body. Correctness only - the cost is the memo's business.
      list.child_count.times do |i|
        c = list.child_at(i)
        expect(c).to equal(list.children[i])
        expect(c.child_count).to eq(c.children.size)
        expect(c.child_at(0)).to equal(c.children[0])
      end
    end

    it "matches #children over random edits" do
      rng = Random.new(42)
      pool = list.children.to_a
      200.times do
        case rng.rand(4)
        when 0 then list.child_at(rng.rand(list.child_count + 1))
        when 1 then (c = list.child_at(rng.rand([list.child_count, 1].max))) && c.remove
        when 2 then list.add_child(pool.sample(random: rng))
        else list.element_child_at(rng.rand(list.element_child_count + 1))
        end
        kids = list.children.to_a
        expect(list.child_count).to eq(kids.size)
        i = rng.rand(kids.size + 1)
        expect(list.child_at(i)).to(kids[i] ? equal(kids[i]) : be_nil)
        els = list.element_children.to_a
        j = rng.rand(els.size + 1)
        expect(list.element_child_at(j)).to(els[j] ? equal(els[j]) : be_nil)
      end
    end
  end

  context "with HTML" do
    let(:doc) { Makiri::HTML("<ul id=u>a<li>1</li><!--c--><li>2</li>b<li>3</li></ul>") }
    let(:list) { doc.at_css("#u") }

    include_examples "an indexed child list"

    it "counts a <template>'s own (empty) children, as #children does" do
      t = Makiri::HTML("<template><p>x</p></template>").at_css("template")
      expect([t.child_count, t.child_at(0)]).to eq([0, nil])
      expect(t.content_fragment.child_count).to eq(1)
    end

    it "works on a Document and a fragment" do
      expect(doc.child_count).to eq(doc.children.size)
      expect(doc.child_at(0)).to equal(doc.children[0])
      frag = doc.fragment("<b>1</b>t<i>2</i>")
      expect([frag.child_count, frag.element_child_count]).to eq([3, 2])
      expect(frag.element_child_at(1).name).to eq("i")
    end
  end

  context "with XML" do
    let(:doc) { Makiri::XML("<r a='1'>a<e>1</e><!--c--><e>2</e>b<e>3</e></r>") }
    let(:list) { doc.root }

    include_examples "an indexed child list"

    it "does not count attributes" do
      expect(list.attribute_nodes.size).to eq(1)
      expect(list.child_count).to eq(6)
    end
  end
end
