# frozen_string_literal: true

# A mutator on a frozen node fails with the FrozenError Ruby's own check gives:
# the message names the receiver and `#receiver` is set. The check returns the
# error rather than raising it through the extension's frames, and that must not
# thin the exception out.
RSpec.describe "FrozenError from a frozen node's mutators" do
  def frozen_error(node)
    node.freeze
    yield node
    nil
  rescue FrozenError => e
    e
  end

  shared_examples "Ruby's own FrozenError" do
    it "names the receiver and sets #receiver" do
      e = frozen_error(node) { |n| mutate.(n) }
      expect(e).to be_a(FrozenError)
      expect(e.message).to eq("can't modify frozen #{node.class}: #{node.inspect}")
      expect(e.receiver).to equal(node)
    end
  end

  context "an XML element, setting an attribute" do
    let(:node) { Makiri::XML("<r a='1'><c/></r>").root }
    let(:mutate) { ->(n) { n["a"] = "2" } }
    include_examples "Ruby's own FrozenError"
  end

  context "an XML element, removing it" do
    let(:node) { Makiri::XML("<r><c/></r>").root.children.first }
    let(:mutate) { ->(n) { n.remove } }
    include_examples "Ruby's own FrozenError"
  end

  context "an HTML element, setting an attribute" do
    let(:node) { Makiri::HTML("<p id='x'>t</p>").at_css("p") }
    let(:mutate) { ->(n) { n["id"] = "y" } }
    include_examples "Ruby's own FrozenError"
  end

  context "an HTML element, adding a child" do
    let(:node) { Makiri::HTML("<p>t</p>").at_css("p") }
    let(:mutate) { ->(n) { n.add_child(n.document.create_element("b")) } }
    include_examples "Ruby's own FrozenError"
  end

  it "leaves an unfrozen node mutable" do
    node = Makiri::XML("<r a='1'/>").root
    node["a"] = "2"
    expect(node["a"]).to eq("2")
  end

  # The contract is "a mutator on a frozen node raises FrozenError", and it used
  # to hold only for the RECEIVER. Inserting a node relinks the node PASSED too -
  # its parent, prev and next all change, and an adoption takes it out of its own
  # document - so the same effect on the same node raised through `a.remove` and
  # did not through `b.add_child(a)`.
  describe "a frozen node passed as the argument" do
    it "refuses a move within one XML document" do
      doc = Makiri::XML("<r><a/><b/></r>")
      a, b = doc.root.children[0], doc.root.children[1]
      a.freeze
      %i[add_child add_previous_sibling add_next_sibling replace].each do |verb|
        expect { b.public_send(verb, a) }.to raise_error(FrozenError)
      end
      expect(doc.root.to_xml).to eq("<r><a/><b/></r>") # nothing moved
    end

    it "refuses an adoption out of another XML document" do
      src = Makiri::XML("<s><m/></s>")
      dst = Makiri::XML("<d/>")
      m = src.root.children.first
      m.freeze
      expect { dst.root.add_child(m) }.to raise_error(FrozenError)
      expect(src.root.to_xml).to eq("<s><m/></s>") # the source is intact
    end

    it "refuses a move within one HTML document" do
      doc = Makiri::HTML("<div><p>x</p><span>y</span></div>")
      para = doc.at_css("p")
      para.freeze
      expect { doc.at_css("span").add_child(para) }.to raise_error(FrozenError)
      expect(doc.at_css("div").inner_html).to eq("<p>x</p><span>y</span>")
    end

    it "still moves a node that is not frozen" do
      doc = Makiri::XML("<r><a/><b/></r>")
      a, b = doc.root.children[0], doc.root.children[1]
      b.add_child(a)
      expect(doc.root.to_xml).to eq("<r><b><a/></b></r>")
    end

    # The reach is the nodes the caller NAMED. A fragment's children move too and
    # cannot be checked: frozenness is a property of a Ruby object, and the arena
    # keeps no map from a node back to its wrapper - so a child the caller never
    # named is out of reach by construction. Pinned so the boundary is a decision
    # rather than a surprise.
    it "does not reach the children of a fragment argument" do
      doc = Makiri::XML("<r/>")
      frag = doc.fragment("<f/>")
      doc.root.add_child(frag)
      expect(doc.root.children.map(&:name)).to eq(%w[f])
    end
  end
end
