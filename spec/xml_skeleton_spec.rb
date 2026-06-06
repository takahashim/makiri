# frozen_string_literal: true

require "spec_helper"

# Phase-1 skeleton (§14 step 4): Makiri::XML::Document.parse(source) (and the
# Makiri::XML(source) convenience that delegates to it) decode
# the input strictly, run the Ruby-free parser with the GVL released, and return
# a Makiri::XML::Document backed by the secure append-only arena. The tokenizer
# and tree builder land next; for now the document is empty, and this exercises
# the decode -> GVL -> arena -> doc -> wrap -> GC pipeline.
RSpec.describe "Makiri::XML skeleton" do
  it "returns a Makiri::XML::Document" do
    expect(Makiri::XML("<feed/>")).to be_a(Makiri::XML::Document)
    expect(Makiri::XML::Document.parse("<a><b/></a>")).to be_a(Makiri::XML::Document)
  end

  it "strict-decodes input (fail-closed) before parsing" do
    expect { Makiri::XML("a\x00b") }.to raise_error(Makiri::XML::SyntaxError)
    expect { Makiri::XML("\xFF\xFE".b) }.to raise_error(Makiri::XML::SyntaxError)
  end

  it "honours the input String encoding (Shift_JIS -> UTF-8) without crashing" do
    sjis = "<a>\x93\xfa\x96{</a>".dup.force_encoding("Shift_JIS")
    expect(Makiri::XML(sjis)).to be_a(Makiri::XML::Document)
  end

  it "frees the arena cleanly (no leak / double-free under the sanitizer)" do
    200.times { Makiri::XML("<x/>") }
    GC.start
    expect(Makiri::XML("<y/>")).to be_a(Makiri::XML::Document)
  end

  it "can be constructed empty via .new (built up programmatically)" do
    d = Makiri::XML::Document.new
    expect(d).to be_a(Makiri::XML::Document)
    expect(d.root).to be_nil
  end
end
