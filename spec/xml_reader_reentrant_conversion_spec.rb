# frozen_string_literal: true

# An XML reader that converts a Ruby argument runs Ruby code: the argument's
# `to_str`. That code may edit the very document being read. The readers
# convert first and only then read the document, so they answer from the
# document as the conversion left it, and nothing reads it while Ruby is
# changing it.
#
# What this guards is the observable order. The hazard it was written for - a
# shared borrow of the arena held across the conversion while the edit takes a
# mutable one - is undefined behaviour that no test (ASan included) reports on
# its own; the order in `glue/xml_node/read.rs` is what rules it out.
RSpec.describe "Makiri::XML readers whose argument conversion edits the document" do
  let(:doc) { Makiri::XML("<r a='1'><c/></r>") }
  let(:root) { doc.root }

  def name_that_edits(root, doc)
    name = Object.new
    name.define_singleton_method(:to_str) do
      root["a"] = "changed"
      root.add_child(doc.create_element("added"))
      "a"
    end
    name
  end

  it "#[] answers from the document as the conversion left it" do
    expect(root[name_that_edits(root, doc)]).to eq("changed")
    expect(root.children.map(&:name)).to eq(%w[c added])
  end

  it "#attribute_by_qualified_name answers from the document as the conversion left it" do
    attr = root.attribute_by_qualified_name(name_that_edits(root, doc))
    expect(attr.value).to eq("changed")
    expect(root.children.map(&:name)).to eq(%w[c added])
  end

  it "still answers nil for a non-element without converting the argument" do
    text = Makiri::XML("<r>t</r>").root.children.first
    untouched = Object.new
    untouched.define_singleton_method(:to_str) { raise "converted" }
    expect(text[untouched]).to be_nil
  end
end
