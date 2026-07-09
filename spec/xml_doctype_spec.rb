# frozen_string_literal: true

require "spec_helper"

# DOCTYPE handling: the native XML reader RECOGNIZES a `<!DOCTYPE ...>` and
# retains its name + external identifiers (Document#internal_subset, a
# Makiri::XML::DocumentType; the Nokogiri name DTD is an alias), but it does NOT
# process the DTD - no
# entity/element declarations are loaded and no external subset is fetched. The
# security consequences (undefined entities, zero I/O) live in xml_security_spec.
RSpec.describe "Makiri::XML DOCTYPE / internal_subset" do
  def dtd(src)
    Makiri::XML(src).internal_subset
  end

  it "returns nil when the document has no DOCTYPE" do
    expect(Makiri::XML("<r>hi</r>").internal_subset).to be_nil
  end

  it "exposes a name-only DOCTYPE with nil external/system ids" do
    d = dtd("<!DOCTYPE html><html/>")
    expect(d).to be_a(Makiri::XML::DocumentType)
    expect(d.name).to eq("html")
    expect(d.external_id).to be_nil
    expect(d.public_id).to be_nil
    expect(d.system_id).to be_nil
  end

  it "exposes a SYSTEM external id" do
    d = dtd(%(<!DOCTYPE root SYSTEM "root.dtd"><root/>))
    expect(d.name).to eq("root")
    expect(d.external_id).to be_nil
    expect(d.system_id).to eq("root.dtd")
  end

  it "exposes a PUBLIC external id (public_id aliases external_id)" do
    d = dtd(%(<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.0 Strict//EN" ) +
            %("http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd"><html/>))
    expect(d.name).to eq("html")
    expect(d.external_id).to eq("-//W3C//DTD XHTML 1.0 Strict//EN")
    expect(d.public_id).to eq(d.external_id)
    expect(d.system_id).to eq("http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd")
  end

  it "distinguishes an absent id (nil) from an empty literal (\"\")" do
    d = dtd(%(<!DOCTYPE r PUBLIC "" ""><r/>))
    expect(d.public_id).to eq("")
    expect(d.system_id).to eq("")
  end

  it "skips an internal subset without processing it" do
    d = dtd(%(<!DOCTYPE r [ <!ELEMENT r (#PCDATA)> <!ATTLIST r a CDATA #IMPLIED> ]><r>x</r>))
    expect(d.name).to eq("r")
    # the internal subset is balanced (brackets / quoted '>') but not parsed
    expect(d.external_id).to be_nil
    expect(d.system_id).to be_nil
  end

  it "links the DOCTYPE into the tree before the root (browser-DOM order), yet XPath 1.0 excludes it" do
    doc = Makiri::XML(%(<!DOCTYPE r SYSTEM "r.dtd"><r><a/><b/></r>))
    # In the tree: a real first child of the document, before the root, with
    # parent/sibling links - like a browser DOM (and Document#children).
    expect(doc.children.map(&:class)).to eq([Makiri::XML::DocumentType, Makiri::XML::Element])
    expect(doc.root.previous).to be_a(Makiri::XML::DocumentType)
    expect(doc.root.previous.name).to eq("r")
    expect(doc.internal_subset.name).to eq("r") # same node, cached
    # But the XPath 1.0 data model has no doctype node, so it stays invisible there.
    expect(doc.xpath("//node()").map(&:name)).to eq(%w[r a b])
    expect(doc.xpath("count(/node())")).to eq(1.0) # only the root element under /
  end

  it "rejects a second DOCTYPE and a DOCTYPE after the root" do
    expect { Makiri::XML("<!DOCTYPE a><!DOCTYPE b><a/>") }
      .to raise_error(Makiri::XML::SyntaxError)
    expect { Makiri::XML("<a/><!DOCTYPE a>") }
      .to raise_error(Makiri::XML::SyntaxError)
  end

  it "serializes back to a DOCTYPE declaration" do
    expect(dtd("<!DOCTYPE html><html/>").to_xml).to eq("<!DOCTYPE html>")
    expect(dtd(%(<!DOCTYPE r SYSTEM "r.dtd"><r/>)).to_xml).to eq(%(<!DOCTYPE r SYSTEM "r.dtd">))
  end

  describe "#create_document_type (DOM createDocumentType / createDocument)" do
    let(:doc) { Makiri::XML::Document.new }

    it "creates a detached DocumentType with name / public / system ids" do
      dt = doc.create_document_type("svg", "-//W3C//DTD SVG", "http://x/svg.dtd")
      expect(dt).to be_a(Makiri::XML::DocumentType)
      expect(dt.name).to eq("svg")
      expect(dt.public_id).to eq("-//W3C//DTD SVG")
      expect(dt.system_id).to eq("http://x/svg.dtd")
      expect(dt.parent).to be_nil
    end

    it "treats an omitted or empty public / system id as absent" do
      dt = doc.create_document_type("svg")
      expect(dt.public_id).to be_nil
      expect(dt.system_id).to be_nil
      expect(doc.create_document_type("svg", "").public_id).to be_nil
    end

    it "preserves the name's case (XML is case-sensitive)" do
      expect(doc.create_document_type("SVG").name).to eq("SVG")
    end

    it "fails closed on an invalid name, embedded NUL, or a '\"' in an id" do
      expect { doc.create_document_type("1 bad") }.to raise_error(ArgumentError)
      expect { doc.create_document_type("a\x00b") }.to raise_error(Makiri::Error)
      expect { doc.create_document_type("ok", %(a"b)) }.to raise_error(Makiri::Error)
    end

    it "assembles a createDocument-style tree: [doctype, documentElement]" do
      dt = doc.create_document_type("svg")
      root = doc.create_loose_dom_element("svg", nil, "svg", "http://www.w3.org/2000/svg")
      doc.add_child(dt)
      doc.add_child(root)
      expect(doc.children.map(&:class)).to eq([Makiri::XML::DocumentType, Makiri::XML::Element])
      expect(dt.parent).to equal(doc)
      expect(dt.next.name).to eq("svg")            # the root follows the doctype
      expect(doc.root.previous.name).to eq("svg")  # (a fresh wrapper per traversal, so compare by node)
      expect(doc.root.previous).to be_a(Makiri::XML::DocumentType)
      expect(doc.internal_subset.name).to eq("svg")
    end

    it "round-trips a created doctype through serialization" do
      d = Makiri::XML("<r/>")
      d.root.add_previous_sibling(d.create_document_type("r", "-//pub", "sys.dtd"))
      expect(d.to_xml).to include(%(<!DOCTYPE r PUBLIC "-//pub" "sys.dtd">))
    end

    describe "placement guards (fail-closed, WHATWG doctype rules)" do
      let(:d) { Makiri::XML("<r><a/></r>") }

      it "rejects a doctype that would follow the document element" do
        expect { d.add_child(d.create_document_type("r")) }.to raise_error(Makiri::Error)
        expect { d.root.add_next_sibling(d.create_document_type("r")) }.to raise_error(Makiri::Error)
      end

      it "rejects a doctype under a non-document parent" do
        expect { d.root.add_child(d.create_document_type("r")) }.to raise_error(Makiri::Error)
      end

      it "rejects a second doctype" do
        d.root.add_previous_sibling(d.create_document_type("r"))
        expect { d.root.add_previous_sibling(d.create_document_type("r2")) }.to raise_error(Makiri::Error)
      end

      it "leaves the tree and node unchanged after a refused insert" do
        dt = d.create_document_type("r")
        before = d.children.map(&:class)
        expect { d.add_child(dt) }.to raise_error(Makiri::Error)
        expect(d.children.map(&:class)).to eq(before)
        expect(dt.parent).to be_nil
      end

      it "clears the internal_subset cache when the doctype is unlinked, and allows a fresh one" do
        d.root.add_previous_sibling(d.create_document_type("r"))
        d.internal_subset.unlink
        expect(d.internal_subset).to be_nil
        d.root.add_previous_sibling(d.create_document_type("r2"))
        expect(d.internal_subset.name).to eq("r2")
      end

      it "replaces a doctype with an element consistently via a single node or a fragment" do
        # A doctype-only document; replacing the doctype with an element must
        # behave the same whether the replacement is one node or a fragment (the
        # replaced node is excluded from the single-root / ordering checks).
        %i[single fragment].each do |mode|
          doc = Makiri::XML::Document.new
          doc.add_child(doc.create_document_type("r"))
          repl = mode == :single ? doc.create_element("x") : doc.fragment("<x/>")
          doc.internal_subset.replace(repl)
          expect(doc.children.map(&:class)).to eq([Makiri::XML::Element])
          expect(doc.internal_subset).to be_nil
        end
      end
    end
  end
end
