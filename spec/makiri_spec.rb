# frozen_string_literal: true

require "stringio"

RSpec.describe Makiri do
  it "has a version number" do
    expect(Makiri::VERSION).not_to be_nil
  end

  describe ".HTML / .parse" do
    it "parses a String into a Document" do
      expect(Makiri::HTML("<p>hi</p>")).to be_a(Makiri::Document)
      expect(Makiri.parse("<p>hi</p>")).to be_a(Makiri::Document)
    end

    it "reads from an IO-like source" do
      io = StringIO.new("<p>hi</p>")
      expect(Makiri::HTML(io)).to be_a(Makiri::Document)
    end
  end
end
