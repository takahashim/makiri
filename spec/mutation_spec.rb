# frozen_string_literal: true

# v0.2: DOM mutation — attribute set/delete, node creation, tree insertion /
# removal / replacement, and inner_html= / outer_html=.
RSpec.describe "Makiri mutation" do
  let(:doc) do
    Makiri::HTML("<html><body><div id=\"d\"><p>one</p></div></body></html>")
  end
  let(:div) { doc.at_css("#d") }

  describe "attributes" do
    it "sets attributes with []=" do
      div["class"] = "c"
      expect(div["class"]).to eq("c")
      div["data-x"] = "1"
      expect(div.keys).to eq(%w[id class data-x])
    end

    it "coerces non-string values" do
      div["data-n"] = 42
      expect(div["data-n"]).to eq("42")
    end

    it "deletes attributes" do
      div["class"] = "c"
      div.delete("class")
      expect(div.key?("class")).to be(false)
      expect(div.delete("absent")).to eq(div) # no-op, returns self
    end

    it "raises when setting an attribute on a non-element" do
      text = div.at_css("p").child
      expect { text["x"] = "y" }.to raise_error(Makiri::Error)
    end
  end

  describe "node creation" do
    it "creates elements and text nodes bound to the document" do
      el = doc.create_element("section")
      expect(el).to be_a(Makiri::Element)
      expect(el.name).to eq("section")
      txt = doc.create_text_node("hi")
      expect(txt).to be_a(Makiri::Text)
      expect(txt.text).to eq("hi")
    end
  end

  describe "tree insertion" do
    it "appends with add_child and is chainable with <<" do
      p2 = doc.create_element("p")
      p2 << doc.create_text_node("two")
      expect(div.add_child(p2)).to equal(p2)
      expect(div.css("p").map(&:text)).to eq(%w[one two])
    end

    it "inserts siblings before and after" do
      first = div.at_css("p")
      first.add_previous_sibling(doc.create_element("hr"))
      b = doc.create_element("b")
      b << doc.create_text_node("X")
      first.add_next_sibling(b)
      expect(div.inner_html).to eq("<hr><p>one</p><b>X</b>")
    end

    it "moves an already-attached node instead of duplicating it" do
      p = div.at_css("p")
      body = doc.at_css("body")
      body.add_child(p) # move out of div
      expect(div.css("p").length).to eq(0)
      expect(body.element_children.map(&:name)).to include("p")
    end
  end

  describe "removal and replacement" do
    it "detaches with remove / unlink" do
      p = div.at_css("p")
      p.remove
      expect(div.inner_html).to eq("")
      # detached node is still usable
      expect(p.text).to eq("one")
    end

    it "replaces a node" do
      p = div.at_css("p")
      span = doc.create_element("span")
      span << doc.create_text_node("S")
      expect(p.replace(span)).to equal(span)
      expect(div.inner_html).to eq("<span>S</span>")
    end
  end

  describe "safety" do
    it "rejects creating a cycle" do
      body = doc.at_css("body")
      expect { div.add_child(body) }.to raise_error(Makiri::Error)
    end

    it "rejects moving a node across documents" do
      other = Makiri::HTML("<p>x</p>")
      foreign = other.at_css("p")
      expect { div.add_child(foreign) }.to raise_error(Makiri::Error)
    end

    it "rejects inserting an attribute node into the tree" do
      attr = doc.at_css("#d").attribute_nodes.first
      expect { div.add_child(attr) }.to raise_error(Makiri::Error)
    end
  end

  describe "inner_html= / outer_html=" do
    it "replaces an element's children" do
      div.inner_html = '<a href="/y">link</a><b>bold</b>'
      expect(div.inner_html).to eq('<a href="/y">link</a><b>bold</b>')
      expect(div.css("a").first["href"]).to eq("/y")
    end

    it "clears children with an empty string" do
      div.inner_html = ""
      expect(div.children.length).to eq(0)
    end

    it "replaces the node itself with outer_html=" do
      doc.at_css("p").outer_html = "<em>E</em><i>I</i>"
      expect(div.inner_html).to eq("<em>E</em><i>I</i>")
    end

    it "preserves <template> contents through fragment import" do
      # import_node does not copy a template's separate content fragment; the
      # mutation path must fix it up (as the document fragment parser does).
      div.inner_html = "<template><span>ok</span></template>"
      tpl = div.at_css("template")
      expect(tpl.content_fragment.at_css("span")&.text).to eq("ok")
    end
  end

  describe "query consistency after mutation" do
    it "rebuilds the attribute index so XPath sees the new tree" do
      div.inner_html = '<a id="link" href="/z">L</a>'
      expect(doc.xpath("//a/@href").map(&:value)).to eq(["/z"])
      expect(doc.at_xpath("//@id[. = 'link']/parent::*").name).to eq("a")
    end

    it "reflects attribute changes in subsequent queries" do
      div["class"] = "fresh"
      expect(doc.css(".fresh").length).to eq(1)
      div.delete("class")
      expect(doc.css(".fresh").length).to eq(0)
    end
  end

  describe "memory safety" do
    it "survives mutation churn under GC stress" do
      GC.stress = true
      begin
        10.times do |i|
          el = doc.create_element("p")
          el << doc.create_text_node("n#{i}")
          div.add_child(el)
        end
        GC.compact
        expect(div.css("p").length).to eq(11) # original + 10
        div.inner_html = "<p>reset</p>"
        expect(div.css("p").map(&:text)).to eq(%w[reset])
      ensure
        GC.stress = false
      end
    end
  end
end
