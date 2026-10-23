# frozen_string_literal: true

# `Node#traverse` lists the subtree natively (`_traverse_order`), in the order
# the recursive definition over `children` visits it: each node after its
# children, the receiver last.
RSpec.describe "Node#traverse" do
  def recursive(node, out = [])
    node.children.each { |c| recursive(c, out) }
    out << node
  end

  def visited(node)
    out = []
    node.traverse { |n| out << n }
    out
  end

  {
    "an HTML document" => -> { Makiri.HTML(<<~HTML) },
      <!doctype html><html><head><title>t</title></head><body>
      <div id="a"><p>one <b>two</b> three</p><!-- c --><template><i>t</i></template>
      <svg><circle/><g><path/></g></svg><ul>#{(1..20).map { |i| "<li>#{i}</li>" }.join}</ul></div>
      </body></html>
    HTML
    "an XML document" => -> { Makiri::XML(%(<?xml version="1.0"?><r xmlns:p="urn:p"><p:a k="v">x<![CDATA[y]]><?pi d?></p:a><b/><!-- c --></r>)) },
    "a fragment" => -> { Makiri::HTML::DocumentFragment.parse("<p>a</p>text<span><i>b</i></span>") },
  }.each do |what, make|
    it "visits #{what} as the recursive definition does" do
      root = make.call
      ([root] + root.xpath("//*").to_a).each do |n|
        expect(visited(n)).to eq(recursive(n))
      end
    end
  end

  it "visits a leaf alone, and returns the receiver" do
    t = Makiri.HTML("<p>x</p>").at_css("p").children.first
    expect(visited(t)).to eq([t])
    expect(t.traverse {}).to equal(t)
  end

  it "visits the nodes listed before the block runs" do
    doc = Makiri.HTML("<div><p>a</p><p>b</p></div>")
    names = []
    doc.at_css("div").traverse do |n|
      names << n.name
      n.add_next_sibling(doc.create_element("em")) if n.name == "p"
    end
    expect(names).to eq(%w[text p text p div])
  end

  it "walks a deep tree without native recursion" do
    doc = Makiri.HTML("<div>" * 350 + "x" + "</div>" * 350)
    expect(visited(doc).size).to eq(doc.xpath("count(//node())").to_i + 1)
  end
end
