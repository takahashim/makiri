# frozen_string_literal: true

require "spec_helper"

# The WHATWG DOM's "ensure pre-insertion validity", which both representations
# now take from one place (`crate::dom_rules`). Each example here is a rule one
# of the two hand-written copies it replaced got wrong.
RSpec.describe "pre-insertion validity" do
  describe "HTML" do
    let(:doc) { Makiri::HTML("<!DOCTYPE html><body><template><p>x</p></template><b>b</b></body>") }
    let(:template) { doc.at_css("template") }

    # A template's contents fragment has no parent: it reaches its template
    # through the host link, so an own-subtree check that walked parents only
    # let the template go into its own contents - a cycle every later deep
    # walk (`dup`, `import_node`) looped on with the GVL held.
    it "refuses a template going into its own contents" do
      content = template.content_fragment
      expect { content.add_child(template) }
        .to raise_error(Makiri::Error, /own subtree/)
      expect { content.at_css("p").add_child(template) }
        .to raise_error(Makiri::Error, /own subtree/)
      expect { content.at_css("p").add_previous_sibling(template) }
        .to raise_error(Makiri::Error, /own subtree/)
      expect(template.parent.name).to eq("body")
      expect(content.children.map(&:name)).to eq(%w[p])
    end

    it "keeps a refused template copyable and serializable" do
      expect { template.content_fragment.add_child(template) }.to raise_error(Makiri::Error)
      expect(template.dup.inner_html).to eq("<p>x</p>")
      expect(Makiri::XML::Document.new.import_node(template, true).name).to eq("template")
      expect(doc.to_html).to include("<template><p>x</p></template>")
    end

    it "still lets a template's contents take other nodes" do
      template.content_fragment.add_child(doc.at_css("b"))
      expect(template.inner_html).to eq("<p>x</p><b>b</b>")
    end

    # Only a Document, a DocumentFragment or an Element has children.
    {
      "a text node" => ->(d) { d.create_text_node("t") },
      "a comment" => ->(d) { d.create_comment("c") },
      "a processing instruction" => ->(d) { d.create_processing_instruction("pi", "data") },
      "an attribute" => lambda { |d|
        d.at_css("b")["id"] = "i"
        d.at_css("b").attribute_nodes.first
      }
    }.each do |what, make|
      it "refuses a child under #{what}" do
        parent = make.call(doc)
        el = doc.create_element("i")
        expect { parent.add_child(el) }
          .to raise_error(Makiri::Error, /only a document, a document fragment or an element/)
        expect(el.parent).to be_nil
        expect(parent.children.to_a).to be_empty
      end
    end

    it "refuses a child under the doctype, which would put it beside <html>" do
      doctype = doc.children.first
      expect(doctype).to be_a(Makiri::HTML::DocumentType)
      b = doc.at_css("b")
      expect { doctype.add_child(b) }
        .to raise_error(Makiri::Error, /only a document, a document fragment or an element/)
      expect(b.parent.name).to eq("body")
      expect(doc.children.map(&:name)).to eq(%w[html html])
    end

    it "gives an attribute no parent to place a sibling beside" do
      b = doc.at_css("b")
      b["id"] = "x"
      attr = b.attribute_nodes.first
      el = doc.create_element("i")
      expect { attr.add_previous_sibling(el) }.to raise_error(Makiri::Error, /no parent/)
      expect { attr.replace(el) }.to raise_error(Makiri::Error, /no parent/)
      expect(b.to_html).to eq(%(<b id="x">b</b>))
    end
  end

  describe "XML" do
    it "refuses text under the Document, so the document still reparses" do
      doc = Makiri::XML("<r/>")
      [doc.create_text_node("hello"), doc.create_cdata("c")].each do |text|
        expect { doc.add_child(text) }
          .to raise_error(Makiri::Error, /text cannot be a child of the document/)
        expect { doc.root.add_previous_sibling(text) }
          .to raise_error(Makiri::Error, /text cannot be a child of the document/)
      end
      frag = doc.fragment("<!--c-->text")
      expect { doc.add_child(frag) }.to raise_error(Makiri::Error, /text cannot be a child/)
      expect(frag.children.size).to eq(2)
      expect(Makiri::XML(doc.to_xml).root.name).to eq("r")
      expect(doc.children.map(&:name)).to eq(%w[r])
    end

    it "still takes a comment or PI at document level" do
      doc = Makiri::XML("<r/>")
      doc.add_child(doc.create_comment("c"))
      expect(Makiri::XML(doc.to_xml).children.map(&:name)).to eq(%w[r comment])
    end

    it "refuses a child under a node that cannot have one" do
      doc = Makiri::XML("<r>t<!--c--></r>")
      doc.root.children.each do |leaf|
        expect { leaf.add_child(doc.create_element("x")) }.to raise_error(Makiri::Error)
      end
      expect(doc.root.to_xml).to eq("<r>t<!--c--></r>")
    end

    it "gives an attribute no parent to place a sibling beside" do
      doc = Makiri::XML(%(<r a="1"><c/></r>))
      attr = doc.root.attribute_nodes.first
      expect { attr.add_previous_sibling(doc.create_element("x")) }.to raise_error(Makiri::Error)
      expect { attr.add_next_sibling(doc.create_element("x")) }.to raise_error(Makiri::Error)
      expect(doc.root.to_xml).to eq(%(<r a="1"><c/></r>))
    end

    describe "a DocumentFragment as the parent" do
      let(:doc) { Makiri::XML("<r/>") }
      let(:frag) { doc.fragment("") }

      it "takes children through every verb, as the DOM and Nokogiri allow" do
        a = frag.add_child(doc.create_element("a"))
        frag << doc.create_text_node("t")
        a.add_previous_sibling(doc.create_comment("c"))
        a.add_next_sibling(doc.create_element("b"))
        frag.children.last.replace(doc.create_element("z"))
        expect(frag.children.map(&:name)).to eq(%w[comment a b z])
        expect(frag.to_xml).to eq("<!--c--><a/><b/><z/>")
        doc.root.add_child(frag)
        expect(doc.root.to_xml).to eq("<r><!--c--><a/><b/><z/></r>")
        expect(frag.children.to_a).to be_empty
      end

      it "is what a deep import of an HTML fragment builds" do
        html = Makiri::HTML("<p/>").fragment("<i>f</i><b>g</b>")
        copy = doc.import_node(html, true)
        expect(copy).to be_a(Makiri::XML::DocumentFragment)
        expect(copy.children.map(&:name)).to eq(%w[i b])
      end

      it "splices another fragment's children into it" do
        frag.add_child(doc.fragment("<x/><y/>"))
        expect(frag.children.map(&:name)).to eq(%w[x y])
      end

      it "refuses a doctype, and a fragment going into its own child" do
        frag.add_child(doc.create_element("a"))
        expect { frag.add_child(doc.create_document_type("r")) }
          .to raise_error(Makiri::Error, /invalid placement/)
        expect { frag.children.first.add_child(frag) }.to raise_error(Makiri::Error, /own subtree/)
        expect { frag.add_child(frag) }.to raise_error(Makiri::Error, /own subtree/)
        expect(frag.children.map(&:name)).to eq(%w[a])
      end

      it "defers an unbound prefix and resolves it when spliced where it is bound" do
        doc = Makiri::XML(%(<r xmlns:p="urn:p"/>))
        frag = doc.fragment("")
        el = doc.create_element("p:x")
        frag.add_child(el)
        expect(el.parent).to equal(frag)
        doc.root.add_child(frag)
        expect(el.parent).to equal(doc.root)
        expect(el.namespace_uri).to eq("urn:p")
        expect(doc.root.to_xml).to eq(%(<r xmlns:p="urn:p"><p:x/></r>))
      end

      it "refuses the splice where the prefix is unbound, and leaves every child undecided" do
        doc = Makiri::XML(%(<r xmlns:p="urn:p"/>))
        frag = doc.fragment("")
        bound = frag.add_child(doc.create_element("p:a"))
        unbound = frag.add_child(doc.create_element("q:b"))
        expect { doc.root.add_child(frag) }.to raise_error(Makiri::Error, /not bound/)
        expect(frag.children.to_a).to eq([bound, unbound])
        expect(doc.root.children.to_a).to be_empty

        # `p:a` was not decided against the context it never joined: it
        # resolves afresh where `p` means something else.
        holder = doc.create_element("h")
        holder["xmlns:p"] = "urn:other"
        holder["xmlns:q"] = "urn:q"
        doc.root.add_child(holder)
        holder.add_child(frag)
        expect(bound.namespace_uri).to eq("urn:other")
        expect(unbound.namespace_uri).to eq("urn:q")
      end
    end
  end
end
