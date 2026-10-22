# frozen_string_literal: true

require "spec_helper"

# A document parse decodes its bytes as the HTML Standard's encoding sniffing
# algorithm decides, with the Encoding Standard's decoders
# (ext/makiri/rust/src/html/encoding.rs). The engine's own tests cover the
# prescan step by step; these hold the Ruby entry points to it.
RSpec.describe "HTML document encoding" do
  JA = "<p>日本語のテキスト、ひらがなとカタカナ。漢字も含む文書です。</p>"

  def text_of(doc) = doc.at_css("p")&.text

  describe "a binary String, which declares nothing" do
    it "reads a <meta charset> in the first 1024 bytes" do
      doc = Makiri::HTML(%(<meta charset="shift_jis">#{JA}).encode("Shift_JIS").b)
      expect([doc.encoding, text_of(doc)]).to eq(["Shift_JIS", JA[3...-4]])
    end

    it "reads <meta http-equiv=Content-Type content=...>" do
      src = %(<meta http-equiv="Content-Type" content="text/html; charset=EUC-JP">#{JA})
      expect(Makiri::HTML(src.encode("EUC-JP").b).encoding).to eq("EUC-JP")
    end

    it "takes valid UTF-8 as UTF-8" do
      doc = Makiri::HTML(JA.b)
      expect([doc.encoding, text_of(doc)]).to eq(["UTF-8", JA[3...-4]])
      expect(Makiri::HTML("<p>plain</p>".b).encoding).to eq("UTF-8")
    end

    it "detects a legacy encoding when nothing declares one" do
      %w[Shift_JIS EUC-JP ISO-2022-JP].each do |name|
        doc = Makiri::HTML(JA.encode(name).b)
        expect([doc.encoding, text_of(doc)]).to eq([name, JA[3...-4]]), name
      end
    end

    it "re-reads the document in the encoding a later <meta> names" do
      src = "<!--#{"x" * 1100}--><meta charset=euc-jp>#{JA}"
      doc = Makiri::HTML(src.encode("EUC-JP").b)
      expect([doc.encoding, text_of(doc)]).to eq(["EUC-JP", JA[3...-4]])
    end
  end

  describe "a String with an encoding, which declares it" do
    it "reads it with the Encoding Standard decoder for that encoding" do
      expect(text_of(Makiri::HTML(JA.encode("Windows-31J")))).to eq(JA[3...-4])
      # ISO-8859-1 is windows-1252 in the Encoding Standard: 0x80 is the euro.
      doc = Makiri::HTML("<p>caf\xE9 \x80</p>".dup.force_encoding("ISO-8859-1"))
      expect([doc.encoding, text_of(doc)]).to eq(["windows-1252", "café €"])
    end

    it "is certain: a <meta> does not change it" do
      doc = Makiri::HTML(%(<meta charset="euc-jp"><p>é</p>))
      expect([doc.encoding, text_of(doc)]).to eq(%w[UTF-8 é])
    end

    it "transcodes one the Encoding Standard has no label for through Ruby" do
      doc = Makiri::HTML("<p>é</p>".encode("UTF-32LE"))
      expect([doc.encoding, text_of(doc)]).to eq(%w[UTF-8 é])
    end
  end

  describe "encoding:" do
    it "is the transport layer's encoding, over the String's own" do
      doc = Makiri::HTML(JA.encode("EUC-JP").b, encoding: "euc-jp")
      expect([doc.encoding, text_of(doc)]).to eq(["EUC-JP", JA[3...-4]])
      expect(Makiri::HTML(JA.encode("EUC-JP").b, encoding: Encoding::EUC_JP).encoding).to eq("EUC-JP")
      expect(Makiri::HTML("<p>x</p>", encoding: :latin1).encoding).to eq("windows-1252")
    end

    it "refuses a label the Encoding Standard does not know" do
      expect { Makiri::HTML("<p>x</p>", encoding: "utf-7") }.to raise_error(ArgumentError, /unknown encoding/)
    end
  end

  describe "a byte order mark" do
    it "wins over everything, and is not part of the text" do
      doc = Makiri::HTML("\xEF\xBB\xBF<p>x</p>".b, encoding: "shift_jis")
      expect([doc.encoding, doc.at_css("body").inner_html]).to eq(["UTF-8", "<p>x</p>"])
      utf16 = Makiri::HTML("﻿<p>あ</p>".encode("UTF-16LE").b)
      expect([utf16.encoding, text_of(utf16)]).to eq(%w[UTF-16LE あ])
    end
  end

  it "is UTF-8 for a document made rather than parsed, and kept by #dup" do
    expect(Makiri::HTML::Document.new.encoding).to eq("UTF-8")
    expect(Makiri::HTML(JA.encode("Shift_JIS").b).dup.encoding).to eq("Shift_JIS")
  end

  it "does not touch fragment parsing, which takes text" do
    frag = Makiri::HTML::DocumentFragment.parse("<p>é</p>")
    expect(frag.at_css("p").text).to eq("é")
  end
end
