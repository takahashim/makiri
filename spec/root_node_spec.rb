# frozen_string_literal: true

require "spec_helper"

# `root_node` is the DOM's `getRootNode()` (not shadow-including): where the
# parent links end. One call, where climbing `parent` from Ruby is one per level.
RSpec.describe "Node#root_node" do
  def climbed(node)
    node = node.parent while node.parent
    node
  end

  context "HTML" do
    let(:doc) { Makiri::HTML("<div><p id=a>t<b>x</b></p></div><template><i>y</i></template>") }

    it "is the Document for every connected node, the Document included" do
      expect(doc.root_node).to equal(doc)
      [doc.root, doc.at_css("b"), doc.at_css("b").child].each do |n|
        expect(n.root_node).to equal(doc)
        expect(n.root_node).to equal(climbed(n))
      end
    end

    it "stops at a template's contents, which have no parent" do
      fragment = doc.at_css("template").content_fragment
      expect(fragment.child.child.root_node).to equal(fragment)
      expect(fragment.root_node).to equal(fragment)
    end

    it "is the DocumentFragment for a node inside one" do
      fragment = Makiri::HTML::DocumentFragment.parse("<b><i>1</i></b>")
      expect(fragment.at_css("i").root_node).to equal(fragment)
    end

    it "is the topmost node of a detached subtree, or the node itself" do
      e = doc.create_element("q")
      t = doc.create_text_node("z")
      e << t
      expect(t.root_node).to equal(e)
      expect(e.root_node).to equal(e)
      removed = doc.at_css("p").remove
      expect(removed.at_css("b").root_node).to equal(removed)
    end

    it "is the attribute itself for an attribute, whose owner is not its parent in the DOM" do
      attr = doc.at_css("p").attribute_nodes.first
      expect(attr.root_node).to equal(attr)
    end
  end

  context "XML" do
    let(:doc) { Makiri::XML(%(<r a="1"><c><d/></c></r>)) }

    it "is the Document for every connected node, the Document included" do
      expect(doc.root_node).to equal(doc)
      d = doc.root.at_xpath("c/d")
      expect(d.root_node).to equal(doc)
      expect(d.root_node).to equal(climbed(d))
    end

    it "is the topmost node of a detached subtree" do
      c = doc.root.at_xpath("c")
      c.remove
      expect(c.at_xpath("d").root_node).to equal(c)
    end

    it "is the attribute itself for an attribute" do
      attr = doc.root.attribute_nodes.first
      expect(attr.root_node).to equal(attr)
    end
  end
end
