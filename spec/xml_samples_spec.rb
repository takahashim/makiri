# frozen_string_literal: true

require "spec_helper"

# End-to-end reads of the document shapes Makiri::XML targets: RSS 2.0, Atom,
# SVG, and a SOAP envelope. These pin that real feeds/APIs are queryable (the
# namespace ergonomics, camelCase element names, mixed prefixes) and that the
# XML host-policy for id()/lang() matches the spec.
RSpec.describe "Makiri::XML real-world samples" do
  describe "RSS 2.0 (no namespace, camelCase elements)" do
    let(:doc) do
      Makiri::XML(<<~XML)
        <?xml version="1.0" encoding="UTF-8"?>
        <rss version="2.0">
          <channel>
            <title>Example Feed</title>
            <link>https://example.com/</link>
            <lastBuildDate>Mon, 02 Jun 2025 10:00:00 GMT</lastBuildDate>
            <item>
              <title>First &amp; foremost</title>
              <link>https://example.com/1</link>
              <guid isPermaLink="true">https://example.com/1</guid>
              <pubDate>Mon, 02 Jun 2025 09:00:00 GMT</pubDate>
            </item>
            <item>
              <title>Second</title>
              <pubDate>Sun, 01 Jun 2025 09:00:00 GMT</pubDate>
            </item>
          </channel>
        </rss>
      XML
    end

    it "reads channel metadata and item count (no prefix needed)" do
      expect(doc.at_xpath("/rss/channel/title").text).to eq("Example Feed")
      expect(doc.xpath("//item").length).to eq(2)
    end

    it "preserves camelCase element names (pubDate / lastBuildDate)" do
      expect(doc.xpath("//item/pubDate").length).to eq(2)
      expect(doc.at_xpath("//channel/lastBuildDate").text).to include("02 Jun 2025")
    end

    it "decodes entities in text and reads attributes" do
      expect(doc.at_xpath("//item[1]/title").text).to eq("First & foremost")
      expect(doc.at_xpath("//item[1]/guid")["isPermaLink"]).to eq("true")
    end
  end

  describe "Atom (default namespace -> registered prefix)" do
    let(:doc) do
      Makiri::XML(<<~XML)
        <?xml version="1.0" encoding="utf-8"?>
        <feed xmlns="http://www.w3.org/2005/Atom" xml:lang="en">
          <title>Atom Example</title>
          <entry>
            <title>Hello</title>
            <link href="https://example.com/a" rel="alternate"/>
            <updated>2025-06-02T10:00:00Z</updated>
          </entry>
          <entry>
            <title>World</title>
            <updated>2025-06-01T10:00:00Z</updated>
          </entry>
        </feed>
      XML
    end
    let(:ns) { { "a" => "http://www.w3.org/2005/Atom" } }

    it "does not match unprefixed names (strict default-namespace)" do
      expect(doc.xpath("//entry").length).to eq(0)
    end

    it "selects entries through a registered prefix" do
      expect(doc.xpath("//a:entry", ns).length).to eq(2)
      expect(doc.xpath("//a:entry/a:title", ns).map(&:text)).to eq(%w[Hello World])
    end

    it "exposes namespace_uri / local_name on a node" do
      entry = doc.at_xpath("//a:entry", ns)
      expect(entry.local_name).to eq("entry")
      expect(entry.namespace_uri).to eq("http://www.w3.org/2005/Atom")
    end

    it "honours xml:lang for lang()" do
      title = doc.at_xpath("//a:title", ns)
      expect(title.xpath('lang("en")')).to be true
    end
  end

  describe "SVG (foreign namespace + a prefixed namespace)" do
    let(:doc) do
      Makiri::XML(<<~XML)
        <svg xmlns="http://www.w3.org/2000/svg"
             xmlns:xlink="http://www.w3.org/1999/xlink"
             width="100" height="100">
          <rect x="0" y="0" width="50" height="50"/>
          <a xlink:href="https://example.com/"><circle r="10"/></a>
        </svg>
      XML
    end
    let(:ns) do
      { "s" => "http://www.w3.org/2000/svg", "xlink" => "http://www.w3.org/1999/xlink" }
    end

    it "reads geometry through the svg prefix" do
      expect(doc.at_xpath("//s:rect", ns)["width"]).to eq("50")
      expect(doc.xpath("//s:circle", ns).length).to eq(1)
    end

    it "reads a prefixed attribute via its namespace" do
      a = doc.at_xpath("//s:a", ns)
      expect(a.xpath("string(@xlink:href)", ns)).to eq("https://example.com/")
    end
  end

  describe "SOAP envelope (multiple namespaces)" do
    let(:doc) do
      Makiri::XML(<<~XML)
        <soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"
                       xmlns:m="https://example.com/stock">
          <soap:Body>
            <m:GetPriceResponse>
              <m:Price>34.5</m:Price>
            </m:GetPriceResponse>
          </soap:Body>
        </soap:Envelope>
      XML
    end
    let(:ns) do
      { "soap" => "http://schemas.xmlsoap.org/soap/envelope/", "m" => "https://example.com/stock" }
    end

    it "navigates the envelope by prefix and reads the payload" do
      expect(doc.at_xpath("//soap:Body/m:GetPriceResponse/m:Price", ns).text).to eq("34.5")
      expect(doc.xpath("number(//m:Price)", ns)).to eq(34.5)
    end
  end

  describe "host policy: id() has no IDs without a DTD" do
    it "returns an empty node-set for id() in XML (DTDs are rejected)" do
      doc = Makiri::XML(%(<doc><item id="1"/><item id="2"/></doc>))
      expect(doc.xpath("count(id('1'))")).to eq(0.0)
    end
  end
end
