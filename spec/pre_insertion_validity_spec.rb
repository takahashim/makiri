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
end
