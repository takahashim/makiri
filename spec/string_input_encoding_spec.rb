# frozen_string_literal: true

# The strict inputs - selectors, XPath, names, attribute values, text - read a
# String in its own encoding, as parsing does: UTF-8, US-ASCII and binary as
# UTF-8 bytes, any other encoding transcoded to UTF-8 first. They used to read
# every String's bytes as UTF-8, so a Windows-1252 "\xC3\xA9" became "é" and
# a UTF-16 "a" became "a\0". A String that cannot be converted is refused,
# never repaired.
RSpec.describe "a strict input's String encoding" do
  let(:doc) { Makiri::HTML(%(<p class="é" title="日本">x</p>)) }
  let(:p_el) { doc.at_css("p") }

  it "reads a selector or an XPath expression in its own encoding" do
    expect(doc.css(".é".encode("Windows-1252")).size).to eq(1)
    expect(doc.css("p[title=日本]".encode("Shift_JIS")).size).to eq(1)
    expect(doc.css("p".encode("UTF-16LE")).size).to eq(1)
    expect(doc.xpath(%(//p[@title="日本"]).encode("EUC-JP")).size).to eq(1)
  end

  it "stores data as the characters it holds, not its bytes" do
    p_el["t"] = "\xC3\xA9".dup.force_encoding("Windows-1252")
    expect(p_el["t"]).to eq("Ã©")
    expect(doc.create_text_node("a".encode("UTF-16LE")).content).to eq("a")
    expect(doc.create_element("b".encode("UTF-16BE")).name).to eq("b")
    x = Makiri::XML("<r/>")
    x.root["a"] = "caf\xE9".dup.force_encoding("ISO-8859-1")
    expect(x.root["a"]).to eq("café")
  end

  it "reads UTF-8, US-ASCII and binary Strings as UTF-8 bytes, as before" do
    expect(doc.css("p[title=日本]".b).size).to eq(1)
    expect(doc.css("p".encode("US-ASCII")).size).to eq(1)
  end

  it "refuses a String it cannot convert" do
    expect { doc.css("\x82".dup.force_encoding("Shift_JIS")) }
      .to raise_error(Makiri::Error, /could not be converted to UTF-8/)
    expect { p_el["t"] = "\xFF".dup.force_encoding("EUC-JP") }
      .to raise_error(Makiri::Error, /could not be converted to UTF-8/)
  end
end
