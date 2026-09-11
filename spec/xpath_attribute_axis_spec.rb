# frozen_string_literal: true

require "spec_helper"

# XPath 1.0 §2.2 for an ATTRIBUTE context node: both sibling axes are empty, and
# the following/preceding axes exclude attribute nodes.
#
# The XML backend used to get this wrong. mkr_xml_node_t chains an element's
# attributes through the same next/prev fields as tree siblings, so walking from
# an attribute wandered into the attribute list: `@a/following-sibling::node()`
# returned the element's later attributes, and `@a/following::node()` both
# included them and skipped the element's own children. A name test hid it (an
# attribute never matches an element name test), so only `node()` exposed it.
# Lexbor keeps attributes on a separate list, so HTML was already correct - both
# backends are asserted here so they cannot drift apart again.
RSpec.describe "XPath axes from an attribute context node" do
  def names(node_set)
    node_set.map do |n|
      case n.node_type
      when 2 then "@#{n.name}"
      when 3 then "text"
      else n.name
      end
    end
  end

  shared_examples "attribute-axis conformance" do
    it "makes both sibling axes empty" do
      expect(names(ctx.xpath("following-sibling::node()"))).to eq([])
      expect(names(ctx.xpath("preceding-sibling::node()"))).to eq([])
      expect(names(ctx.xpath("following-sibling::*"))).to eq([])
      expect(names(ctx.xpath("preceding-sibling::*"))).to eq([])
    end

    it "keeps attribute nodes out of following/preceding" do
      expect(names(ctx.xpath("following::node()"))).not_to include("@b", "@c")
      expect(names(ctx.xpath("preceding::node()"))).not_to include("@b", "@c")
    end

    it "walks following from the owner element, matching libxml2" do
      expect(names(ctx.xpath("following::node()"))).to eq(["s"])
    end

    it "leaves the axes that DO contain the owner element alone" do
      expect(names(ctx.xpath("self::node()"))).to eq(["@a"])
      expect(names(ctx.xpath("parent::node()"))).to eq(["e"])
      expect(names(ctx.xpath("ancestor::node()"))).to include("e")
    end
  end

  context "XML backend" do
    let(:doc) { Makiri::XML(%(<r><e a="1" b="2" c="3"><k/>t</e><s/></r>)) }
    let(:ctx) { doc.root.children.first.attribute_nodes.first }

    include_examples "attribute-axis conformance"

    it "still finds the element's attributes from the element itself" do
      el = doc.root.children.first
      expect(names(el.xpath("@*"))).to eq(%w[@a @b @c])
    end
  end

  context "HTML backend" do
    let(:doc) do
      Makiri::HTML(%(<html><body><e a="1" b="2" c="3"><k></k>t</e><s></s></body></html>))
    end
    let(:ctx) { doc.at_css("e").attribute_nodes.first }

    include_examples "attribute-axis conformance"
  end
end
