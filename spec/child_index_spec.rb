# frozen_string_literal: true

require "spec_helper"

# `child_count` / `element_child_count` / `child_at` / `element_child_at` /
# `last_child` answer what `children` / `element_children` answer, without
# building a NodeSet - the shape a `childNodes.length` / `item(i)` loop wants.
RSpec.describe "counting and indexing children in place" do
  def agree_with_children(node)
    kids = node.children.to_a
    elements = node.element_children.to_a
    expect(node.child_count).to eq(kids.size)
    expect(node.element_child_count).to eq(elements.size)
    expect(node.last_child).to equal(kids.last)
    (0..kids.size).each { |i| expect(node.child_at(i)).to equal(kids[i]) }
    (0..elements.size).each { |i| expect(node.element_child_at(i)).to equal(elements[i]) }
  end

  shared_examples "in-place children" do
    it "agrees with children / element_children on every node" do
      stack = [doc]
      while (n = stack.pop)
        agree_with_children(n)
        stack.concat(n.children.to_a)
      end
    end

    it "answers nil for a negative index, as for one past the end" do
      expect(parent.child_at(-1)).to be_nil
      expect(parent.element_child_at(-1)).to be_nil
      expect(parent.child_at(2**70)).to be_nil
    end

    it "refuses an index that is not an Integer" do
      expect { parent.child_at(1.0) }.to raise_error(TypeError)
      expect { parent.element_child_at("0") }.to raise_error(TypeError)
      expect { parent.child_at(nil) }.to raise_error(TypeError)
    end

    it "follows a mutation" do
      before = parent.child_count
      parent.add_child(doc.create_element("q"))
      expect(parent.child_count).to eq(before + 1)
      expect(parent.last_child.name).to eq("q")
      expect(parent.element_child_at(parent.element_child_count - 1).name).to eq("q")
    end

    it "answers 0 and nil on a leaf" do
      leaf = parent.children.find(&:text?)
      expect(leaf.child_count).to eq(0)
      expect(leaf.element_child_count).to eq(0)
      expect(leaf.child_at(0)).to be_nil
      expect(leaf.last_child).to be_nil
    end
  end

  context "HTML" do
    let(:doc) { Makiri::HTML("<div>a<p>1</p><!--c--><p>2</p>b<span></span></div><template><i></i></template>") }
    let(:parent) { doc.at_css("div") }

    include_examples "in-place children"

    it "counts a template's own (empty) children, as children does" do
      template = doc.at_css("template")
      expect(template.child_count).to eq(template.children.size)
      expect(template.child_count).to eq(0)
      expect(template.content_fragment.child_count).to eq(1)
    end

    it "answers 0 for an attribute" do
      element = doc.create_element("x")
      element["a"] = "1"
      attr = element.attribute_nodes.first
      expect(attr.child_count).to eq(0)
      expect(attr.child_at(0)).to be_nil
    end
  end

  context "XML" do
    let(:doc) { Makiri::XML(%(<r>a<p>1</p><!--c--><p>2</p>b<?pi x?><s/></r>)) }
    let(:parent) { doc.root }

    include_examples "in-place children"
  end
end
