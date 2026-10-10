# frozen_string_literal: true

# Node#root_node: DOM getRootNode() (not shadow-including).
RSpec.describe "Node#root_node" do
  it "is the Document for a node in the tree, and the Document itself" do
    doc = Makiri::HTML("<p><b>x</b></p>")
    expect(doc.at_css("b").child.root_node).to equal(doc)
    expect(doc.root_node).to equal(doc)
  end

  it "is the topmost node of a detached subtree" do
    doc = Makiri::HTML("<div><p><b>x</b></p></div>")
    p = doc.at_css("p").remove
    expect(p.root_node).to equal(p)
    expect(p.at_css("b").child.root_node).to equal(p)
  end

  it "is the fragment for a fragment's nodes, a template's contents included" do
    doc = Makiri::HTML("<template><p><b>x</b></p></template>")
    contents = doc.at_css("template").content_fragment
    expect(contents.child_at(0).child_at(0).root_node).to equal(contents)
    frag = doc.fragment("<i>y</i>")
    expect(frag.child_at(0).child_at(0).root_node).to equal(frag)
  end

  it "is the Attr itself for an attribute, as in the DOM" do
    doc = Makiri::HTML("<p class=k></p>")
    attr = doc.at_css("p").attribute_nodes.first
    expect(attr.root_node).to equal(attr)
  end

  it "works on XML" do
    doc = Makiri::XML("<r a='1'><e>x</e></r>")
    expect(doc.at_xpath("//e").child.root_node).to equal(doc)
    attr = doc.root.attribute_nodes.first
    expect(attr.root_node).to equal(attr)
    e = doc.at_xpath("//e").remove
    expect(e.child.root_node).to equal(e)
  end
end
