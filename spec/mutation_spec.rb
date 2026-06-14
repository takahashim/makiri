# frozen_string_literal: true

# v0.2: DOM mutation - attribute set/delete, node creation, tree insertion /
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

    describe "#set_attribute_ns" do
      it "sets a namespaced attribute, reusing the slot on (ns, local) match" do
        div.set_attribute_ns("urn:x", "x:foo", "1")
        expect(div.to_html).to include('x:foo="1"')
        div.set_attribute_ns("urn:x", "x:foo", "2")           # same ns+local -> replace
        expect(div.to_html.scan("x:foo").length).to eq(1)     # not duplicated
        expect(div.to_html).to include('x:foo="2"')
      end

      it "treats nil/empty namespace as the null namespace" do
        div.set_attribute_ns(nil, "bare", "b")
        expect(div["bare"]).to eq("b")
      end

      it "keeps same-local attributes in different namespaces distinct" do
        div.set_attribute_ns("urn:a", "a:k", "1")
        div.set_attribute_ns("urn:b", "b:k", "2")
        expect(div.to_html).to include('a:k="1"').and include('b:k="2"')
      end

      it "fails closed on a non-element node and on invalid bytes" do
        text = div.at_css("p").child
        expect { text.set_attribute_ns("urn:x", "x:y", "v") }.to raise_error(Makiri::Error)
        expect { div.set_attribute_ns("urn:x", "x:y", "v\x00") }.to raise_error(Makiri::Error)
        expect { div.set_attribute_ns("urn\x00", "x:y", "v") }.to raise_error(Makiri::Error)
      end
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

    describe "#create_processing_instruction (DOM createProcessingInstruction)" do
      it "creates a PI with a target and data, bound to the document" do
        pi = doc.create_processing_instruction("xml-stylesheet", 'href="s.css"')
        expect(pi).to be_a(Makiri::ProcessingInstruction)
        expect(pi.target).to eq("xml-stylesheet")
        expect(pi.content).to eq('href="s.css"')
        expect(pi.document).to equal(doc)
        expect(pi.parent).to be_nil
      end

      it "is insertable and serializes in the tree" do
        pi = doc.create_processing_instruction("php", "echo 1;")
        el = doc.at_css("div") || doc.at_css("body")
        el.add_child(pi)
        expect(el.to_html).to include("<?php echo 1;>")
      end

      it "fails closed when the data contains the PI terminator '?>'" do
        expect { doc.create_processing_instruction("t", "a?>b") }
          .to raise_error(Makiri::Error)
      end

      it "rejects an embedded NUL or invalid UTF-8 in target or data" do
        expect { doc.create_processing_instruction("t\x00", "d") }
          .to raise_error(Makiri::Error)
        expect { doc.create_processing_instruction("t", "d\xFF".b) }
          .to raise_error(Makiri::Error)
      end
    end

    describe "#create_document_fragment (DOM createDocumentFragment)" do
      it "creates an empty fragment bound to the document" do
        fr = doc.create_document_fragment
        expect(fr).to be_a(Makiri::DocumentFragment)
        expect(fr.children).to be_empty
        expect(fr.document).to equal(doc)
      end

      it "can be filled and spliced (its children) into the tree" do
        fr = doc.create_document_fragment
        fr << doc.create_element("span") << doc.create_element("b")
        expect(fr.to_html).to eq("<span></span><b></b>")
        el = doc.at_css("div") || doc.at_css("body")
        el.add_child(fr)
        expect(el.element_children.map(&:name)).to include("span", "b")
      end
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

    it "fails closed adding a sibling to / replacing a parentless node" do
      p = div.at_css("p")
      p.remove # now detached: no parent
      expect { p.add_previous_sibling(doc.create_element("hr")) }
        .to raise_error(Makiri::Error, /no parent/)
      expect { p.add_next_sibling(doc.create_element("hr")) }
        .to raise_error(Makiri::Error, /no parent/)
      expect { p.replace(doc.create_element("hr")) }
        .to raise_error(Makiri::Error, /no parent/)
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

    it "preserves nested <template> contents" do
      div.inner_html = "<template><b>x</b><template><i>y</i></template></template>"
      outer = div.at_css("template")
      inner = outer.content_fragment.at_css("template")
      expect(outer.content_fragment.at_css("b").text).to eq("x")
      expect(inner.content_fragment.at_css("i").text).to eq("y")
    end

    it "handles a deeply nested fragment without overflowing the stack" do
      # The template-content fixup walks the import iteratively (not recursively
      # on DOM depth), so a very deep fragment must not crash.
      n = 40_000
      div.inner_html = ("<div>" * n) + "<template><span>ok</span></template>" + ("</div>" * n)
      expect(div.at_css("template").content_fragment.at_css("span").text).to eq("ok")
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

    it "invalidates the element-by-tag index when #name= renames an element" do
      multi = Makiri::HTML("<html><body><div>x</div><div>y</div></body></html>")
      multi.xpath("//div")           # build the persisted tag index
      multi.at_xpath("//div").name = "section"
      # the //newtag fast path is served from the tag index; without invalidation
      # the renamed element would be missing from it (a truncated wrong result).
      expect(multi.xpath("//section").map(&:text)).to eq(["x"])
      expect(multi.xpath("//div").map(&:text)).to eq(["y"])
    end
  end

  describe "memory safety", :gc_compact do
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
