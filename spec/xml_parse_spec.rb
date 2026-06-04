# frozen_string_literal: true

require "spec_helper"

# Minimal XML tokenizer + tree builder (§14 steps 5-6): elements, attributes and
# character data, with fail-closed well-formedness errors. Namespaces, entities,
# comments/CDATA/PI/XML-decl/DOCTYPE arrive in later steps. Tree *structure* is
# asserted in C (Makiri.__c_selftest -> mkr_xml_parse_selftest, also run under
# the sanitizer); these specs pin the accept/reject contract from Ruby.
RSpec.describe "Makiri::XML minimal parse" do
  def ok?(src)
    Makiri::XML(src).is_a?(Makiri::XML::Document)
  end

  describe "accepts well-formed documents" do
    %w[
      <a/>
      <a></a>
      <a><b/><c></c></a>
    ].each do |src|
      it(src) { expect(ok?(src)).to be true }
    end

    it "attributes (single and double quoted)" do
      expect(ok?(%(<a x="1" y='two' z="a b"/>))).to be true
    end

    it "mixed character data and child elements" do
      expect(ok?("<a>hello <b>x</b> world</a>")).to be true
    end

    it "leading/trailing whitespace around the root (misc)" do
      expect(ok?("  \n <root/> \n ")).to be true
    end
  end

  describe "rejects malformed documents (fail-closed SyntaxError)" do
    {
      "unclosed element"          => "<a>",
      "mismatched end tag"        => "<a></b>",
      "multiple root elements"    => "<a/><b/>",
      "content before root"       => "junk<a/>",
      "content after root"        => "<a/>tail",
      "stray end tag"             => "</a>",
      "missing attribute value"   => "<a x=>",
      "unquoted attribute value"  => "<a x=1/>",
      "raw < in attribute value"  => %(<a x="<"/>),
      "unterminated attr value"   => %(<a x="1>),
      "unterminated start tag"    => "<a ",
      "empty element name"        => "<>",
    }.each do |label, src|
      it(label) { expect { Makiri::XML(src) }.to raise_error(Makiri::XML::SyntaxError) }
    end

    # Unsupported in the minimal subset (handled in later steps), rejected for now.
    {
      "comment (step 9)"          => "<!-- c --><a/>",
      "XML declaration (step 9)"  => "<?xml version='1.0'?><a/>",
    }.each do |label, src|
      it(label) { expect { Makiri::XML(src) }.to raise_error(Makiri::XML::SyntaxError) }
    end
  end

  it "the depth budget is fail-closed (deeply nested input does not crash)" do
    deep = "<a>" * 5000 + "</a>" * 5000
    expect { Makiri::XML(deep) }.to raise_error(Makiri::XML::LimitExceeded)
  end
end
