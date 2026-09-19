# frozen_string_literal: true

# Browser-compatible UTF-8 input sanitisation (dom_adapter/utf8_input.c, or its
# Rust port): every invalid sequence becomes U+FFFD, per WHATWG byte-stream
# decoding, so parsing NEVER fails on bad bytes and the DOM is always valid
# UTF-8.
#
# The Rust port replaced Lexbor's decode/encode pipeline with the standard
# library's lossy decode. That is only sound if the two agree on where one
# replacement character ends and the next begins - lossy decoders genuinely
# differ there (`E1 80 41` is one U+FFFD plus "A" under the maximal-subpart rule
# and two U+FFFD under a byte-at-a-time one). The enumeration below is the
# standing check that the shipped build follows the maximal-subpart rule.
#
# The bytes are read back through a comment node: the HTML tokenizer copies
# comment data through verbatim, so what comes out is the sanitiser's output.
# Six bytes cannot travel that channel and are excluded rather than asserted on
# - NUL (the tokenizer replaces it), CR (it normalises CR/CRLF to LF, which an
# earlier version of this check mistook for a sanitiser mismatch), and the four
# that end or reopen a comment.
RSpec.describe "UTF-8 sanitiser: the maximal-subpart rule" do
  UNSENDABLE_BYTES = [0x00, 0x0d, 0x26, 0x2d, 0x3c, 0x3e].freeze

  # What the sanitiser made of these bytes.
  def sanitized(bytes)
    doc = Makiri::HTML("<!--#{bytes.pack("C*")}-->")
    doc.children.find { |n| n.is_a?(Makiri::HTML::Comment) }&.content
  end

  def sendable?(bytes) = bytes.none? { |b| UNSENDABLE_BYTES.include?(b) }

  # The reference: Ruby's own scrub, which follows the same maximal-subpart rule.
  def reference(bytes)
    bytes.pack("C*").force_encoding("UTF-8").scrub("\u{FFFD}")
  end

  def expect_agreement(bytes, label)
    return unless sendable?(bytes)

    got = sanitized(bytes)
    expect(got).to eq(reference(bytes)), "#{label}: #{bytes.inspect}"
    expect(got.encoding.name).to eq("UTF-8")
    expect(got.valid_encoding?).to be(true), "#{label} produced invalid UTF-8"
  end

  it "passes valid UTF-8 through unchanged" do
    ["", "plain", "café", "üñî", "日本語", "\u{1F4A9}", "a\u{FFFD}b"].each do |s|
      bytes = s.b.bytes
      next unless sendable?(bytes)

      expect(sanitized(bytes)).to eq(s) unless s.empty?
    end
  end

  it "agrees with the maximal-subpart rule on every 1-byte input" do
    (1..255).each { |b| expect_agreement([b], "1-byte") }
  end

  it "agrees on every 2-byte sequence from every non-ASCII lead" do
    (0x80..0xFF).each do |lead|
      [0x00, 0x41, 0x7F, 0x80, 0x8F, 0x9F, 0xA0, 0xBF, 0xC0, 0xFF].each do |b2|
        expect_agreement([lead, b2], "2-byte")
      end
    end
  end

  it "agrees on the 3-byte leads against the continuation boundaries" do
    (0xE0..0xEF).each do |lead|
      [0x80, 0x9F, 0xA0, 0xBF, 0x41, 0xC0].each do |b2|
        [0x80, 0xBF, 0x41, 0xC0].each { |b3| expect_agreement([lead, b2, b3], "3-byte") }
      end
    end
  end

  it "agrees on the 4-byte leads against the continuation boundaries" do
    (0xF0..0xF7).each do |lead|
      [0x80, 0x8F, 0x90, 0xBF, 0x41].each do |b2|
        [0x80, 0xBF, 0x41].each do |b3|
          [0x80, 0xBF, 0x41].each { |b4| expect_agreement([lead, b2, b3, b4], "4-byte") }
        end
      end
    end
  end

  # The landmarks, each alone and in the four contexts that move the boundary:
  # followed by ASCII, followed by a continuation byte, preceded by ASCII, and
  # doubled. A local rather than a constant: spec/utf8_input_spec.rb already
  # defines top-level constants, and a second file adding more would warn.
  landmarks = {
    "overlong 2-byte (lowest)" => [0xC0, 0x80],
    "overlong 2-byte (highest)" => [0xC1, 0xBF],
    "overlong 3-byte (lowest)" => [0xE0, 0x80, 0x80],
    "overlong 3-byte (highest)" => [0xE0, 0x9F, 0xBF],
    "overlong 4-byte (lowest)" => [0xF0, 0x80, 0x80, 0x80],
    "overlong 4-byte (highest)" => [0xF0, 0x8F, 0xBF, 0xBF],
    "surrogate (lowest)" => [0xED, 0xA0, 0x80],
    "surrogate (highest)" => [0xED, 0xBF, 0xBF],
    "beyond U+10FFFF" => [0xF4, 0x90, 0x80, 0x80],
    "truncated 3-byte" => [0xE1, 0x80],
    "truncated 4-byte (one continuation)" => [0xF1, 0x80],
    "truncated 4-byte (two continuations)" => [0xF1, 0x80, 0x80],
    "valid 3-byte" => [0xE2, 0x82, 0xAC],
    "valid 4-byte" => [0xF0, 0x9F, 0x92, 0xA9],
  }.freeze

  it "agrees on each landmark in every boundary-moving context" do
    landmarks.each do |label, base|
      expect_agreement(base, label)
      expect_agreement(base + [0x41], "#{label} + ASCII")
      expect_agreement(base + [0x80], "#{label} + continuation")
      expect_agreement([0x41] + base, "ASCII + #{label}")
      expect_agreement(base + base, "#{label} doubled")
    end
  end

  it "agrees on random byte strings" do
    rng = Random.new(20_260_913)
    2_000.times do
      bytes = Array.new(rng.rand(1..24)) { rng.rand(1..255) }
      expect_agreement(bytes, "random")
    end
  end

  # The guarantee the whole subsystem exists for.
  it "never fails to parse, whatever the bytes" do
    rng = Random.new(7)
    500.times do
      src = Array.new(rng.rand(1..64)) { rng.rand(0..255) }.pack("C*")
      doc = nil
      expect { doc = Makiri::HTML("<p>#{src}</p>") }.not_to raise_error
      expect(doc.text.valid_encoding?).to be(true)
    end
  end

  it "keeps attribute values valid after sanitisation" do
    doc = Makiri::HTML("<div a=\"x\xC3!y\" b=\"\xF0\x9F\">t</div>")
    el = doc.at_css("div")
    expect(el["a"].valid_encoding?).to be(true)
    expect(el["b"].valid_encoding?).to be(true)
    expect(el["a"]).to include("\u{FFFD}")
  end
end
