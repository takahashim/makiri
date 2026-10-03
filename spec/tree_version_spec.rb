# frozen_string_literal: true

require "spec_helper"

# `Document#tree_version` is the key a child-list cache is kept under: it grows
# with every edit that can change a child list of a node the document owns -
# attached, detached or in a fragment - and attribute and character-data edits
# (a Text, Comment, CDATA or PI node's `content=`) leave it alone. The
# promise a cache relies on is the converse: an unchanged version means no
# child list changed, which the randomized example at the bottom checks.
RSpec.describe "Document#tree_version" do
  def bumps(doc)
    before = doc.tree_version
    yield
    expect(doc.tree_version).to be > before
  end

  def keeps(doc)
    before = doc.tree_version
    yield
    expect(doc.tree_version).to eq(before)
  end

  it "starts as an Integer and reads without allocating" do
    doc = Makiri::HTML("<p>a</p>")
    expect(doc.tree_version).to be_a(Integer)
    doc.tree_version
    before = GC.stat(:total_allocated_objects)
    100.times { doc.tree_version }
    expect(GC.stat(:total_allocated_objects) - before).to be < 10
  end

  context "HTML" do
    let(:doc) { Makiri::HTML(%(<div id="d"><p id="p">t</p><i>x</i></div><template><b>1</b></template>)) }
    let(:div) { doc.at_css("div") }
    let(:para) { doc.at_css("p") }

    it "grows with every structural edit" do
      bumps(doc) { div.add_child(doc.create_element("a")) }
      bumps(doc) { div << doc.create_text_node("z") }
      bumps(doc) { para.before(doc.create_element("a")) }
      bumps(doc) { para.after(doc.create_comment("c")) }
      bumps(doc) { doc.at_css("i").replace(doc.create_element("u")) }
      bumps(doc) { div.inner_html = "<p>new</p>" }
      bumps(doc) { div.at_css("p").outer_html = "<em>e</em>" }
      bumps(doc) { div.content = "plain" }
      bumps(doc) { div.child.remove }
      bumps(doc) { doc.at_css("template").inner_html = "<s>2</s>" }
    end

    it "grows for edits inside a detached subtree and a fragment" do
      detached = doc.create_element("q")
      bumps(doc) { detached << doc.create_element("r") }
      frag = doc.fragment("<a>1</a>")
      bumps(doc) { frag.add_child(doc.create_element("b")) }
      bumps(doc) { doc.at_css("template").content_fragment << doc.create_element("c") }
    end

    it "grows for the source and the target of a move between documents" do
      other = Makiri::HTML("<span>s</span>")
      bumps(doc) { bumps(other) { div.add_child(other.at_css("span")) } }
      frag = Makiri::HTML::DocumentFragment.parse("<em>1</em><em>2</em>")
      bumps(doc) { bumps(frag.document) { div.add_child(frag) } }
    end

    it "grows when a fragment is spliced in and so emptied" do
      frag = doc.fragment("<a>1</a><a>2</a>")
      bumps(doc) { div.add_child(frag) }
      expect(frag.children.size).to eq(0)
    end

    it "is left alone by character-data edits" do
      text = para.child
      comment = doc.create_comment("c")
      para << comment
      pi = doc.create_processing_instruction("t", "d")
      para << pi
      keeps(doc) do
        text.content = "text data"
        text.content = ""
        comment.content = "c2"
        pi.content = "d2"
      end
      expect(para.text).to eq("")
    end

    it "is left alone by a refused Attr#remove" do
      para["k"] = "v"
      keeps(doc) do
        expect { para.attribute_nodes.first.remove }.to raise_error(Makiri::Error)
      end
    end

    it "is left alone by attribute edits and by reads" do
      keeps(doc) do
        para["class"] = "x"
        para.set_attribute_ns("urn:k", "k:a", "1")
        para.remove_attribute_ns("urn:k", "a")
        para.delete("class")
        div.css("p")
        div.at_xpath(".//p")
        div.to_html
        div.children
      end
    end
  end

  context "XML" do
    let(:doc) { Makiri::XML(%(<r xmlns:k="urn:k"><p a="1">t</p><i/></r>)) }
    let(:root) { doc.root }
    let(:para) { root.at_xpath("p") }

    it "grows with every structural edit" do
      bumps(doc) { root.add_child(doc.create_element("a")) }
      bumps(doc) { para.before(doc.create_element("b")) }
      bumps(doc) { para.after(doc.create_comment("c")) }
      bumps(doc) { root.at_xpath("i").replace(doc.create_element("u")) }
      bumps(doc) { para.content = "new" }
      bumps(doc) { para.child.remove }
    end

    it "grows for edits inside a detached subtree and a fragment" do
      detached = doc.create_element("q")
      bumps(doc) { detached << doc.create_element("r") }
      frag = doc.fragment("<a/>")
      bumps(doc) { frag.add_child(doc.create_element("b")) }
    end

    it "grows for the source and the target of a move between documents" do
      other = Makiri::XML("<o><s/></o>")
      bumps(doc) { bumps(other) { root.add_child(other.root.at_xpath("s")) } }
    end

    it "is left alone by character-data edits" do
      para << doc.create_comment("c")
      para << doc.create_cdata("x")
      para << doc.create_processing_instruction("t", "d")
      text, comment, cdata, pi = para.children.to_a
      keeps(doc) do
        text.content = "text data"
        comment.content = "c2"
        cdata.content = "y"
        pi.content = "d2"
      end
      expect(para.children.map(&:content)).to eq(["text data", "c2", "y", "d2"])
    end

    it "is left alone by attribute edits" do
      keeps(doc) do
        para["b"] = "2"
        para.set_attribute_ns("urn:k", "k:c", "3")
        para.remove_attribute_ns("urn:k", "c")
        para.delete("b")
      end
    end
  end

  # The cache's promise: while the version stands still, every child list is as
  # it was. Random edits - structural and attribute, on attached and detached
  # nodes, within and between documents - and after each one, any list that
  # changed must have come with a new version.
  describe "an unchanged version means unchanged child lists" do
    def snapshot(nodes)
      nodes.to_h { |n| [n, n.children.to_a] }
    end

    {
      "HTML" => -> { Makiri::HTML("<div><p>a</p><p>b</p><span>c</span></div>") },
      "XML" => -> { Makiri::XML("<r><p>a</p><p>b</p><s>c</s></r>") },
    }.each do |kind, make|
      it "holds over random edits (#{kind})" do
        rng = Random.new(20_261_003)
        docs = [make.call, make.call]
        pool = docs.flat_map { |d| [d.root, *d.root.children.to_a] }
        tracked = pool.dup

        400.times do
          d = docs.sample(random: rng)
          host = (pool.select { |n| n.document.equal?(d) && n.element? } + [d.root]).sample(random: rng)
          other = pool.sample(random: rng)
          before_versions = docs.map(&:tree_version)
          before_lists = snapshot(tracked)

          begin
            case rng.rand(9)
            when 0 then host.add_child(d.create_element("n"))
            when 1 then host.add_child(other) unless other.equal?(host)
            when 2 then other.remove if other.parent
            when 3 then host.before(d.create_text_node("t")) if host.parent
            when 4 then host.content = "c#{rng.rand(9)}"
            when 5 then host["a#{rng.rand(3)}"] = "v"
            when 6 then host.delete("a#{rng.rand(3)}")
            when 7 then host.add_child(d.fragment("<f>1</f><f>2</f>"))
            when 8 then other.replace(d.create_element("r")) if other.parent && !other.equal?(host)
            end
          rescue Makiri::Error, ArgumentError
            # A refused edit (a cycle, a node that cannot take children) is part
            # of the sweep: it must leave the lists or bump the version too.
          end

          pool.concat(host.children.to_a.select(&:element?)).uniq!
          pool.shift while pool.size > 40
          after_lists = snapshot(tracked)
          tracked.each do |n|
            next if before_lists[n] == after_lists[n]

            i = docs.index { |doc| doc.equal?(n.document) }
            expect(docs[i].tree_version).to be > before_versions[i], "#{n.name}'s children changed without a version bump"
          end
          tracked = (tracked + pool).uniq.last(60)
        end
      end
    end
  end
end
