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
  # P8: markup declaration open, anything else: the bogus comment holds what
  # was read, also when the input ends inside "[CDATA[".
  it "keeps a partial [CDATA[ at the end of the input as the comment's data" do
    expect(Makiri::HTML("<![CDA").children.first.content).to eq("[CDA")
  end
  # P9: markup declaration open at the end of the input: an empty comment.
  it "makes an empty comment of a <! at the end of the input" do
    doc = Makiri::HTML("x<!")
    expect(doc.at_css("body").inner_html).to eq("x<!---->")
  end
  # P10: the Noah's Ark clause compares values; `x` and `x=""` both have "".
  it "counts an attribute with no value as the empty value among formatting elements" do
    doc = Makiri::HTML(%(<p><b x><b x><b x=""><b x=""></p>x))
    expect(doc.css("body > b, body > b b").size).to eq(3)
  end
  # P13: "in body", an "input" start tag: "hidden" compared ASCII case-insensitively.
  it "keeps frameset-ok after an input whose type is HIDDEN" do
    doc = Makiri::HTML(%(<input type=HIDDEN><frameset>))
    expect(doc.at_css("frameset")).not_to be_nil
  end
  # P15: foreign content, the start tags that break out include "sup".
  it "takes a <sup> out of SVG like a <sub>" do
    doc = Makiri::HTML("<svg><sup>x</sup></svg>")
    expect(doc.at_css("body").inner_html).to eq("<svg></svg><sup>x</sup>")
  end
  # P14: "in table", anything else: "in body" reprocesses <image> as <img>.
  it "fosters an <image> in a table as an <img>" do
    expect(Makiri::HTML("<table><tr><image>").at_css("body").inner_html)
      .to eq("<img><table><tbody><tr></tr></tbody></table>")
    expect(Makiri::DocumentFragment.parse("<image>", context: "table").to_html).to eq("<img>")
  end
  # P12: "in body", a "textarea" start tag: switch to "text".
  it "reads a textarea's contents without reconstructing formatting elements" do
    doc = Makiri::HTML("<p><b></p><textarea>x</textarea>y")
    expect(doc.at_css("body").inner_html).to eq("<p><b></b></p><textarea>x</textarea><b>y</b>")
  end
  # P11: the adoption agency algorithm, steps 13, 14.5 and 19: the bookmark
  # stays between its neighbours when entries are removed before it.
  it "reconstructs the active formatting elements the adoption agency left" do
    doc = Makiri::HTML("<em><s><b><div><strike><tt><nobr><a><div></s></div></div>x")
    expect(doc.at_css("body > em > b").children.last.to_html)
      .to eq("<tt><nobr><a>x</a></nobr></tt>")
  end
  # P16: "the end" pops the stack of open elements for a fragment too, and
  # an option's pop steps update the select's selectedcontent.
  it "updates a fragment's selectedcontent from an option left open" do
    frag = Makiri::DocumentFragment.parse("<select><button><selectedcontent></selectedcontent></button><option>A")
    expect(frag.at_css("selectedcontent").inner_html).to eq("A")
  end
  # P17: the fragment parsing algorithm, step 2: the document takes the
  # context document's quirks mode.
  it "parses inner_html= in the quirks mode of the element's document" do
    doc = Makiri::HTML("<p>quirks</p><div id=a></div>")
    doc.at_css("#a").inner_html = "<p><table></table>"
    expect(doc.at_css("#a").inner_html).to eq("<p><table></table></p>")
    doc = Makiri::HTML("<!doctype html><div id=a></div>")
    doc.at_css("#a").inner_html = "<p><table></table>"
    expect(doc.at_css("#a").inner_html).to eq("<p></p><table></table>")
  end
  # P18: the fragment parsing algorithm reads the context element's
  # attributes: an annotation-xml with encoding="text/html" is an HTML
  # integration point.
  it "parses inner_html= of an HTML-encoded annotation-xml as HTML" do
    doc = Makiri::HTML(%(<!doctype html><math><annotation-xml encoding="text/html"></annotation-xml></math>))
    ax = doc.at_xpath('//*[local-name()="annotation-xml"]')
    ax.inner_html = "<foo></foo>"
    expect(ax.children.first.xpath("namespace-uri()")).to eq("http://www.w3.org/1999/xhtml")
  end
  # P20: the fragment parsing algorithm sets the form element pointer to the
  # context's nearest form, itself or an ancestor.
  it "ignores a <form> in inner_html= of an element inside a form" do
    doc = Makiri::HTML("<!doctype html><form><div id=a></div></form>")
    doc.at_css("#a").inner_html = "<form><input></form>x"
    expect(doc.at_css("#a").inner_html).to eq("<input>x")
  end
  # P19: an element in a namespace the parser does not know is foreign:
  # CDATA sections are read in it, and foreign attributes adjusted.
  it "parses inner_html= of an element in another namespace as foreign content" do
    doc = Makiri::HTML("<!doctype html><body>")
    el = doc.create_element_ns("urn:x", "q:foo")
    doc.at_css("body") << el
    el.inner_html = %(<bar xlink:href="a"/><![CDATA[x<y]]>)
    expect(el.to_html).to eq(%(<q:foo><bar xlink:href="a"></bar>x&lt;y</q:foo>))
    expect(el.at_xpath("*/@*").xpath("namespace-uri()")).to eq("http://www.w3.org/1999/xlink")
  end
  # R1: serializing HTML fragments, step 1: a void element serializes as "".
  it "serializes a void element's children as nothing" do
    doc = Makiri::HTML("<!doctype html><br>")
    br = doc.at_css("br")
    br << doc.create_text_node("x")
    expect(br.inner_html).to eq("")
    expect(br.to_html).to eq("<br>")
  end
  # R2: serializing HTML fragments: an element in the HTML, SVG or MathML
  # namespace is written by its local name.
  it "writes a prefixed HTML, SVG or MathML element by its local name" do
    doc = Makiri::HTML("<!doctype html><p></p>")
    p_el = doc.at_css("p")
    p_el << doc.create_element_ns("http://www.w3.org/2000/svg", "s:rect")
    p_el << doc.create_element_ns("http://www.w3.org/1999/xhtml", "h:br")
    p_el << doc.create_element_ns("urn:x", "q:foo")
    expect(p_el.inner_html).to eq("<rect></rect><br><q:foo></q:foo>")
  end
end
