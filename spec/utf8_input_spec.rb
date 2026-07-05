# frozen_string_literal: true

# Text-input contract: Makiri accepts UTF-8. HTML parsing decodes leniently like
# a browser (invalid bytes -> U+FFFD, parse never fails); the programmatic
# query/mutation APIs require valid UTF-8 with no NUL byte and raise otherwise.
RSpec.describe "UTF-8 text-input contract" do
  NUL = "\u0000"
  INVALID = "\xC3\x28" # 0xC3 then 0x28: not a valid UTF-8 sequence

  describe "HTML parsing (browser-compatible, never raises)" do
    it "replaces invalid UTF-8 with U+FFFD and yields valid text" do
      doc = Makiri::HTML("<p>a#{INVALID}b</p>".dup.force_encoding("BINARY"))
      text = doc.at_xpath("//p").text
      expect(text).to be_valid_encoding
      expect(text).to eq("a�(b") # 0xC3 -> U+FFFD, 0x28 '(' kept
    end

    it "drops a NUL byte in body text (per the HTML5 tokenizer)" do
      doc = Makiri::HTML("<p>a#{NUL}b</p>")
      expect(doc.at_xpath("//p").text).to eq("ab")
    end

    it "transcodes a large invalid-UTF-8 input (exercises buffer growth)" do
      # ~80k lone lead bytes: each is invalid and becomes U+FFFD, so the output
      # is ~3x the input -> forces the sanitiser's buffer to grow and crosses its
      # internal decode/encode chunk boundaries. Must stay memory-clean (ASan).
      src = ("<p>" + ("\xC3" * 80_000) + "</p>").dup.force_encoding("BINARY")
      text = Makiri::HTML(src).at_xpath("//p").text
      expect(text).to be_valid_encoding
      expect(text.chars.uniq).to eq(["�"])
      expect(text.length).to eq(80_000)
    end

    it "leaves well-formed UTF-8 unchanged" do
      expect(Makiri::HTML("<p>héllo 日本語</p>").at_xpath("//p").text).to eq("héllo 日本語")
    end

    it "decodes invalid UTF-8 in a parsed fragment too" do
      frag = Makiri::DocumentFragment.parse("<i>#{INVALID}</i>".dup.force_encoding("BINARY"))
      expect(frag.at_css("i").text).to be_valid_encoding
    end
  end

  describe "input encoding is honoured (transcoded to UTF-8)" do
    jp = "日本語テスト"

    it "transcodes a Shift_JIS document so its content survives" do
      html = "<html><body><p>#{jp}</p></body></html>".encode("Shift_JIS")
      txt = Makiri::HTML(html).at_css("p").text
      expect(txt).to be_valid_encoding
      expect(txt).to eq(jp)
    end

    it "transcodes an EUC-JP document" do
      html = "<html><body><p>#{jp}</p></body></html>".encode("EUC-JP")
      expect(Makiri::HTML(html).at_css("p").text).to eq(jp)
    end

    it "transcodes a single-byte legacy encoding (ISO-8859-1)" do
      html = "<html><body><p>caf\xE9</p></body></html>".dup.force_encoding("ISO-8859-1")
      expect(Makiri::HTML(html).at_css("p").text).to eq("café")
    end

    it "leaves UTF-8 / US-ASCII / binary input as-is" do
      expect(Makiri::HTML("<p>#{jp}</p>").at_css("p").text).to eq(jp)
      expect(Makiri::HTML("<p>plain</p>".encode("US-ASCII")).at_css("p").text).to eq("plain")
      expect(Makiri::HTML("<p>#{jp}</p>".dup.b).at_css("p").text).to eq(jp)
    end

    it "honours the encoding in fragment parses too" do
      frag = Makiri::DocumentFragment.parse("<p>#{jp}</p>".encode("Shift_JIS"))
      expect(frag.at_css("p").text).to eq(jp)
      doc = Makiri::HTML("<html><body></body></html>")
      expect(doc.fragment("<p>#{jp}</p>".encode("EUC-JP")).at_css("p").text).to eq(jp)
    end

    it "replaces bytes that don't round-trip with U+FFFD, never raising" do
      html = "<p>\x80</p>".dup.force_encoding("ISO-8859-1")
      expect(Makiri::HTML(html).at_css("p").text).to be_valid_encoding
    end
  end

  describe "programmatic APIs (strict: valid UTF-8, no NUL)" do
    let(:doc) { Makiri::HTML("<div id='d'>x</div>") }
    let(:el)  { doc.at_css("div") }

    # [boundary description, callable]. NUL is rejected for NAMES and engine
    # inputs (below); it is ALLOWED for HTML data-family content (see the separate
    # "data-family content accepts a NUL" block). Invalid UTF-8 is rejected
    # everywhere, including the relaxed data-family sites.
    {
      "attribute value (invalid UTF-8)" => -> (d, e) { e["k"] = INVALID.dup.force_encoding("BINARY") },
      "attribute name (NUL)"            => -> (d, e) { e["a#{NUL}b"] = "v" },
      "element content (invalid UTF-8)" => -> (d, e) { e.content = INVALID.dup.force_encoding("BINARY") },
      "create_element (invalid UTF-8)"  => -> (d, e) { d.create_element(INVALID.dup.force_encoding("BINARY")) },
      "create_text_node (invalid UTF-8)" => -> (d, e) { d.create_text_node(INVALID.dup.force_encoding("BINARY")) },
      "create_comment (invalid UTF-8)"  => -> (d, e) { d.create_comment(INVALID.dup.force_encoding("BINARY")) },
      "name= (NUL)"                     => -> (d, e) { e.name = "a#{NUL}b" },
      "css selector (NUL)"              => -> (d, e) { d.css("div#{NUL}") },
      "css selector (invalid UTF-8)"    => -> (d, e) { d.css(INVALID.dup.force_encoding("BINARY")) },
      "xpath expression (NUL)"          => -> (d, e) { d.xpath("//div#{NUL}") },
      "xpath expression (invalid UTF-8)" => -> (d, e) { d.xpath("//#{INVALID}".dup.force_encoding("UTF-8")) },
      "attribute read [] (NUL)"         => -> (d, e) { e["a#{NUL}b"] },
      "attribute read [] (invalid UTF-8)" => -> (d, e) { e[INVALID.dup.force_encoding("BINARY")] },
      "key? (NUL)"                      => -> (d, e) { e.key?("a#{NUL}b") },
      "fragment context (NUL)"          => -> (d, e) { Makiri::DocumentFragment.parse("<p>x</p>", context: "a#{NUL}b") },
      "fragment context (invalid UTF-8)" => -> (d, e) { d.fragment("<p>x</p>", context: INVALID.dup.force_encoding("BINARY")) },
    }.each do |desc, body|
      it "rejects #{desc} with Makiri::Error" do
        expect { body.call(doc, el) }.to raise_error(Makiri::Error)
      end
    end

    # DOM data-family (text/comment content, attribute values) accepts U+0000,
    # matching the HTML DOM / browsers; the bytes round-trip verbatim. Names and
    # engine inputs stay NUL-strict (asserted above), and XML rejects NUL
    # entirely (see xml_build_spec).
    describe "data-family content accepts a NUL (HTML DOM conformance)" do
      it "keeps a NUL in an attribute value" do
        el["k"] = "a#{NUL}b"
        expect(el["k"].bytesize).to eq(3)
      end

      it "keeps a NUL in element content" do
        el.content = "a#{NUL}b"
        expect(el.text.bytesize).to eq(3)
      end

      it "keeps a NUL in create_text_node and create_comment" do
        expect(doc.create_text_node("a#{NUL}b").text.bytesize).to eq(3)
        expect(doc.create_comment("a#{NUL}b").to_html).to eq("<!--a#{NUL}b-->")
      end
    end

    it "rejects an invalid-UTF-8 XPath variable value" do
      ctx = Makiri::XPathContext.new(doc)
      expect { ctx.register_variable("v", INVALID.dup.force_encoding("BINARY")) }
        .to raise_error(Makiri::Error)
    end

    it "rejects a NUL in a registered namespace prefix" do
      ctx = Makiri::XPathContext.new(doc)
      expect { ctx.register_namespace("a#{NUL}", "urn:x") }.to raise_error(Makiri::Error)
    end

    it "accepts valid multibyte UTF-8 everywhere" do
      el["café"] = "néﬁ"
      expect(el["café"]).to eq("néﬁ")
      el.content = "日本語"
      expect(el.text).to eq("日本語")
      expect(doc.css("div").length).to eq(1)
      expect(doc.xpath("//div").length).to eq(1)
    end
  end
end
