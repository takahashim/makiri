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

  describe "programmatic APIs (strict: valid UTF-8, no NUL)" do
    let(:doc) { Makiri::HTML("<div id='d'>x</div>") }
    let(:el)  { doc.at_css("div") }

    # [boundary description, callable]
    {
      "attribute value (invalid UTF-8)" => -> (d, e) { e["k"] = INVALID.dup.force_encoding("BINARY") },
      "attribute value (NUL)"           => -> (d, e) { e["k"] = "a#{NUL}b" },
      "attribute name (NUL)"            => -> (d, e) { e["a#{NUL}b"] = "v" },
      "element content (NUL)"           => -> (d, e) { e.content = "a#{NUL}b" },
      "element content (invalid UTF-8)" => -> (d, e) { e.content = INVALID.dup.force_encoding("BINARY") },
      "create_element (invalid UTF-8)"  => -> (d, e) { d.create_element(INVALID.dup.force_encoding("BINARY")) },
      "create_text_node (NUL)"          => -> (d, e) { d.create_text_node("a#{NUL}b") },
      "create_comment (NUL)"            => -> (d, e) { d.create_comment("a#{NUL}b") },
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
