# frozen_string_literal: true

# An HTML <template>'s contents, as a contract: a DOM layer over Makiri
# (dommy) keeps a template's contents in Element#content_fragment itself and
# relies on each behaviour below. The HTML Standard's template has two
# separate lists - its own children (appendChild) and its template contents -
# and every example here is one side of keeping them apart.
RSpec.describe "HTML template contents (contract)" do
  let(:doc) { Makiri::HTML("<body><template id=t><s>1</s></template></body>") }
  let(:template) { doc.at_css("template") }

  it "is one fragment, the same object on every call" do
    a = template.content_fragment
    b = template.content_fragment
    expect(a).to be_a(Makiri::DocumentFragment)
    expect(a).to equal(b)
    expect(a.document).to equal(doc)
    expect(a.children.map(&:name)).to eq(%w[s])
    expect(a.children.first.parent).to equal(a)
  end

  it "writes what the fragment holds, as it is edited" do
    template.content_fragment << doc.create_element("k")
    expect(template.to_html).to eq("<template id=\"t\"><s>1</s><k></k></template>")
    template.content_fragment.children.first.unlink
    expect(template.inner_html).to eq("<k></k>")
  end

  it "keeps the contents out of queries over the document" do
    expect(doc.css("s")).to be_empty
    expect(doc.xpath("//s")).to be_empty
    expect(doc.at_css("#t")).to eq(template)
    expect(template.children).to be_empty
  end

  it "replaces the contents with inner_html=, leaving the element's children empty" do
    template.inner_html = "<i>2</i>"
    expect(template.content_fragment.children.map(&:name)).to eq(%w[i])
    expect(template.children).to be_empty
  end

  it "gives a made or fragment-parsed template the same contents" do
    made = doc.create_element("template")
    expect(made.content_fragment.children).to be_empty
    made.content_fragment << doc.create_element("b")
    expect(made.to_html).to eq("<template><b></b></template>")
    parsed = doc.fragment("<template><u></u></template>").children.first
    expect(parsed.content_fragment.children.map(&:name)).to eq(%w[u])
  end

  it "copies the contents to the copy's contents (import_node, dup)" do
    other = Makiri::HTML("<body></body>")
    [other.import_node(template, true), template.dup].each do |copy|
      expect(copy.content_fragment.children.map(&:name)).to eq(%w[s])
      expect(copy.children).to be_empty
    end
  end

  # "Serializing HTML fragments" writes a template's CONTENTS in place of its
  # children. Lexbor wrote both, so a child given with add_child showed up
  # where a browser writes nothing.
  it "writes the contents, not the template's own children" do
    template.add_child(doc.create_element("k"))
    expect(template.children.map(&:name)).to eq(%w[k])
    expect(template.to_html).to eq("<template id=\"t\"><s>1</s></template>")
    expect(doc.body.inner_html).to eq("<template id=\"t\"><s>1</s></template>")
    nested = Makiri::HTML("<body><template><div><template><i></i></template></div></template></body>")
    nested.at_css("template").add_child(nested.create_element("own"))
    expect(nested.body.inner_html).to eq("<template><div><template><i></i></template></div></template>")
  end

  # Lexbor's own walk serializes until a template of the document may have a
  # child of its own; each way to give it one switches the document over.
  describe "every way to give a template its own child is written as contents only" do
    let(:fresh) { Makiri::HTML("<body><div><template><s></s></template></div></body>") }
    let(:tpl) { fresh.at_css("template") }
    let(:want) { "<template><s></s></template>" }

    it "add_child, and a sibling placed next to an own child" do
      tpl.add_child(fresh.create_element("k"))
      tpl.children.first.add_next_sibling(fresh.create_element("m"))
      expect(tpl.to_html).to eq(want)
    end

    it "content=" do
      tpl.content = "text"
      expect(tpl.children.size).to eq(1)
      expect(tpl.to_html).to eq(want)
    end

    it "outer_html= on an own child" do
      k = fresh.create_element("k")
      tpl << k
      k.outer_html = "<b>x</b>"
      expect(tpl.to_html).to eq(want)
    end

    it "a detached template" do
      made = fresh.create_element("template")
      made.content_fragment << fresh.create_element("s")
      made << fresh.create_element("k")
      expect(made.to_html).to eq(want)
    end

    it "a copy into another document, by import_node and by insertion" do
      tpl << fresh.create_element("k")
      other = Makiri::HTML("<body></body>")
      expect(other.import_node(tpl, true).to_html).to eq(want)
      moved = Makiri::HTML("<body></body>")
      moved.body << tpl
      expect(moved.body.inner_html).to eq(want)
    end

    it "a nested template inside another's contents" do
      outer = Makiri::HTML("<body><template><template><i></i></template></template></body>")
      inner = outer.at_css("template").content_fragment.children.first
      inner << outer.create_element("k")
      expect(outer.body.inner_html).to eq("<template><template><i></i></template></template>")
    end
  end

  # XML has no template contents: crossing into XML, the contents become the
  # copy's children, and the template's own children follow them. They were
  # dropped.
  it "carries both the contents and the own children into XML, contents first" do
    template.add_child(doc.create_element("k"))
    copy = Makiri::XML("<r/>").import_node(template, true)
    expect(copy.children.map(&:name)).to eq(%w[s k])
  end
end
