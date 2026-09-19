# frozen_string_literal: true

# M6: source location. The parser records each element start-tag's byte offset
# (via a chained tokenizer callback) and resolves it to a 1-based line through
# the document's line table. Nodes the tracker cannot place report nil rather
# than a wrong line.
RSpec.describe "Makiri source location" do
  def find(node, name)
    return node if node.respond_to?(:name) && node.name == name
    return nil unless node.respond_to?(:children)

    node.children.each do |c|
      hit = find(c, name)
      return hit if hit
    end
    nil
  end

  describe "Node#line" do
    let(:doc) do
      Makiri::HTML(<<~HTML)
        <html>
        <body>
          <div id="main">
            <p>one</p>
            <p class="x">two</p>
          </div>
          <a href="/l">link</a>
        </body>
        </html>
      HTML
    end

    it "reports the line of explicit elements" do
      expect(find(doc.root, "html").line).to eq(1)
      expect(find(doc.root, "body").line).to eq(2)
      expect(find(doc.root, "div").line).to eq(3)
      expect(find(doc.root, "a").line).to eq(7)
    end

    it "distinguishes sibling elements on different lines" do
      ps = doc.xpath("//p")
      expect(ps.map(&:line)).to eq([4, 5])
    end

    it "locates nested same-name elements independently" do
      d = Makiri::HTML("<html><body>\n<div>\n<div>inner</div>\n</div></body></html>")
      expect(d.xpath("//div").map(&:line)).to eq([2, 3])
    end

    it "locates void / self-closing start tags" do
      d = Makiri::HTML("<html><body>\n\n<input name=q>\n</body></html>")
      expect(find(d.root, "input").line).to eq(3)
    end
  end

  # The offsets are recorded during the parse but stamped into the DOM lazily -
  # on the first `#line`, or on the first MUTATION, whichever comes first. The
  # mutation case is the one that needs pinning: the stamping walks the tree
  # pairing elements with tokens in document order, so a walk that ran AFTER an
  # edit would pair them wrongly, and `#line` must never answer a wrong line.
  describe "the deferred stamping" do
    SRC = <<~HTML
      <!doctype html>
      <html>
      <body>
      <div id="a">x</div>
      <p id="b">y</p>
      <span id="c">z</span>
      </body>
      </html>
    HTML

    def lines_of(doc)
      doc.css("div,p,span").map { |n| [n["id"], n.line] }
    end

    it "answers the same lines whether or not the tree was edited first" do
      untouched = lines_of(Makiri::HTML(SRC))
      expect(untouched).to eq([["a", 4], ["b", 5], ["c", 6]])

      inserted = Makiri::HTML(SRC)
      inserted.at_css("#a").add_child(inserted.create_element("b"))
      expect(lines_of(inserted)).to eq(untouched)

      renamed = Makiri::HTML(SRC)
      renamed.at_css("#b").name = "h1"
      expect(renamed.css("div,h1,span").map { |n| [n["id"], n.line] }).to eq(untouched)

      attributed = Makiri::HTML(SRC)
      attributed.at_css("#a")["class"] = "x"
      expect(lines_of(attributed)).to eq(untouched)
    end

    it "keeps the surviving nodes' lines after a removal" do
      doc = Makiri::HTML(SRC)
      doc.at_css("#a").remove
      expect(doc.css("p,span").map { |n| [n["id"], n.line] }).to eq([["b", 5], ["c", 6]])
    end

    it "still answers nil for a node that was never parsed" do
      doc = Makiri::HTML(SRC)
      made = doc.create_element("i")
      doc.at_css("#a").add_child(made)
      expect(made.line).to be_nil
    end

    it "is idempotent - asking twice gives the same answer" do
      doc = Makiri::HTML(SRC)
      first = lines_of(doc)
      expect(lines_of(doc)).to eq(first)
      doc.at_css("#a")["k"] = "v"
      expect(lines_of(doc)).to eq(first)
    end
  end

  describe "nodes without a recorded location" do
    let(:doc) { Makiri::HTML("<html><body><p>hi<!--c--></p></body></html>") }

    it "returns nil for parser-inserted implicit elements" do
      # No explicit <html> in the source -> the implicit one is unplaced.
      d = Makiri::HTML("<div>x</div>")
      expect(find(d.root, "html").line).to be_nil
      expect(find(d.root, "div").line).to eq(1)
    end

    it "returns nil for text and comment nodes" do
      p = find(doc.root, "p")
      text = p.children.find(&:text?)
      comment = p.children.find { |c| c.is_a?(Makiri::Comment) }
      expect(text.line).to be_nil
      expect(comment.line).to be_nil
    end

    it "returns nil for attribute nodes" do
      a = find(Makiri::HTML(%(<html><body><a href="/x">L</a></body></html>)).root, "a")
      expect(a.attribute_nodes.first.line).to be_nil
    end
  end

  describe "parsing still behaves" do
    it "produces an equivalent DOM to the plain parse path" do
      doc = Makiri::HTML("<html><body><div><p>a</p><p>b</p></div></body></html>")
      expect(doc.xpath("//p").map(&:text)).to eq(%w[a b])
      expect(find(doc.root, "div")["id"]).to be_nil
      expect(doc.title).to eq("")
    end

    it "handles empty and whitespace-only input" do
      expect(Makiri::HTML("").root).not_to be_nil
      expect(Makiri::HTML("   \n  ").root).not_to be_nil
    end
  end

  describe "memory safety", :gc_compact do
    it "stays correct under GC stress and compaction" do
      GC.stress = true
      begin
        doc = Makiri::HTML("<html><body>\n<p>x</p>\n<p>y</p></body></html>")
        lines = doc.xpath("//p").map(&:line)
        expect(lines).to eq([2, 3])
        GC.compact
        expect(doc.xpath("//p").map(&:line)).to eq([2, 3])
      ensure
        GC.stress = false
      end
    end

    it "survives many tracked parses being dropped" do
      gc_churn_iters(300).times do |i|
        d = Makiri::HTML("<html><body>\n<p id='p#{i}'>#{i}</p></body></html>")
        expect(d.at_xpath("//p").line).to eq(2)
      end
      GC.start
    end
  end
end
