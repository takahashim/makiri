# frozen_string_literal: true

# An HTML document keeps its document-order index across evaluates: the first
# sort large enough to want one builds it, every later evaluate reads it, and
# an edit drops it. These examples hold the sorted answers to the plain
# document order (a single-path `//node()`, which needs no sort) through the
# index's whole life: built, reused, dropped by each kind of edit, and asked
# about nodes it does not hold.
RSpec.describe "XPath document order kept by an HTML document" do
  let(:html) do
    rows = (1..300).map { |i| %(<li id="i#{i}"><a href="/#{i}">a#{i}</a><span>s#{i}</span></li>) }.join
    "<!doctype html><html><body><ul>#{rows}</ul></body></html>"
  end

  # The answer the sort must give: `nodes` in the order a walk from `root`
  # meets them. The walk is Ruby's, over `children`: an XPath reference
  # (`.//node()`, two steps) would be sorted by the very index under test.
  def in_walk_order(root, nodes)
    rank = {}
    stack = [root]
    until stack.empty?
      n = stack.pop
      rank[n] = rank.size
      stack.concat(n.children.to_a.reverse)
    end
    nodes.sort_by { |n| rank.fetch(n) }
  end

  def union(root)
    root.xpath(".//span | .//a").to_a
  end

  it "sorts a union in document order, built and reused" do
    doc = Makiri::HTML(html)
    first = union(doc)
    expect(first.size).to eq(600)
    expect(first).to eq(in_walk_order(doc, first))
    expect(union(doc)).to eq(first)
  end

  it "answers the tree as it is after each kind of edit" do
    doc = Makiri::HTML(html)
    ul = doc.at_css("ul")
    union(doc) # builds the index

    # Each edit moves or adds nodes the index (if it were kept) would place
    # elsewhere: moving a parsed <li> first, while its descendants still hold
    # their old positions.
    edits = [
      -> { ul.children.first.add_previous_sibling(ul.children.last) },
      -> { ul.children[3].add_next_sibling(ul.children[250]) },
      -> { ul.add_child(doc.create_element("a")) },
      -> { ul.children[10].remove },
      -> { ul.children[20].inner_html = "<span>x</span><a>y</a>" },
      -> { doc.at_css("body").add_child(doc.fragment("<a>f</a><span>g</span>")) },
      -> { ul.children[5].replace(doc.create_element("span")) },
    ]
    edits.each_with_index do |edit, i|
      edit.call
      got = union(doc)
      expect(got).to eq(in_walk_order(doc, got)), "after edit #{i}"
    end
  end

  it "sorts nodes outside the document's tree by walking, while the index is built" do
    doc = Makiri::HTML(html)
    union(doc)

    frag = doc.fragment("<p>" + ("<span>s</span><a>a</a>" * 150) + "</p>")
    got = union(frag)
    expect(got.size).to eq(300)
    expect(got).to eq(in_walk_order(frag, got))

    doc.at_css("body").inner_html = "<template>#{"<span>s</span><a>a</a>" * 150}</template>"
    union(doc)
    contents = doc.at_css("template").content_fragment
    got = union(contents)
    expect(got.size).to eq(300)
    expect(got).to eq(in_walk_order(contents, got))
  end

  it "shares the index with an evaluate a handler starts" do
    doc = Makiri::HTML(html)
    inner = nil
    handler = Object.new
    handler.define_singleton_method(:probe) do |nodes|
      inner = doc.xpath("//span | //a").to_a
      nodes
    end
    got = doc.xpath("probe(//a | //span)", handler).to_a
    expect(got).to eq(in_walk_order(doc, got))
    expect(inner).to eq(got)
  end

  it "reports the index to the GC while it is kept, and takes it back with an edit" do
    require "objspace"
    doc = Makiri::HTML(html)
    before = ObjectSpace.memsize_of(doc)
    union(doc)
    built = ObjectSpace.memsize_of(doc)
    # Four bytes for every node of the arena, attributes included.
    expect(built - before).to be >= 4 * 600 * 2
    doc.at_css("ul").add_child(doc.create_element("li"))
    expect(ObjectSpace.memsize_of(doc)).to be < built
  end
end
