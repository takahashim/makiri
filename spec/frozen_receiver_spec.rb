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
end
