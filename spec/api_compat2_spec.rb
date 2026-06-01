# frozen_string_literal: true

# Remaining Nokogiri-compat API: Node#name=, #to_h, Document#encoding /
# #meta_encoding(=), ProcessingInstruction + DocumentFragment.
RSpec.describe "Makiri Nokogiri-compat API (part 2)" do
  describe "Node#name=" do
    let(:doc) { Makiri::HTML('<html><body><div id="m" class="c"><p>x</p></div></body></html>') }

    it "renames an element in place, preserving identity, attributes, children" do
      div = doc.at_css("#m")
      div.name = "section"
      expect(div.name).to eq("section")
      expect(doc.at_css("section")).to eq(div)  # same node
      expect(div["id"]).to eq("m")
      expect(div["class"]).to eq("c")
      expect(div.css("p").length).to eq(1)
    end

    it "is reflected in serialization and queries" do
      div = doc.at_css("#m")
      div.name = "article"
      expect(div.to_html).to start_with("<article")
      expect(doc.xpath("//article").length).to eq(1)
      expect(doc.css("div").length).to eq(0)
    end

    it "rejects non-elements" do
      text = doc.at_css("p").child
      expect { text.name = "x" }.to raise_error(Makiri::Error)
    end
  end

  describe "Node#to_h" do
    it "returns a name => value hash" do
      doc = Makiri::HTML('<html><body><a id="x" href="/y" data-n="1">L</a></body></html>')
      expect(doc.at_css("a").to_h).to eq("id" => "x", "href" => "/y", "data-n" => "1")
    end
  end

  describe "Document encoding" do
    it "reports UTF-8 as the in-memory encoding" do
      expect(Makiri::HTML("<p>x</p>").encoding).to eq("UTF-8")
    end

    it "reads the declared meta charset" do
      d1 = Makiri::HTML('<html><head><meta charset="Shift_JIS"></head><body></body></html>')
      expect(d1.meta_encoding).to eq("Shift_JIS")

      d2 = Makiri::HTML('<html><head><meta http-equiv="Content-Type" content="text/html; charset=EUC-JP"></head></html>')
      expect(d2.meta_encoding).to eq("EUC-JP")

      expect(Makiri::HTML("<html><head></head></html>").meta_encoding).to be_nil
    end

    it "sets the meta charset, inserting a <meta> when absent" do
      doc = Makiri::HTML("<html><head></head><body></body></html>")
      doc.meta_encoding = "UTF-8"
      expect(doc.meta_encoding).to eq("UTF-8")
      expect(doc.css("meta[charset]").length).to eq(1)
    end
  end

  describe "ProcessingInstruction" do
    it "is part of the node class hierarchy" do
      expect(Makiri::ProcessingInstruction.ancestors).to include(Makiri::Node)
    end
  end

  describe "DocumentFragment" do
    describe ".parse" do
      let(:frag) { Makiri::DocumentFragment.parse('<a href="/x">L</a><b>z</b>') }

      it "builds a queryable, serializable fragment" do
        expect(frag).to be_a(Makiri::DocumentFragment)
        expect(frag).to be_document_fragment
        expect(frag.children.map(&:name)).to eq(%w[a b])
        expect(frag.css("a").first["href"]).to eq("/x")
        expect(frag.to_html).to eq('<a href="/x">L</a><b>z</b>')
      end

      it "cannot be spliced into a different document" do
        doc = Makiri::HTML("<html><body><div></div></body></html>")
        expect { doc.at_css("div").add_child(frag) }.to raise_error(Makiri::Error)
      end
    end

    describe "Document#fragment + splicing" do
      let(:doc) { Makiri::HTML('<html><body><section id="m"></section></body></html>') }
      let(:host) { doc.at_css("#m") }

      it "add_child contributes the fragment's children" do
        host.add_child(doc.fragment("<i>one</i><u>two</u>"))
        expect(host.inner_html).to eq("<i>one</i><u>two</u>")
      end

      it "splices around siblings in document order" do
        host.add_child(doc.fragment("<i>one</i>"))
        doc.at_css("i").add_next_sibling(doc.fragment("<s>a</s><s>b</s>"))
        doc.at_css("i").add_previous_sibling(doc.fragment("<em>p</em>"))
        expect(host.inner_html).to eq("<em>p</em><i>one</i><s>a</s><s>b</s>")
      end

      it "replaces a node with a fragment's children" do
        host.add_child(doc.fragment("<i>one</i>"))
        doc.at_css("i").replace(doc.fragment("<r>1</r><r>2</r>"))
        expect(host.inner_html).to eq("<r>1</r><r>2</r>")
      end
    end
  end

  describe "memory safety" do
    it "survives fragment churn under GC stress" do
      GC.stress = true
      begin
        doc = Makiri::HTML('<html><body><div id="m"></div></body></html>')
        5.times { |i| doc.at_css("#m").add_child(doc.fragment("<p>#{i}</p>")) }
        GC.compact
        expect(doc.css("#m p").length).to eq(5)
        f = Makiri::DocumentFragment.parse("<span>s</span>")
        expect(f.to_html).to eq("<span>s</span>")
      ensure
        GC.stress = false
      end
    end
  end
end
