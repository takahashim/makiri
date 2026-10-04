# frozen_string_literal: true

require "spec_helper"

# What a copy into an XML document costs against its byte budget: no more
# than its source holds, and nothing at all when the copy fails.
RSpec.describe "XML copies and the byte budget" do
  # The smallest max_bytes in `range` for which the block succeeds.
  def smallest(range)
    range.find do |b|
      yield b
      true
    rescue Makiri::XML::LimitExceeded
      false
    end
  end

  # A declaration set with []= is the only copy of the URI the nodes it binds
  # hold; the copy shares it the same way.
  it "dups a document built by mutation within the budget it was built under" do
    long = "urn:" + ("x" * 2000)
    build = lambda do |b|
      d = Makiri::XML::Document.parse("<r/>", max_bytes: b)
      d.root["xmlns:p"] = long
      d.root << d.create_element("p:x")
      d
    end
    budget = smallest(2000..8000) { |b| build.(b) }
    doc = build.(budget)
    expect(doc.dup.to_xml).to eq(doc.to_xml)
  end

  # Every element of one namespace shares one copy of its URI, as the HTML
  # source shares one namespace id.
  it "imports HTML elements sharing their namespace URI" do
    html = Makiri::HTML("<div>" + ("<p></p>" * 200) + "</div>").at_css("div")
    xml_src = Makiri::XML(%(<div xmlns="http://www.w3.org/1999/xhtml">) + ("<p/>" * 200) + "</div>").root
    cost = ->(node) { smallest(1000..60_000) { |b| Makiri::XML::Document.parse("<d/>", max_bytes: b).import_node(node, true) } }
    expect(cost.(html)).to be <= cost.(xml_src) + 100
  end

  # A copy that runs out of budget halfway gives back what it took, so the
  # document can still be edited within what it had.
  describe "a copy that fails" do
    def room(doc)
      n = 0
      loop do
        doc.create_element("e")
        n += 1
      end
    rescue Makiri::XML::LimitExceeded
      n
    end

    let(:big_xml) { Makiri::XML("<r>" + ("<e a='1'>text</e>" * 2000) + "</r>").root }
    let(:big_html) { Makiri::HTML("<div>" + ("<p a='1'>text</p>" * 2000) + "</div>").at_css("div") }

    # The receiving document, built the same way each time: one to count its
    # room untouched, one to fail the copy on. A clone needs something big
    # enough in it to fail.
    def build(with_content)
      d = Makiri::XML::Document.parse("<r/>", max_bytes: 20_000)
      d.root << d.create_text_node("t" * 11_000) if with_content
      d
    end

    {
      "import_node from XML" => ->(d, s) { d.import_node(s[:xml], true) },
      "import_node from HTML" => ->(d, s) { d.import_node(s[:html], true) },
      "add_child from another document" => ->(d, s) { d.root.add_child(s[:xml]) },
      "clone_node(true)" => ->(d, _) { d.root.clone_node(true) },
    }.each do |what, copy|
      it "#{what} leaves the budget as it was" do
        clone = what.start_with?("clone")
        expected = room(build(clone))
        doc = build(clone)
        expect { copy.(doc, xml: big_xml, html: big_html) }.to raise_error(Makiri::XML::LimitExceeded)
        expect(room(doc)).to eq(expected)
      end
    end
  end
end
