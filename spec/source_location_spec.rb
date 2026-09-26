# frozen_string_literal: true

# M6: source location. Each element is stamped with its start tag's byte offset
# as the tree builder creates it (via a chained tokenizer callback), and the
# offset resolves to a 1-based line through the document's line table. Nodes the tracker cannot place report nil rather
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

  # The offsets are stamped as the parser creates each element, so a mutation
  # needs no special ordering: nothing is paired up after the parse, and an
  # edit cannot make a later #line match an element to the wrong tag.
  describe "lines after a mutation" do
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

  # Every element answers the line of its own start tag or nil. Each
  # expectation lists every element in order, so a wrong line anywhere fails -
  # the old post-parse matching gave parser-created elements a LATER tag's line.
  describe "elements the parser creates or moves" do
    def lines(html)
      Makiri::HTML(html).css("body *").map { |n| [n.name, n.line] }
    end

    it "leaves an implied tbody nil and does not let it steal a later tbody's line" do
      html = "<table>\n<tr><td>1</td></tr>\n</table>\n<table>\n<tbody>\n<tr><td>2</td></tr></tbody></table>"
      expect(lines(html)).to eq([
        ["table", 1], ["tbody", nil], ["tr", 2], ["td", 2],
        ["table", 4], ["tbody", 5], ["tr", 6], ["td", 6]
      ])
    end

    it "leaves a formatting element the adoption agency recreates nil" do
      html = "<b>\n<p>x</b>\ny</p>\n\n\n<b>z</b>"
      expect(lines(html)).to eq([["b", 1], ["p", 2], ["b", nil], ["b", 6]])
    end

    it "never answers a wrong line under foster parenting" do
      expect(lines("<table>\n<p>x</p>\n<tr><td>1</td></tr></table>"))
        .to eq([["p", 2], ["table", 1], ["tbody", nil], ["tr", 3], ["td", 3]])
      # A fostered VOID element lands before the table, at neither place the
      # stamping looks, so it is nil rather than guessed.
      expect(lines("<table>\n<input>\n<tr><td>1</td></tr></table>"))
        .to eq([["input", nil], ["table", 1], ["tbody", nil], ["tr", 3], ["td", 3]])
    end

    it "locates void elements and self-closing foreign elements" do
      html = "<div>\n<br>\n<img src=x>\n<svg>\n<path/>\n</svg><p>a\n<hr>\n</div>"
      expect(lines(html)).to eq([
        ["div", 1], ["br", 2], ["img", 3], ["svg", 4], ["path", 5], ["p", 6], ["hr", 7]
      ])
    end

    it "locates the contents of a template" do
      doc = Makiri::HTML("<template>\n<i>t</i>\n<img>\n<p>\n</template>")
      tpl = doc.at_css("template")
      expect(tpl.line).to eq(1)
      expect(tpl.content_fragment.css("*").map { |n| [n.name, n.line] })
        .to eq([["i", 2], ["img", 3], ["p", 4]])
    end

    it "answers for every element of a large document (no token cap)" do
      n = 200_000
      doc = Makiri::HTML((1..n).map { |i| "<p>#{i}</p>" }.join("\n"))
      ps = doc.css("p")
      expect(ps.size).to eq(n)
      expect(ps.each_with_index.all? { |p, i| p.line == i + 1 }).to be(true)
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

  # Lexbor's import copies the node's source offset, which indexes the OTHER
  # document's text: an imported <div> from line 41 answered 22 here, a line it
  # was never on - and only when the source had been asked for a line first.
  describe "a node copied from another document" do
    let(:src) { Makiri.HTML(("<p>x</p>\n" * 40) + "<div id=far>f<i id=inner>i</i></div>") }
    let(:dst) { Makiri.HTML("<p>1</p>\n<p id=t>2</p>" + ("\n<i>pad</i>" * 20)) }

    before { expect(src.at_css("#far").line).to eq(41) } # the source has a line

    it "has no line after import_node, at any depth" do
      imported = dst.import_node(src.at_css("#far"), true)
      dst.at_css("#t").add_child(imported)
      expect(imported.line).to be_nil
      expect(imported.at_css("#inner").line).to be_nil
    end

    it "has no line after being adopted by insertion" do
      dst.at_css("#t").add_child(src.at_css("#far"))
      expect(dst.css("#far").last.line).to be_nil
    end

    it "keeps the line of a copy made within the same document" do
      expect(dst.at_css("#t").clone_node(true).line).to eq(2)
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
