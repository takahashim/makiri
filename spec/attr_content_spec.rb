# frozen_string_literal: true

require "spec_helper"

# An Attr's content= is its value, in HTML and XML alike: the owner reads the
# new value, only attribute_version moves, and a frozen Attr refuses before
# anything changes.
RSpec.describe "Attr#content=" do
  {
    "HTML" => -> { Makiri::HTML(%(<p a="1"><b>x</b></p>)).then { |d| [d, d.at_css("p")] } },
    "XML" => -> { Makiri::XML(%(<p a="1"><b>x</b></p>)).then { |d| [d, d.root] } },
  }.each do |kind, make|
    context kind do
      let(:pair) { make.call }
      let(:doc) { pair[0] }
      let(:owner) { pair[1] }
      let(:attr) { owner.attribute_nodes.find { |a| a.name == "a" } }

      it "sets the value its owner reads" do
        attr.content = "z"
        expect([attr.value, owner["a"]]).to eq(%w[z z])
      end

      it "moves attribute_version and leaves tree_version" do
        before = [doc.tree_version, doc.attribute_version]
        attr.content = "z"
        expect(doc.tree_version).to eq(before[0])
        expect(doc.attribute_version).to be > before[1]
      end

      # An Attr's edit changes its owner's attribute list, so a frozen owner
      # refuses it as it refuses `delete`.
      it "refuses content= and remove when the owner element is frozen" do
        before = [doc.tree_version, doc.attribute_version]
        owner.freeze
        expect { attr.content = "z" }.to raise_error(FrozenError)
        expect { attr.remove }.to raise_error(FrozenError)
        expect([owner["a"], doc.tree_version, doc.attribute_version]).to eq(["1", *before])
      end

      it "refuses on a frozen Attr, changing nothing" do
        before = [doc.tree_version, doc.attribute_version]
        attr.freeze
        expect { attr.content = "z" }.to raise_error(FrozenError)
        expect([owner["a"], doc.tree_version, doc.attribute_version]).to eq(["1", *before])
      end
    end
  end

  context "XML namespace declarations" do
    it "holds a declaration's new value to the declaration rules" do
      doc = Makiri::XML(%(<r xmlns:p="urn:p"/>))
      decl = doc.root.attribute_nodes.find { |a| a.name == "xmlns:p" }
      expect { decl.content = "" }.to raise_error(Makiri::Error, /not permitted/)
      expect(doc.root["xmlns:p"]).to eq("urn:p")
    end

    it "rebinds as []= does: decided nodes keep their namespace" do
      src = %(<r xmlns:p="urn:p" p:b="2"><p:c/></r>)
      via_attr = Makiri::XML(src)
      via_attr.root.attribute_nodes.find { |a| a.name == "xmlns:p" }.content = "urn:q"
      via_aset = Makiri::XML(src)
      via_aset.root["xmlns:p"] = "urn:q"
      expect(via_attr.to_xml).to eq(via_aset.to_xml)
      expect(via_attr.root.children.first.namespace_uri).to eq("urn:p")
    end
  end
end
