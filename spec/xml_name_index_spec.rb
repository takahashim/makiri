# frozen_string_literal: true

require "spec_helper"

# The element-name index (ext/makiri/xml/mkr_xml_index.c) serves document-rooted
# descendant name tests (//name, css("name")) from a name->elements bucket
# instead of walking. These tests pin that the indexed result is byte-identical
# to a ground-truth tree walk, in document order, and that the index is dropped
# on every structural mutation so it never goes stale.
RSpec.describe "Makiri::XML element-name index" do
  # Ground truth: every element with the given local name (+ optional ns uri),
  # collected by an independent recursive walk, in document order.
  def walk_by_name(node, local, ns = nil, out = [])
    node.children.each do |c|
      next unless c.respond_to?(:element?) && c.element?
      out << c if c.local_name == local && (ns.nil? || c.namespace_uri == ns)
      walk_by_name(c, local, ns, out)
    end
    out
  end

  it "matches a tree walk in document order (no namespace)" do
    doc = Makiri::XML("<r><a id='1'/><b><a id='2'/></b><a id='3'/><c><d><a id='4'/></d></c></r>")
    truth = walk_by_name(doc, "a").map { |e| e["id"] }
    expect(truth).to eq(%w[1 2 3 4])
    expect(doc.css("a").map { |e| e["id"] }).to eq(truth)
    expect(doc.xpath("//a").map { |e| e["id"] }).to eq(truth)
  end

  it "distinguishes namespaces (default vs prefixed vs other)" do
    doc = Makiri::XML(<<~XML)
      <feed xmlns="urn:a" xmlns:dc="urn:dc">
        <entry id="a1"/><dc:entry id="d1"/>
        <wrap><entry id="a2"/></wrap>
      </feed>
    XML
    expect(doc.css("entry").map { |e| e["id"] }).to eq(%w[a1 a2])          # default ns only
    expect(walk_by_name(doc, "entry", "urn:a").map { |e| e["id"] }).to eq(%w[a1 a2])
    expect(doc.css("dc|entry", "dc" => "urn:dc").map { |e| e["id"] }).to eq(%w[d1])
    expect(doc.xpath("//a:entry", "a" => "urn:a").map { |e| e["id"] }).to eq(%w[a1 a2])
  end

  it "stays correct across repeated queries (the index is cached)" do
    doc = Makiri::XML("<r><x/><x/><y/></r>")
    3.times { expect(doc.css("x").length).to eq(2) }
    3.times { expect(doc.css("y").length).to eq(1) }
    expect(doc.css("z").length).to eq(0)
  end

  it "is invalidated on add_child / before / after / replace" do
    doc = Makiri::XML("<r><x id='1'/></r>")
    expect(doc.css("x").length).to eq(1)
    doc.root.add_child(doc.create_element("x"))
    expect(doc.css("x").length).to eq(2)
    doc.css("x").first.before(doc.create_element("x"))
    expect(doc.css("x").length).to eq(3)
    doc.css("x").first.after(doc.create_element("y"))
    expect(doc.css("x").length).to eq(3)
    expect(doc.css("y").length).to eq(1)
    doc.css("y").first.replace(doc.create_element("x"))
    expect(doc.css("x").length).to eq(4)
    expect(doc.css("y").length).to eq(0)
  end

  it "is invalidated on remove and rename (#name=)" do
    doc = Makiri::XML("<r><a/><a/><b/></r>")
    expect(doc.css("a").length).to eq(2)
    doc.css("a").first.remove
    expect(doc.css("a").length).to eq(1)
    doc.at_css("b").name = "a"
    expect(doc.css("a").length).to eq(2)
    expect(doc.css("b").length).to eq(0)
  end

  it "reflects #content= (children replaced) " do
    doc = Makiri::XML("<r><box><a/><a/></box></r>")
    expect(doc.css("a").length).to eq(2)
    doc.at_css("box").content = "text"
    expect(doc.css("a").length).to eq(0)
  end

  it "agrees with the walk on a custom-named, deeply nested document" do
    inner = (1..40).map { |i| "<Item k='#{i}'><Sub/></Item>" }.join
    doc = Makiri::XML("<Root><Group>#{inner}</Group><Item k='top'/></Root>")
    expect(doc.css("Item").map { |e| e["k"] }).to eq(walk_by_name(doc, "Item").map { |e| e["k"] })
    expect(doc.css("Item").length).to eq(41)
    expect(doc.css("item").length).to eq(0) # case-sensitive
  end
end
