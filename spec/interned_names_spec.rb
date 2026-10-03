# frozen_string_literal: true

require "spec_helper"

# `interned_local_name` / `interned_namespace_uri` / `interned_tag_name` answer
# what their plain twins answer, as Ruby's own interned (fstring) Strings:
# frozen, the same object for the same name, and no allocation per read. The
# plain readers keep returning a fresh, unfrozen String (Nokogiri's contract).
RSpec.describe "interned name readers" do
  let(:readers) { %i[local_name namespace_uri tag_name] }

  # Every node under `root`, with each element's attributes.
  def all_nodes(root)
    out = []
    stack = [root]
    while (n = stack.pop)
      out << n
      out.concat(n.attribute_nodes.to_a) if n.element?
      stack.concat(n.children.to_a)
    end
    out
  end

  shared_examples "interned twins" do
    it "agree with the plain readers on every node" do
      nodes.each do |n|
        readers.each do |r|
          expect(n.public_send(:"interned_#{r}")).to eq(n.public_send(r)), "#{r} of #{n.inspect}"
        end
      end
    end

    it "are frozen UTF-8 and the same object for the same name" do
      readers.each do |r|
        v = element.public_send(:"interned_#{r}")
        expect(v).to be_frozen
        expect(v.encoding).to eq(Encoding::UTF_8)
        expect(v).to equal(twin.public_send(:"interned_#{r}"))
        expect(v).to equal(-v.dup)
      end
    end

    it "leave the plain readers unfrozen" do
      readers.each { |r| expect(element.public_send(r)).not_to be_frozen }
    end

    it "allocate nothing once the name is interned" do
      element.interned_local_name
      before = GC.stat(:total_allocated_objects)
      100.times do
        element.interned_local_name
        element.interned_namespace_uri
        element.interned_tag_name
      end
      expect(GC.stat(:total_allocated_objects) - before).to be < 10
    end
  end

  context "HTML" do
    let(:doc) { Makiri::HTML(%(<div id="a"><p>t</p><!--c--></div><div></div><svg><foreignObject xlink:href="#x"/></svg>)) }
    let(:element) { doc.css("div")[0] }
    let(:twin) { doc.css("div")[1] }
    let(:nodes) { all_nodes(doc.root) }

    include_examples "interned twins"

    it "keeps a foreign element's case" do
      fo = doc.at_css("svg").child
      expect(fo.interned_local_name).to eq("foreignObject")
      expect(fo.interned_tag_name).to eq("foreignObject")
    end
  end

  context "XML" do
    let(:doc) { Makiri::XML(%(<p:r xmlns:p="urn:x" a="1"><p:c/><p:c/>t<!--c--><d/></p:r>)) }
    let(:element) { doc.root.children[0] }
    let(:twin) { doc.root.children[1] }
    let(:nodes) { all_nodes(doc.root) }

    include_examples "interned twins"

    it "answers nil where the plain readers do" do
      text = doc.root.children.find(&:text?)
      expect(text.interned_local_name).to be_nil
      expect(doc.root.at_xpath("d").interned_namespace_uri).to be_nil
    end
  end
end
