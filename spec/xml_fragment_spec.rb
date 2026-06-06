# frozen_string_literal: true

require "spec_helper"

# XML fragment parsing: Makiri::XML::DocumentFragment.parse (a standalone,
# self-contained fragment with its own backing document) and
# Makiri::XML::Document#fragment (bound to a document, inheriting its namespaces).
# A fragment is 0+ top-level nodes with no single-root rule; inserting it splices
# its children, not the fragment node itself.
RSpec.describe "Makiri::XML fragments" do
  describe "DocumentFragment.parse (standalone)" do
    it "parses multiple top-level nodes, including character data" do
      f = Makiri::XML::DocumentFragment.parse(%(<a/>text<b x="1">y</b>))
      expect(f).to be_a(Makiri::XML::DocumentFragment)
      expect(f.name).to eq("#document-fragment")
      expect(f.children.map(&:name)).to eq(%w[a text b])
      expect(f.to_xml).to eq(%(<a/>text<b x="1">y</b>))
    end

    it "is self-contained: a prefix must be declared within the fragment" do
      f = Makiri::XML::DocumentFragment.parse(%(<p:a xmlns:p="urn:p"/>))
      expect(f.children.first.namespace_uri).to eq("urn:p")
      expect { Makiri::XML::DocumentFragment.parse("<p:unbound/>") }
        .to raise_error(Makiri::XML::SyntaxError)
    end

    it "fails closed on document-only constructs and malformed input" do
      ["<a>", "</x>", "<!DOCTYPE r>", "<?xml version='1.0'?>", "<a></b>", "<a]]>"].each do |bad|
        expect { Makiri::XML::DocumentFragment.parse(bad) }
          .to raise_error(Makiri::XML::SyntaxError), "expected #{bad.inspect} to be rejected"
      end
    end

    it "round-trips and canonicalizes as its children" do
      f = Makiri::XML::DocumentFragment.parse("<a><b/></a><!-- c --><d/>")
      expect(f.to_xml).to eq("<a><b/></a><!-- c --><d/>")
      expect(f.canonicalize).to eq("<a><b></b></a><d></d>") # comments off by default
    end

    it "an empty fragment is valid and serializes to empty" do
      f = Makiri::XML::DocumentFragment.parse("")
      expect(f.children).to be_empty
      expect(f.to_xml).to eq("")
    end
  end

  describe "Document#fragment (bound, inherits namespaces)" do
    let(:doc) { Makiri::XML(%(<r xmlns:p="urn:p" xmlns="urn:d"><k/></r>)) }

    it "resolves prefixes and the default namespace against the document" do
      f = doc.fragment("<p:item/><plain/>")
      expect(f.children.map { |c| [c.name, c.namespace_uri] })
        .to eq([["p:item", "urn:p"], ["plain", "urn:d"]])
    end

    it "splices its children when inserted" do
      doc.root.add_child(doc.fragment("<p:a/><p:b/>"))
      expect(doc.root.children.map(&:name)).to eq(%w[k p:a p:b])
      expect(doc.xpath("//p:a", "p" => "urn:p").length).to eq(1)
    end
  end

  describe "splicing across the insertion verbs" do
    def doc_with_target
      d = Makiri::XML("<r><t/></r>")
      [d, d.at_xpath("//t")]
    end

    it "add_child / << append the children" do
      d, t = doc_with_target
      t.add_child(d.fragment("<a/><b/>"))
      expect(d.root.to_xml).to eq("<r><t><a/><b/></t></r>")
    end

    it "before / after place the children in order around the node" do
      d, t = doc_with_target
      t.before(d.fragment("<x/><y/>"))
      expect(d.root.to_xml).to eq("<r><x/><y/><t/></r>")

      d, t = doc_with_target
      t.after(d.fragment("<x/><y/>"))
      expect(d.root.to_xml).to eq("<r><t/><x/><y/></r>")
    end

    it "replace swaps the node for the fragment's children" do
      d, t = doc_with_target
      t.replace(d.fragment("<x/>mid<y/>"))
      expect(d.root.to_xml).to eq("<r><x/>mid<y/></r>")
    end
  end

  describe "cross-document import" do
    it "deep-copies a standalone fragment into the target and re-resolves namespaces" do
      doc = Makiri::XML(%(<r xmlns:p="urn:p"/>))
      frag = Makiri::XML::DocumentFragment.parse(%(<p:a xmlns:p="urn:p"/><plain/>))
      doc.root.add_child(frag)
      expect(doc.root.children.map { |c| [c.name, c.namespace_uri] })
        .to eq([["p:a", "urn:p"], ["plain", nil]])
      expect(doc.to_xml).to include("<p:a") # spliced into the live tree
    end
  end
end
