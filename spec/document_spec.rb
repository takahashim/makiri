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
end
