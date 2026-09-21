# frozen_string_literal: true

require "spec_helper"
require "objspace"

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

    it "replaces the root element with a single-element fragment" do
      d = Makiri::XML("<r><a/></r>")
      d.root.replace(d.fragment("<x/>"))
      expect(d.children.map(&:name)).to eq(%w[x])
    end

    it "refuses a replace that would leave two roots, destroying nothing" do
      # The single-root budget is checked across the WHOLE fragment before any
      # link changes, so the replaced subtree survives a rejected replace.
      d = Makiri::XML("<r><a/></r>")
      frag = d.fragment("<x/><y/>")
      expect { d.root.replace(frag) }.to raise_error(Makiri::Error)
      expect(d.root.to_xml).to eq("<r><a/></r>")   # untouched
      expect(frag.children.map(&:name)).to eq(%w[x y]) # fragment intact
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

  describe "a rejected fragment leaves no trace in the arena" do
    # The partial fragment is unreachable - it hangs off a root #fragment never
    # returned - so tree.rs rewinds the arena. Without the rewind, a loop of
    # rejected fragments charged a live document until every later operation
    # failed with Limit: 100k of these grew a <r/> document to 77 MB.
    it "does not grow the document" do
      doc = Makiri::XML("<r/>")
      before = ObjectSpace.memsize_of(doc)
      2_000.times do
        expect { doc.fragment("<a><b>#{"x" * 200}</b>") }.to raise_error(Makiri::Error)
      end
      expect(ObjectSpace.memsize_of(doc) - before).to be < 4_096
    end

    it "leaves the document usable and unchanged" do
      doc = Makiri::XML("<r><keep/></r>")
      expect { doc.fragment("<unclosed>") }.to raise_error(Makiri::Error)
      expect(doc.to_xml).to include("<r><keep/></r>")
      frag = doc.fragment("<ok/>")
      doc.root.add_child(frag)
      expect(doc.root.children.map(&:name)).to eq(%w[keep ok])
    end
  end

  describe "a rejected fragment insertion is all or nothing" do
    # place() used to insert a fragment's children one at a time, so a child the
    # rules refused left the earlier ones linked. The document node found it:
    # two elements there is one too many, and the first was already the root by
    # the time the second was refused. Place::Replace always validated the whole
    # fragment first; the other three verbs do now too.
    it "leaves the document untouched when one child of an appended fragment is refused" do
      doc = Makiri::XML::Document.new
      frag = doc.fragment("<a/><b/>")
      expect { doc.add_child(frag) }.to raise_error(Makiri::Error)
      expect(doc.root).to be_nil
      expect(doc.to_xml).not_to include("<a/>")
      expect(frag.children.map(&:name)).to eq(%w[a b])
    end

    it "still appends a fragment the rules allow" do
      doc = Makiri::XML::Document.new
      doc.add_child(doc.fragment("<only/>"))
      expect(doc.root.name).to eq("only")
    end

    it "refuses a second root through before/after as well" do
      doc = Makiri::XML("<r/>")
      %i[add_previous_sibling add_next_sibling].each do |verb|
        frag = doc.fragment("<x/><y/>")
        expect { doc.root.public_send(verb, frag) }.to raise_error(Makiri::Error)
        expect(doc.root.name).to eq("r")
        expect(doc.children.map(&:name)).to eq(%w[r])
        expect(frag.children.map(&:name)).to eq(%w[x y])
      end
    end
  end
end
