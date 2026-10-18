# frozen_string_literal: true

require "spec_helper"

# Steps of the pinned HTML Standard that Lexbor, the parser the port was
# translated from, lacks, and the port takes (docs/parser_differences.ja.md,
# the P items). Lexbor's DOM (MAKIRI_LEXBOR_DOM=1) still answers the old way,
# so these run on the port only.
RSpec.describe "HTML Standard steps the port takes and Lexbor does not" do
  before { skip "Lexbor's DOM answers the old way" if Makiri.send(:__lexbor_dom?) }

  # P1: whatwg/html#12908 - "after head", anything else: the implicit <body>
  # sets frameset-ok back to "ok".
  it "lets a frameset replace the implicit body after a template in the head" do
    doc = Makiri::HTML("<head><template></template></head><p><frameset>")
    expect(doc.at_xpath("/html/frameset")).not_to be_nil
    expect(doc.at_xpath("/html/body")).to be_nil
  end

  # P2: whatwg/html#12257 - "parsing template contents" decides the form
  # element pointer: a fragment parsed in a template context counts, and a
  # form start tag in a table inside a template is kept.
  it "nests forms in a fragment parsed in a template context" do
    frag = Makiri::DocumentFragment.parse("<form><form></form></form>", context: "template")
    expect(frag.to_html).to eq("<form><form></form></form>")
  end

  it "keeps a form start tag in a table inside a template" do
    doc = Makiri::HTML("<template><table><form></table></template>")
    expect(doc.at_css("template").inner_html).to eq("<table><form></form></table>")
  end

  # P3: in foreign content, a U+FFFD in the input is "any other character"
  # and sets frameset-ok to "not ok"; only the one a U+0000 became does not.
  it "ignores a frameset after a U+FFFD written in foreign content" do
    doc = Makiri::HTML("<!doctype html><svg>&#xFFFD;</svg><frameset><frame>")
    expect(doc.at_xpath("/html/frameset")).to be_nil
    expect(doc.at_xpath("/html/body")).not_to be_nil
  end

  it "still takes a frameset after a U+0000 in foreign content" do
    doc = Makiri::HTML("<!doctype html><svg>\u0000</svg><frameset><frame>")
    expect(doc.at_xpath("/html/frameset")).not_to be_nil
  end
  # P4: "in table", a character token: the current node may be a template.
  it "takes text in a template's table contents to \"in table text\"" do
    doc = Makiri::HTML("<template><tbody></tbody><p><b></p> </template>")
    expect(doc.at_css("template").inner_html).to eq("<tbody></tbody><p><b></b></p> ")
  end
  # P5: "in column group", a DOCTYPE token: parse error, ignore the token.
  it "ignores a DOCTYPE in a column group" do
    doc = Makiri::HTML("<table><colgroup><!doctype html><col></table>")
    expect(doc.at_css("table").to_html).to eq("<table><colgroup><col></colgroup></table>")
  end
  # P6: "in column group", taking a text token character by character.
  it "keeps the whitespace after other text in a column group with no colgroup to close" do
    doc = Makiri::HTML("<template><col>a b</template>")
    expect(doc.at_css("template").inner_html).to eq("<col> ")
    frag = Makiri::DocumentFragment.parse("a b<col>", context: "colgroup")
    expect(frag.to_html).to eq(" <col>")
  end
  # P7: "after after frameset", whitespace characters go to "in body".
  it "keeps the whitespace of mixed text after a frameset document" do
    doc = Makiri::HTML("<frameset></frameset></html> x ")
    expect(doc.to_html).to eq("<html><head></head><frameset></frameset>  </html>")
  end
end
