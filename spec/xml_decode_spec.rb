# frozen_string_literal: true

require "spec_helper"

# Strict input decode for the XML reader (§2.1): honours the String's declared
# encoding like the HTML path, but fail-closed — invalid UTF-8, an undecodable
# byte, or an embedded NUL all raise Makiri::XML::SyntaxError (no U+FFFD repair).
# Makiri::XML.__decode is the internal hook exercising this until the full
# Makiri::XML(...) parse pipeline lands.
RSpec.describe "Makiri::XML.__decode (strict input decode)" do
  def decode(str) = Makiri::XML.__decode(str)

  it "passes valid UTF-8 through, tagged UTF-8" do
    out = decode("<a>café</a>")
    expect(out).to eq("<a>café</a>")
    expect(out.encoding).to eq(Encoding::UTF_8)
  end

  it "transcodes other encodings to UTF-8 (content preserved)" do
    sjis = "\x93\xfa\x96{".dup.force_encoding("Shift_JIS") # 日本
    out = decode(sjis)
    expect(out.encoding).to eq(Encoding::UTF_8)
    expect(out).to eq("日本")
  end

  it "raises on invalid UTF-8 (no U+FFFD substitution)" do
    expect { decode("a\xFF\xFEb".b) }.to raise_error(Makiri::XML::SyntaxError, /UTF-8/)
  end

  it "raises on an undecodable byte in a declared encoding" do
    bad = "\x80".dup.force_encoding("US-ASCII") # 0x80 is not valid US-ASCII...
    # US-ASCII passes through to the strict UTF-8 validator, which rejects it.
    expect { decode(bad) }.to raise_error(Makiri::XML::SyntaxError)
  end

  it "raises on an embedded NUL" do
    expect { decode("a\x00b") }.to raise_error(Makiri::XML::SyntaxError, /NUL/)
  end

  it "Makiri::XML::SyntaxError is a Makiri::Error" do
    expect(Makiri::XML::SyntaxError.ancestors).to include(Makiri::Error)
    expect(Makiri::XML::LimitExceeded.ancestors).to include(Makiri::Error)
  end
end
