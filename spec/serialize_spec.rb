# frozen_string_literal: true

# M8: HTML serialization via Lexbor's serializer.
RSpec.describe "Makiri serialization" do
  let(:doc) do
    Makiri::HTML(%(<html><body><div id="d" class="c"><p>a</p><b>x</b></div></body></html>))
  end
  let(:div) { doc.at_css("#d") }

  describe "Node#to_html / #to_s / #outer_html" do
    it "serializes the node and its subtree (outer)" do
      expect(div.to_html).to eq('<div id="d" class="c"><p>a</p><b>x</b></div>')
    end

    it "aliases to_s and outer_html to outer HTML" do
      expect(div.to_s).to eq(div.to_html)
      expect(div.outer_html).to eq(div.to_html)
    end

    it "serializes a whole document from the document node" do
      expect(doc.to_html).to start_with("<html>")
      expect(doc.to_html).to include('<div id="d" class="c">')
    end

    it "returns UTF-8" do
      expect(div.to_html.encoding).to eq(Encoding::UTF_8)
    end

    it "escapes text content on output" do
      d = Makiri::HTML("<p>a &amp; b &lt;c&gt;</p>")
      expect(d.at_css("p").inner_html).to eq("a &amp; b &lt;c&gt;")
    end
  end

  describe "pretty printing" do
    it "indents nested elements with pretty: true" do
      out = div.to_html(pretty: true)
      expect(out).to include("\n")
      expect(out.lines.first).to eq("<div id=\"d\" class=\"c\">\n")
      # compact by default
      expect(div.to_html).not_to include("\n")
    end

    it "pretty-prints inner_html too" do
      expect(div.inner_html(pretty: true)).to include("\n")
    end
  end

  describe "Node#inner_html" do
    it "serializes children without the node's own tag" do
      expect(div.inner_html).to eq("<p>a</p><b>x</b>")
    end

    it "is empty for an empty element" do
      d = Makiri::HTML("<html><body><div></div></body></html>")
      expect(d.at_css("div").inner_html).to eq("")
    end
  end

  describe "round trip" do
    it "reparses to an equivalent subtree" do
      html = div.to_html
      reparsed = Makiri::HTML("<html><body>#{html}</body></html>")
      expect(reparsed.at_css("#d").to_html).to eq(html)
    end
  end

  describe "NodeSet#to_html / #text" do
    it "concatenates outer HTML of each node" do
      expect(doc.css("p, b").to_html).to eq("<p>a</p><b>x</b>")
    end

    it "concatenates text of each node" do
      expect(doc.css("p, b").text).to eq("ax")
    end

    it "is empty for an empty set" do
      expect(doc.css(".none").to_html).to eq("")
      expect(doc.css(".none").text).to eq("")
    end
  end

  describe "memory safety" do
    it "serializes correctly under GC stress" do
      GC.stress = true
      begin
        expect(div.to_html).to eq('<div id="d" class="c"><p>a</p><b>x</b></div>')
        GC.compact
        expect(div.inner_html).to eq("<p>a</p><b>x</b>")
      ensure
        GC.stress = false
      end
    end

    # The serialization buffer is capped at a multiple of the document's live size
    # (its Lexbor mraw pools), so a normal document serializes but a pathologically
    # deep CONSTRUCTED tree, whose pretty-printed indentation is super-linear in the
    # content, fails closed with Makiri::Error rather than growing without bound.
    it "bounds the serialization buffer, failing closed on a deep pretty-print" do
      doc = Makiri::HTML("<div/>")
      cur = doc.at_css("div")
      6000.times { e = doc.create_element("div"); cur.add_child(e); cur = e }
      expect(doc.at_css("body").inner_html.bytesize).to be < 200_000   # compact form is bounded
      expect { doc.at_css("body").to_html(pretty: true) }              # indentation would explode
        .to raise_error(Makiri::Error)
    end
  end
end
