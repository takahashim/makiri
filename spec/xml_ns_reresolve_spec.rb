# frozen_string_literal: true

require "spec_helper"

# mkr_xml_mutate.c states its own invariant: "a mutated node's ns_uri is
# identical to what a re-parse would compute". Changing an xmlns declaration
# used to break it - mkr_xml_set_attribute / mkr_xml_remove_attribute never
# re-resolved the subtree, so descendants kept URIs from the old binding.
#
# It stayed hidden because a resolved ns_uri is invisible to serialization
# (only the prefix is written), so comparing serialized output finds nothing.
# What notices is XPath, which matches name tests on the resolved URI.
RSpec.describe "Makiri::XML namespace re-resolution after a declaration changes" do
  # The tree's own view of every element's resolved namespace.
  def resolved(node, out = [])
    node.children.each do |c|
      next unless c.node_type == 1

      out << [c.name, c.namespace_uri]
      resolved(c, out)
    end
    out
  end

  # The invariant: the tree agrees with what re-parsing its own output computes.
  def expect_matches_reparse(doc)
    expect(resolved(doc)).to eq(resolved(Makiri::XML(doc.root.to_xml)))
  end

  it "re-resolves the subtree when a declaration is removed" do
    doc = Makiri::XML(%(<r xmlns:p="urn:a"><mid xmlns:p="urn:b"><p:x/></mid></r>))
    expect(doc.at_xpath("//p:x", "p" => "urn:b")).not_to be_nil

    doc.root.children.first.delete("xmlns:p")

    expect(doc.at_xpath("//p:x", "p" => "urn:a")).not_to be_nil
    expect(doc.at_xpath("//p:x", "p" => "urn:b")).to be_nil
    expect_matches_reparse(doc)
  end

  it "re-resolves the subtree when a declaration is added" do
    doc = Makiri::XML(%(<r xmlns:p="urn:a"><mid><p:x/></mid></r>))
    doc.root.children.first["xmlns:p"] = "urn:b"

    expect(doc.at_xpath("//p:x", "p" => "urn:b")).not_to be_nil
    expect(doc.at_xpath("//p:x", "p" => "urn:a")).to be_nil
    expect_matches_reparse(doc)
  end

  it "re-resolves when a declaration's value is replaced in place" do
    doc = Makiri::XML(%(<r xmlns:p="urn:a"><p:x/></r>))
    doc.root["xmlns:p"] = "urn:z"

    expect(doc.at_xpath("//p:x", "p" => "urn:z")).not_to be_nil
    expect_matches_reparse(doc)
  end

  it "re-resolves the default namespace too" do
    doc = Makiri::XML(%(<r xmlns="urn:a"><x/></r>))
    expect(doc.root.children.first.namespace_uri).to eq("urn:a")

    doc.root["xmlns"] = "urn:b"

    expect(doc.root.children.first.namespace_uri).to eq("urn:b")
    expect_matches_reparse(doc)
  end

  it "reaches the whole subtree, not just the direct children" do
    doc = Makiri::XML(%(<r xmlns:p="urn:a"><mid><a><b><p:deep p:k="v"/></b></a></mid></r>))
    doc.root.children.first["xmlns:p"] = "urn:b"

    deep = doc.at_xpath("//p:deep", "p" => "urn:b")
    expect(deep).not_to be_nil
    expect(deep.attribute_nodes.first.namespace_uri).to eq("urn:b")
    expect_matches_reparse(doc)
  end

  it "leaves an ordinary attribute alone" do
    doc = Makiri::XML(%(<r xmlns:p="urn:a"><p:x/></r>))
    doc.root["id"] = "v"
    expect(doc.root.children.first.namespace_uri).to eq("urn:a")
    expect_matches_reparse(doc)
  end

  # Removing the last declaration of a prefix its descendants still use leaves a
  # subtree that cannot resolve. Re-resolution is all-or-nothing, so the tree is
  # left as it was rather than half-rewritten. (Whether the removal should be
  # refused outright is a separate spec question.)
  it "leaves the subtree untouched when it would no longer resolve" do
    doc = Makiri::XML(%(<r xmlns:p="urn:a"><p:x/></r>))
    before = resolved(doc)

    doc.root.delete("xmlns:p")

    expect(resolved(doc)).to eq(before)
  end

  # The same all-or-nothing rule on the insertion path: a rejected move must not
  # leave the moved subtree carrying URIs resolved against the rejected context.
  it "leaves the subtree untouched when an insert is rejected mid-resolution" do
    doc = Makiri::XML(<<~XML)
      <r xmlns:p="urn:p">
        <a xmlns:q="urn:q" xmlns:p="urn:inner"><mid><p:y/><q:x/></mid></a>
        <b/>
      </r>
    XML
    mid = doc.root.children.find { |c| c.name == "a" }.children.first
    before = [[mid.name, mid.namespace_uri]] + resolved(mid)

    expect { doc.root.children.find { |c| c.name == "b" }.add_child(mid) }
      .to raise_error(Makiri::Error, /not bound/)

    expect([[mid.name, mid.namespace_uri]] + resolved(mid)).to eq(before)
  end
end
