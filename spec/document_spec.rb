# frozen_string_literal: true

RSpec.describe Makiri::Document do
  let(:doc) do
    Makiri::HTML("<html><head><title>Hello</title></head><body><p>x</p></body></html>")
  end

  describe ".parse" do
    it "returns a Document" do
      expect(doc).to be_a(described_class)
      expect(doc).to be_a(Makiri::Node)
    end

    it "synthesizes html/head/body per the HTML5 tree construction rules" do
      expect(Makiri::HTML("<p>x</p>").root.name).to eq("html")
    end

    it "accepts empty input" do
      expect(Makiri::HTML("")).to be_a(described_class)
    end

    it "cannot be constructed with .new" do
      expect { described_class.new }.to raise_error(StandardError)
    end
  end

  describe "#name" do
    it "is 'document'" do
      expect(doc.name).to eq("document")
    end
  end

  describe "#root" do
    it "is the <html> element" do
      expect(doc.root).to be_a(Makiri::Element)
      expect(doc.root.name).to eq("html")
    end
  end

  describe "#title" do
    it "returns the document title" do
      expect(doc.title).to eq("Hello")
    end

    it "is '' when absent" do
      expect(Makiri::HTML("<p>x</p>").title).to eq("")
    end
  end

  describe "#errors" do
    it "is an Array (empty for now)" do
      expect(doc.errors).to eq([])
    end
  end

  describe "#internal_subset (doctype)" do
    it "exposes the doctype name and public/system identifiers" do
      html = %(<!DOCTYPE html PUBLIC "-//W3C//DTD HTML 4.01//EN" ) +
             %("http://www.w3.org/TR/html4/strict.dtd"><html><body>x</body></html>)
      dt = Makiri::HTML(html).internal_subset
      expect(dt).to be_a(Makiri::DocumentType)
      expect(dt.name).to eq("html")
      expect(dt.public_id).to eq("-//W3C//DTD HTML 4.01//EN")
      expect(dt.external_id).to eq(dt.public_id) # Nokogiri-compatible alias
      expect(dt.system_id).to eq("http://www.w3.org/TR/html4/strict.dtd")
    end

    it "reports nil ids for a bare doctype and exposes only one when present" do
      bare = Makiri::HTML("<!DOCTYPE html><html><body>x</body></html>").internal_subset
      expect([bare.public_id, bare.system_id]).to eq([nil, nil])

      sys = Makiri::HTML('<!DOCTYPE potato SYSTEM "taco"><html></html>').internal_subset
      expect([sys.public_id, sys.system_id]).to eq([nil, "taco"])
    end

    it "is nil when the document has no doctype" do
      expect(Makiri::HTML("<html><body>x</body></html>").internal_subset).to be_nil
    end

    it "is reachable as a document child (XPath cannot select it)" do
      doc = Makiri::HTML("<!DOCTYPE html><html><body>x</body></html>")
      expect(doc.children.first).to eq(doc.internal_subset)
      expect(doc.internal_subset.node_type).to eq(10) # DOCUMENT_TYPE
    end
  end
end
