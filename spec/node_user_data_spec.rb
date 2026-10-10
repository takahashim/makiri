# frozen_string_literal: true

# Node#user_data: one value per node, kept for the document's life.
RSpec.describe "Node#user_data" do
  shared_examples "a user data slot" do
    it "is nil until set, and nil forgets it" do
      expect(node.user_data).to be_nil
      expect(node.user_data = :w).to eq(:w)
      expect(node.user_data).to eq(:w)
      node.user_data = nil
      expect(node.user_data).to be_nil
    end

    it "is read through every navigation to the node" do
      node.user_data = "wrapper"
      expect(node.parent.children.find { |c| c == node }.user_data).to eq("wrapper")
    end

    it "keeps its value across GC and compaction" do
      node.user_data = Object.new.tap { |o| o.instance_variable_set(:@x, "kept") }
      GC.start
      GC.compact if GC.respond_to?(:compact)
      GC.start
      expect(node.user_data.instance_variable_get(:@x)).to eq("kept")
    end

    it "is per node, and a copy starts with none" do
      node.user_data = 1
      expect(node.parent.user_data).to be_nil
      expect(node.clone_node(true).user_data).to be_nil
    end

    it "is the Document's own on a Document" do
      doc.user_data = [:doc]
      expect(doc.user_data).to eq([:doc])
      expect(node.user_data).to be_nil
    end

    it "refuses a frozen node" do
      node.freeze
      expect { node.user_data = 1 }.to raise_error(FrozenError)
    end
  end

  context "with HTML" do
    let(:doc) { Makiri::HTML("<div><p>x</p></div>") }
    let(:node) { doc.at_css("p") }

    include_examples "a user data slot"

    it "follows a node moved within its document" do
      node.user_data = :moved
      doc.at_css("div").add_previous_sibling(node)
      expect(doc.at_css("p").user_data).to eq(:moved)
    end

    it "does not follow an import into another document" do
      node.user_data = :src
      other = Makiri::HTML("<main></main>")
      copy = other.at_css("main").add_child(node)
      expect(copy.user_data).to be_nil
    end
  end

  context "with XML" do
    let(:doc) { Makiri::XML("<r><e>x</e></r>") }
    let(:node) { doc.at_xpath("//e") }

    include_examples "a user data slot"
  end

  it "survives GC.stress with many nodes" do
    doc = Makiri::HTML("<ul>#{'<li>x</li>' * 200}</ul>")
    GC.stress = true
    begin
      doc.css("li").each_with_index { |li, i| li.user_data = "w#{i}" }
    ensure
      GC.stress = false
    end
    GC.compact if GC.respond_to?(:compact)
    expect(doc.css("li").map(&:user_data)).to eq(Array.new(200) { |i| "w#{i}" })
  end
end
