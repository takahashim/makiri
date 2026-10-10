# frozen_string_literal: true

# local_name / prefix / namespace_uri / tag_name answer frozen, interned
# Strings; #name stays a fresh String (Nokogiri's contract).
RSpec.describe "interned DOM names" do
  def check(a, b, meth)
    va = a.public_send(meth)
    expect(va).to be_frozen
    expect(va.encoding).to eq(Encoding::UTF_8)
    expect(b.public_send(meth)).to equal(va)
    expect(va).to equal(-va.dup)
  end

  it "interns HTML element and attribute names" do
    doc = Makiri::HTML("<div class=a></div><div class=b></div><svg><path xlink:href='x'/></svg>")
    a, b = doc.css("div").to_a
    %i[local_name namespace_uri tag_name].each { |m| check(a, b, m) }
    check(a.attribute_nodes.first, b.attribute_nodes.first, :local_name)
    href = doc.at_xpath("//svg:path", "svg" => "http://www.w3.org/2000/svg").attribute_nodes.first
    expect([href.prefix, href.namespace_uri]).to all(be_frozen)
    expect(href.prefix).to eq("xlink")
  end

  it "interns XML names" do
    doc = Makiri::XML("<r xmlns:p='urn:p'><p:e/><p:e/></r>")
    a, b = doc.root.element_children.to_a
    %i[local_name prefix namespace_uri tag_name].each { |m| check(a, b, m) }
  end

  it "keeps #name a fresh, mutable String" do
    doc = Makiri::HTML("<div></div><div></div>")
    a, b = doc.css("div").to_a
    expect(a.name).not_to be_frozen
    expect(a.name).not_to equal(b.name)
    expect(Makiri::XML("<r/>").root.name).not_to be_frozen
  end
end
