# frozen_string_literal: true

require "spec_helper"

# The reserved xml: and xmlns: URIs are in every XML arena from its start, and
# every URI stored after that is checked against them: an attribute in the XML
# namespace costs the same bytes however it got there - parsed, set with
# set_attribute_ns, or imported from another document, XML or HTML.
RSpec.describe "reserved namespace URIs in the XML byte budget" do
  XML_NS = "http://www.w3.org/XML/1998/namespace"

  # The smallest max_bytes under which importing `node` into a fresh
  # document succeeds.
  def import_budget(node)
    (64..4096).find do |b|
      Makiri::XML::Document.parse("<d/>", max_bytes: b).import_node(node, true)
    rescue Makiri::XML::LimitExceeded
      nil
    end
  end

  let(:parsed) { Makiri::XML(%(<r xml:lang="en"/>)).root }

  it "imports an attribute set with set_attribute_ns within the parsed one's budget" do
    doc = Makiri::XML(%(<r/>))
    doc.root.set_attribute_ns(XML_NS, "xml:lang", "en")
    expect(import_budget(doc.root)).to eq(import_budget(parsed))
  end

  # An HTML element carries the XHTML namespace URI an XML <r> does not, so
  # what is compared is what the xml: attribute adds.
  it "imports an HTML element's xml: attribute for what the parsed one costs" do
    html = Makiri::HTML(%(<r></r><r></r>))
    plain, with_lang = html.css("r").to_a
    with_lang.set_attribute_ns(XML_NS, "xml:lang", "en")
    xml_plain = Makiri::XML(%(<r/>)).root
    expect(import_budget(with_lang) - import_budget(plain))
      .to eq(import_budget(parsed) - import_budget(xml_plain))
  end

  it "stores no second copy when set_attribute_ns names a reserved URI" do
    doc = Makiri::XML::Document.parse(%(<r/>), max_bytes: 4096)
    expect do
      40.times { |i| doc.root.set_attribute_ns(XML_NS, "xml:a#{i}", "") }
    end.not_to raise_error
  end
end
