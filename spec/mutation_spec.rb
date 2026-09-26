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
        # The VALUE is data-family, so an embedded NUL is now permitted (DOM
        # conformance); the qualified name and namespace stay NUL-strict.
        div.set_attribute_ns("urn:x", "x:y", "v\x00")
        expect(div.attribute_nodes.find { |a| a.name == "x:y" }.value.bytesize).to eq(2)
        expect { div.set_attribute_ns("urn:x", "x\x00:y", "v") }.to raise_error(Makiri::Error)
        expect { div.set_attribute_ns("urn\x00", "x:y", "v") }.to raise_error(Makiri::Error)
      end
    end

    describe "#remove_attribute_ns" do
      it "removes only the attribute in that namespace" do
        div["k"] = "plain"
        div.set_attribute_ns("urn:a", "a:k", "ns")
        div.remove_attribute_ns("urn:a", "k")
        expect(div["k"]).to eq("plain")
        expect(div.to_html).not_to include("a:k")
      end

      it "leaves the null-namespace attribute alone for a namespace never used" do
        div["k"] = "plain"
        div.remove_attribute_ns("urn:never-interned", "k")
        expect(div["k"]).to eq("plain")
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
        expect(el.to_html).to include("<?php echo 1;?>")
      end

      it "fails closed when the data contains the PI terminator '?>'" do
        expect { doc.create_processing_instruction("t", "a?>b") }
          .to raise_error(Makiri::Error)
      end

      # DOM: the target must match the XML Name production. Lexbor does not
      # check it, and an unchecked target was serialized as it stood - a live
      # <script> included.
      it "refuses a target that is not an XML Name" do
        ["x?><script>alert(1)</script><?y", "a b", "", "1x"].each do |target|
          expect { doc.create_processing_instruction(target, "d") }
            .to raise_error(ArgumentError, /processing instruction target/)
        end
        expect(doc.create_processing_instruction("xml-stylesheet", "d").target).to eq("xml-stylesheet")
      end

      it "rejects an embedded NUL or invalid UTF-8 in target or data" do
        expect { doc.create_processing_instruction("t\x00", "d") }
          .to raise_error(Makiri::Error)
        expect { doc.create_processing_instruction("t", "d\xFF".b) }
          .to raise_error(Makiri::Error)
      end
    end

    describe "#create_document_type (DOM createDocumentType)" do
      it "creates a detached DocumentType with name / public / system ids" do
        dt = doc.create_document_type("html", "-//W3C//DTD HTML 4.01//EN", "http://x/s.dtd")
        expect(dt).to be_a(Makiri::DocumentType)
        expect(dt.name).to eq("html")
        expect(dt.public_id).to eq("-//W3C//DTD HTML 4.01//EN")
        expect(dt.system_id).to eq("http://x/s.dtd")
        expect(dt.parent).to be_nil
      end

      it "preserves the name's case (DOM createDocumentType is case-preserving)" do
        # Unlike the HTML parser (which lowercases the DOCTYPE name per HTML5),
        # the programmatic DOM factory keeps case, incl. through serialization.
        dt = doc.create_document_type("SVG")
        expect(dt.name).to eq("SVG")
        expect(doc.create_document_type("ns:MixedCase.").name).to eq("ns:MixedCase.")
        doc.root.add_previous_sibling(dt)
        expect(doc.to_html).to start_with("<!DOCTYPE SVG>")
        expect(Makiri::HTML("<html></html>").import_node(dt, true).name).to eq("SVG")
      end

      it "treats an omitted or empty public / system id as absent" do
        dt = doc.create_document_type("html")
        expect(dt.public_id).to be_nil
        expect(dt.system_id).to be_nil
        expect(doc.create_document_type("html", "").public_id).to be_nil
      end

      it "fails closed on an invalid or non-UTF-8 name" do
        expect { doc.create_document_type("1 bad>") }.to raise_error(ArgumentError)
        expect { doc.create_document_type("") }.to raise_error(ArgumentError)
        expect { doc.create_document_type("h\xFF".b) }.to raise_error(Makiri::Error)
      end

      it "is placed before the document element" do
        dt = doc.create_document_type("html")
        doc.root.add_previous_sibling(dt)
        expect(doc.children.map(&:class)).to eq([Makiri::HTML::DocumentType, Makiri::HTML::Element])
        expect(dt.parent).to equal(doc)
        expect(doc.to_html).to start_with("<!DOCTYPE html>")
      end

      describe "placement guards (fail-closed, WHATWG doctype rules)" do
        it "rejects a second doctype in the document" do
          doc.root.add_previous_sibling(doc.create_document_type("a"))
          expect { doc.root.add_previous_sibling(doc.create_document_type("b")) }
            .to raise_error(Makiri::Error, /already has a doctype/)
        end

        it "rejects a doctype that would follow the document element" do
          expect { doc.add_child(doc.create_document_type("html")) }
            .to raise_error(Makiri::Error, /precede the document element/)
          expect { doc.root.add_next_sibling(doc.create_document_type("html")) }
            .to raise_error(Makiri::Error, /precede the document element/)
        end

        it "rejects a doctype under a non-document parent" do
          expect { div.add_child(doc.create_document_type("html")) }
            .to raise_error(Makiri::Error, /child of the document/)
        end

        it "rejects inserting an element (or an element-bearing fragment) ahead of an existing doctype" do
          d = Makiri::HTML("<!DOCTYPE html><html></html>")
          dt = d.children.first
          expect { dt.add_previous_sibling(d.create_element("x")) }
            .to raise_error(Makiri::Error, /precede the document element/)
          expect { dt.before(d.fragment("<y></y>")) }
            .to raise_error(Makiri::Error, /precede the document element/)
          # a comment before the doctype stays legal (prolog content)
          dt.add_previous_sibling(d.create_comment("ok"))
          expect(d.children.map(&:class))
            .to eq([Makiri::HTML::Comment, Makiri::HTML::DocumentType, Makiri::HTML::Element])
        end

        # A document has one element child and no text child. Lexbor enforces
        # neither, so `<<` made a second root.
        it "rejects a second root element, text under the document, and a two-element fragment" do
          d = Makiri::HTML("<p>x</p>")
          expect { d << d.create_element("y") }.to raise_error(Makiri::Error, /already has a root element/)
          expect { d << d.create_text_node("t") }.to raise_error(Makiri::Error, /text cannot be a child/)
          expect { d << d.fragment("<a></a><b></b>") }.to raise_error(Makiri::Error, /already has a root element/)
          expect(d.xpath("count(/*)")).to eq(1)
          d << d.create_comment("fine")
          d.root.replace(d.create_element("html")) # replacing the root is fine
          expect(d.xpath("count(/*)")).to eq(1)
        end

        it "rejects a second doctype even when another node precedes the first" do
          # The duplicate check must scan the whole child list: stopping at the
          # insertion point would let the leading comment hide the doctype.
          d = Makiri::HTML("<!--c--><!DOCTYPE html><html><body>b</body></html>")
          expect { d.children.first.before(d.create_document_type("x")) }
            .to raise_error(Makiri::Error, /already has a doctype/)
          expect { d.children.first.replace(d.create_document_type("x")) }
            .to raise_error(Makiri::Error, /already has a doctype/)
          expect(d.children.map(&:class))
            .to eq([Makiri::HTML::Comment, Makiri::HTML::DocumentType, Makiri::HTML::Element])
        end

        it "leaves the tree and the rejected node unchanged after a refused insert" do
          dt = doc.create_document_type("html")
          before = doc.children.map(&:class)
          expect { doc.add_child(dt) }.to raise_error(Makiri::Error)
          expect(doc.children.map(&:class)).to eq(before)
          expect(dt.parent).to be_nil
        end

        it "allows replacing the document element itself with a doctype" do
          doc.root.replace(doc.create_document_type("html"))
          expect(doc.children.map(&:class)).to eq([Makiri::HTML::DocumentType])
        end
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

    # The DOM adopts a node from another document rather than refusing it, and
    # so do Chrome and Nokogiri. Lexbor cannot relink a node across arenas, so
    # the node is copied here and removed there - a move from the outside,
    # except that the node handed back is a different object.
    it "adopts a node from another document" do
      other = Makiri::HTML("<p id='foreign'>x</p>")
      foreign = other.at_css("p")
      adopted = div.add_child(foreign)

      expect(adopted.name).to eq("p")
      expect(adopted.document).to equal(doc)
      expect(div.to_html).to include("foreign")
      expect(other.at_css("p")).to be_nil          # gone from the source
    end

    it "rejects inserting an attribute node into the tree" do
      attr = doc.at_css("#d").attribute_nodes.first
      expect { div.add_child(attr) }.to raise_error(Makiri::Error)
    end

    # WHATWG DOM's pre-insertion check: a document is never a child. It was
    # let through, making the document its own element's descendant (or, from
    # another document, splicing a whole copied #document into an element).
    it "rejects inserting a document, its own or another" do
      el = doc.create_element("div")
      expect { el.add_child(doc) }.to raise_error(Makiri::Error, /document node/)
      expect(doc.parent).to be_nil

      other = Makiri::HTML("<p>o</p>")
      expect { div.add_child(other) }.to raise_error(Makiri::Error, /document node/)
      expect { div.add_next_sibling(other) }.to raise_error(Makiri::Error, /document node/)
      expect(div.to_html).not_to include("#document")
    end
  end

  describe "#content=" do
    it "clears an attached element without leaving an empty Text node" do
      document = Makiri::HTML('<main><section><p id="old">old</p><span>tail</span></section></main>')
      target = document.at_css("section")
      parent = target.parent
      children = target.children.to_a
      before = children.map(&:text)

      expect(target.text).to eq("oldtail")
      expect(document.xpath("//p").length).to eq(1)
      expect(document.xpath("//@id").map(&:value)).to eq(["old"])

      target.content = ""

      expect(target.children).to be_empty
      expect(target.xpath("count(text())")).to eq(0.0)
      expect(target.parent).to equal(parent)
      expect(children.map(&:text)).to eq(before)
      expect(children.map(&:parent)).to eq([nil, nil])
      expect(document.xpath("//p")).to be_empty
      expect(document.xpath("//@id")).to be_empty
      expect(document.css("#old")).to be_empty
    end

    it "clears a detached element and keeps its old children readable" do
      document = Makiri::HTML("<main></main>")
      target = document.create_element("section")
      child = document.create_element("p")
      child.content = "old"
      target << child

      target.content = ""

      expect(target.parent).to be_nil
      expect(target.children).to be_empty
      expect(target.xpath("count(text())")).to eq(0.0)
      expect(child.parent).to be_nil
      expect(child.text).to eq("old")
    end

    it "clears document-bound and standalone fragments without empty Text nodes" do
      document = Makiri::HTML("<main></main>")
      [document.fragment("<p>one</p><b>two</b>"),
       Makiri::DocumentFragment.parse("<p>one</p><b>two</b>")].each do |fragment|
        children = fragment.children.to_a

        fragment.content = ""

        expect(fragment.parent).to be_nil
        expect(fragment.children).to be_empty
        expect(fragment.xpath("count(text())")).to eq(0.0)
        expect(children.map(&:parent)).to eq([nil, nil])
        expect(children.map(&:text)).to eq(%w[one two])
      end
    end

    it "clears a template content fragment without leaving an empty Text node" do
      document = Makiri::HTML("<template><p>old</p><b>tail</b></template>")
      fragment = document.at_css("template").content_fragment
      children = fragment.children.to_a

      fragment.content = ""

      expect(fragment.children).to be_empty
      expect(fragment.xpath("count(text())")).to eq(0.0)
      expect(children.map(&:parent)).to eq([nil, nil])
      expect(children.map(&:text)).to eq(%w[old tail])
    end

    it "keeps a leaf Text attached when its content becomes empty" do
      text = doc.create_text_node("old")
      div << text

      text.content = ""

      expect(text.parent).to equal(div)
      expect(text.content).to eq("")
      expect(div.children.last).to equal(text)
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
      # on DOM depth), so a very deep fragment must not crash. `inner_html=`
      # parses under the default tree-depth limit, so the depth arrives through
      # `Document#fragment`, which runs the same import.
      n = 40_000
      html = ("<div>" * n) + "<template><span>ok</span></template>" + ("</div>" * n)
      div.add_child(div.document.fragment(html, max_tree_depth: -1))
      expect(div.at_css("template").content_fragment.at_css("span").text).to eq("ok")
      expect { div.inner_html = html }.to raise_error(Makiri::Error, /tree depth limit/)
    end
  end

  describe "query consistency after mutation" do
    # Adopting takes the node out of the document it came from, and that
    # document's text and element indexes still list it - they have to be
    # dropped too, or its #text and //tag go on answering with the node.
    %i[add_child add_previous_sibling add_next_sibling replace].each do |verb|
      it "drops the SOURCE document's indexes when #{verb} adopts a node" do
        src = Makiri::HTML("<div><p>moved</p><span>stay</span></div>")
        dst = Makiri::HTML("<section><b>anchor</b></section>")
        expect([src.text, src.xpath("//p").size]).to eq(["movedstay", 1]) # warm both indexes
        dst.at_css("b").public_send(verb, src.at_css("p"))
        expect(src.text).to eq("stay")
        expect(src.xpath("//p")).to be_empty
        expect(src.at_xpath("//p")).to be_nil
        expect(dst.xpath("//p").size).to eq(1)
      end
    end

    it "drops the source document's indexes when a whole subtree is adopted" do
      src = Makiri::HTML("<div><i>x</i></div><p>rest</p>")
      dst = Makiri::HTML("<section></section>")
      expect([src.xpath("//i").size, src.text]).to eq([1, "xrest"])
      dst.at_css("section").add_child(src.at_css("div"))
      expect(src.xpath("//i")).to be_empty
      expect(src.text).to eq("rest")
    end

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

    # Lexbor's `lxb_dom_document_root` answers with the document's FIRST CHILD
    # when the document has no `<html>` - so a comment or processing instruction
    # put in front of the root element becomes what the text index is asked to
    # build over. It is rooted at a container, and a leaf one used to reach into
    # a range table sized for no containers at all.
    it "reads text after a leaf node is placed before the root element" do
      built = Makiri::HTML("")
      built.children.to_a.each(&:unlink)
      root = built.create_element("p")
      built.add_child(root)
      leaf = built.create_element("span")
      root.add_child(leaf)
      root.add_child(built.create_text_node("t5"))

      [built.create_processing_instruction("pi", "d"), built.create_comment("c")].each do |node|
        root.add_previous_sibling(node)

        expect(root.text).to eq("t5")
        expect(leaf.text).to eq("")
        node.unlink
      end
      # Text is no child of a document (DOM pre-insertion validity), so that
      # leaf is refused before it can reach the index.
      expect { root.add_previous_sibling(built.create_text_node("x")) }
        .to raise_error(Makiri::Error, /text cannot be a child of the document/)
      expect(root.text).to eq("t5")
    end

  end

  # Data-family mutations (text/comment node content, attribute values) accept an
  # embedded NUL (U+0000) so the HTML DOM API can hold it, matching browsers /
  # WHATWG DOM (createTextNode/setAttribute must not reject U+0000). The bytes are
  # stored and read back verbatim (length-preserved, never truncated at the NUL).
  # Names, tags, namespaces, PI target/data, selectors, and XPath stay NUL-strict;
  # invalid UTF-8 is still rejected everywhere, including these relaxed sites.
  describe "embedded NUL in data-family content (DOM conformance)" do
    let(:nul) { "a\x00b" } # 3 bytes

    it "allows a NUL in an attribute value and round-trips it verbatim" do
      div["data-x"] = nul
      expect(div["data-x"].bytesize).to eq(3)
      expect(div["data-x"]).to eq(nul)
    end

    it "allows a NUL in create_text_node and reads it back via #text" do
      t = doc.create_text_node(nul)
      expect(t.text.bytesize).to eq(3)
      expect(t.text).to eq(nul)
    end

    it "allows a NUL in create_comment and preserves it in serialization" do
      c = doc.create_comment(nul)
      expect(c.to_html).to eq("<!--a\x00b-->")
    end

    it "allows a NUL via content= on an element (single text child) and on a text node" do
      div.content = nul
      expect(div.text.bytesize).to eq(3)
      t = div.at_css("p")&.child || doc.create_text_node("x")
      t.content = nul
      expect(t.text).to eq(nul)
    end

    it "round-trips a NUL-bearing attribute value through serialization" do
      div["data-x"] = nul
      reparsed = Makiri::HTML(div.to_html).at_css("#d")
      # The HTML tokenizer replaces U+0000 in an attribute value with U+FFFD on
      # re-parse (WHATWG), so this is not byte-identical - assert it stays a valid
      # single-attribute round-trip rather than a specific byte sequence.
      expect(reparsed["data-x"]).not_to be_nil
    end

    it "still rejects a NUL in a NAME / tag (fail closed)" do
      expect { doc.create_element("a\x00b") }
        .to raise_error(Makiri::Error, /must not contain a NUL byte/)
      expect { div["a\x00b"] = "v" }
        .to raise_error(Makiri::Error, /must not contain a NUL byte/)
    end

    it "still rejects invalid UTF-8 in the relaxed data-family sites" do
      expect { doc.create_text_node("\xFF".b) }
        .to raise_error(Makiri::Error, /must be valid UTF-8/)
      expect { div["data-x"] = "\xFF".b }
        .to raise_error(Makiri::Error, /must be valid UTF-8/)
      expect { doc.create_comment("\xFF".b) }
        .to raise_error(Makiri::Error, /must be valid UTF-8/)
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

  # Names were written into the markup unchecked, so a name could carry markup
  # of its own. The WHATWG DOM's name rules now apply (XML had its own).
  describe "element and attribute names" do
    let(:doc) { Makiri::HTML("<p>t</p>") }
    let(:para) { doc.at_css("p") }

    it "refuses names that would become markup" do
      expect { para[%(x="y" onload)] = "v" }.to raise_error(ArgumentError, /attribute name/)
      expect { doc.create_element("a href=javascript:x") }.to raise_error(ArgumentError, /element name/)
      expect { para.set_attribute_ns("urn:x", ":a", "v") }.to raise_error(ArgumentError, /attribute name/)
      expect(para.to_html).to eq("<p>t</p>")
    end

    it "keeps accepting the names HTML uses" do
      para["data-x"] = "1"
      para["@click"] = "go"
      para[":href"] = "u"
      para["v-on:x"] = "1"
      expect(para.attribute_nodes.map(&:name)).to eq(%w[data-x @click :href v-on:x])
      expect(doc.create_element("my-widget").name).to eq("my-widget")
    end

    it "refuses a prefix without a namespace in set_attribute_ns, as the DOM does" do
      expect { para.set_attribute_ns(nil, "x:y", "v") }.to raise_error(Makiri::Error, /does not fit/)
      para.set_attribute_ns("http://www.w3.org/1999/xlink", "xlink:href", "#")
      expect(para["xlink:href"]).to eq("#")
    end
  end
end
