# frozen_string_literal: true

# Node#text / #content is served from a per-document text index
# (html/dom/index.rs): a flat doc-order array of every text/CDATA slice plus a
# node -> [start, end) map, built once the walks Node#text served cover half
# the document, and dropped on any mutation. The
# result must be byte-identical to a plain descendant walk, stay correct across
# mutations (index rebuild), and fall back cleanly where the node is not indexed
# (fragments). These cases also give the index ASan/fuzz coverage.
RSpec.describe "Node#text via the text index" do
  # Reference: concatenate TEXT/CDATA descendant data by an independent walk.
  def ref_text(node)
    out = +""
    stack = node.children.to_a.reverse
    until stack.empty?
      n = stack.pop
      if n.text? || n.is_a?(Makiri::CDATASection)
        out << n.content
      else
        stack.concat(n.children.to_a.reverse)
      end
    end
    out
  end

  let(:html) do
    <<~HTML
      <!doctype html><html><body>
        <div id="a">A<span>B<i>C</i></span>D</div>
        <div id="b">café\nüñ<p>déép<b>!</b></p></div>
        <div id="empty"></div>
      </body></html>
    HTML
  end
  let(:doc) { Makiri::HTML(html) }

  it "matches a descendant walk for the whole document" do
    expect(doc.text).to eq(ref_text(doc.root))
    expect(doc.text.encoding.name).to eq("UTF-8")
  end

  it "matches a descendant walk for arbitrary subtrees" do
    %w[#a #b #empty span i p].each do |sel|
      el = doc.at_css(sel)
      expect(el.text).to eq(ref_text(el)), "mismatch for #{sel}"
      expect(el.text.encoding.name).to eq("UTF-8")
    end
  end

  it "returns an empty UTF-8 string for a text-free element" do
    t = doc.at_css("#empty").text
    expect(t).to eq("")
    expect(t.encoding.name).to eq("UTF-8")
  end

  it "preserves multi-byte UTF-8 content exactly" do
    el = doc.at_css("#b")
    expect(el.text).to include("café", "üñ", "déép", "!")
    expect(el.text.bytesize).to eq(ref_text(el).bytesize)
  end

  it "is stable across repeated calls (index reuse)" do
    el = doc.at_css("#a")
    3.times { expect(el.text).to eq("ABCD") }
  end

  describe "invalidation on mutation (index is rebuilt)" do
    it "reflects add_child" do
      el = doc.at_css("#a")
      el.text # build
      em = doc.create_element("em")
      em.content = "X"
      el.add_child(em)
      expect(el.text).to eq(ref_text(el))
      expect(doc.text).to eq(ref_text(doc.root))
    end

    it "reflects content=" do
      el = doc.at_css("#a")
      el.text
      el.content = "replaced"
      expect(el.text).to eq("replaced")
      expect(doc.text).to eq(ref_text(doc.root))
    end

    it "reflects add_previous_sibling and remove" do
      doc.at_css("#a").text # build
      doc.at_css("span").add_previous_sibling(doc.create_text_node("PRE"))
      expect(doc.at_css("#a").text).to eq(ref_text(doc.at_css("#a")))
      doc.at_css("span").remove
      expect(doc.at_css("#a").text).to eq(ref_text(doc.at_css("#a")))
    end

    it "reflects replace" do
      el = doc.at_css("#a")
      el.text
      doc.at_css("span").replace(doc.create_text_node("Z"))
      expect(el.text).to eq("AZD")
    end

    it "stays correct after an attribute change (shared invalidation hook)" do
      el = doc.at_css("#a")
      before = el.text
      el["class"] = "x"
      expect(el.text).to eq(before)
    end
  end

  describe "fallback for non-indexed nodes" do
    it "extracts text from a standalone DocumentFragment" do
      fr = Makiri::DocumentFragment.parse("<a>x</a><b>y</b>z")
      expect(fr.text).to eq("xyz")
    end

    it "extracts text from a document-bound fragment" do
      fr = doc.fragment("<p>one</p>two")
      expect(fr.text).to eq("onetwo")
    end
  end

  # The first doc.text walks (every node: past the half the index waits for),
  # the second builds the index, so every call after is served from it.
  it "serves every element from the index once built, and again after each edit" do
    d = Makiri::HTML(html)
    check_all = lambda do |round|
      d.text
      d.text
      d.css("*").each { |el| expect(el.text).to eq(ref_text(el)), "#{round}: #{el.name}" }
      expect(d.text).to eq(ref_text(d.root))
    end
    check_all.call("parsed")
    d.at_css("#a span").add_child(d.create_text_node("new"))
    check_all.call("added")
    d.at_css("#b p").remove
    check_all.call("removed")
    d.at_css("#a").inner_html = "x<b>y</b>z"
    check_all.call("inner_html=")
    d.at_css("#a b").content = "Y"
    check_all.call("content=")
  end
end
