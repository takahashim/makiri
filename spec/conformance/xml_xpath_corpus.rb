# frozen_string_literal: true

# Corpus for the XML XPath differential (spec/conformance/xml_xpath_diff.rb):
# representative XML document shapes (RSS / Atom / SVG / SOAP / plain / mixed),
# each with the namespace prefix map its queries use, plus a set of structural,
# prefix-free expressions run against every document.
module XmlXPathCorpus
  ATOM  = "http://www.w3.org/2005/Atom"
  DC    = "http://purl.org/dc/elements/1.1/"
  SVG   = "http://www.w3.org/2000/svg"
  XLINK = "http://www.w3.org/1999/xlink"
  SOAP  = "http://schemas.xmlsoap.org/soap/envelope/"
  STOCK = "https://example.com/stock"

  # --- documents: { name, xml, ns: {prefix=>uri}, exprs: [name/prefix-specific] }
  DOCS = [
    {
      name: "plain",
      ns: {},
      xml: <<~XML,
        <?xml version="1.0"?>
        <catalog xmlns="">
          <book id="b1" available="yes">
            <title lang="en">First &amp; Foremost</title>
            <author>Ada</author>
            <price currency="USD">9.99</price>
            <tags><tag>a</tag><tag>b</tag><tag>c</tag></tags>
          </book>
          <book id="b2">
            <title lang="fr">Deuxieme</title>
            <author>Bob</author>
            <price currency="EUR">12.5</price>
            <!-- a comment -->
          </book>
          <book id="b3"><title>Third</title><author>Ada</author><price>0</price></book>
        </catalog>
      XML
      exprs: [
        "/catalog/book", "//book", "//book[@id='b2']", "//book[2]", "//book[last()]",
        "//book[@available]", "//book[price>10]", "//book[price<10]", "//book[price=0]",
        "//title[@lang='en']", "//book[author='Ada']", "count(//book[author='Ada'])",
        "//tag", "//tag[2]", "//book[1]/tags/tag[position()=2]",
        "//book[1]/following-sibling::book", "//book[3]/preceding-sibling::book",
        "//book[2]/ancestor::catalog", "//title/ancestor::book/@id",
        "//book/parent::*", "//price[@currency='EUR']/..",
        "string(//book[1]/title)", "string(//book[1]/price)", "number(//book[1]/price)",
        "sum(//price)", "//book[@id='b1']/title/@lang",
        "normalize-space(//book[2]/title)", "//author[. = 'Ada']",
        "//book[contains(title,'e')]", "//book[starts-with(@id,'b')]",
        "concat(//book[1]/@id,'-',//book[2]/@id)", "//book[not(@available)]",
        "//@currency", "//@id", "//book[count(tags/tag)=3]",
      ],
    },
    {
      name: "atom",
      ns: { "a" => ATOM },
      xml: <<~XML,
        <?xml version="1.0" encoding="utf-8"?>
        <feed xmlns="#{ATOM}" xml:lang="en">
          <title>Example</title>
          <updated>2025-06-02T10:00:00Z</updated>
          <entry>
            <title>First</title>
            <link href="https://example.com/1" rel="alternate"/>
            <id>urn:1</id>
            <updated>2025-06-02T09:00:00Z</updated>
          </entry>
          <entry>
            <title>Second</title>
            <link href="https://example.com/2" rel="self"/>
            <id>urn:2</id>
          </entry>
        </feed>
      XML
      exprs: [
        "//a:entry", "//a:entry/a:title", "//a:feed/a:entry", "//a:entry[1]/a:title",
        "//a:entry[last()]", "count(//a:entry)", "//a:link/@href", "//a:link[@rel='self']",
        "//a:entry[a:id='urn:2']", "string(//a:entry[1]/a:title)",
        "//a:entry[2]/preceding-sibling::a:entry", "//a:title/ancestor::a:entry/a:id",
        "local-name(//a:entry[1])", "namespace-uri(//a:entry[1])", "name(//a:entry[1])",
        "//a:entry/a:link[@rel='alternate']", "//entry", "//a:link/@xlink:href",
      ],
    },
    {
      name: "rss",
      ns: { "dc" => DC },
      xml: <<~XML,
        <?xml version="1.0"?>
        <rss version="2.0" xmlns:dc="#{DC}">
          <channel>
            <title>Feed</title>
            <link>https://example.com/</link>
            <lastBuildDate>Mon, 02 Jun 2025 10:00:00 GMT</lastBuildDate>
            <item>
              <title>One</title>
              <dc:creator>Ada</dc:creator>
              <pubDate>Mon, 02 Jun 2025 09:00:00 GMT</pubDate>
              <guid isPermaLink="true">https://example.com/1</guid>
            </item>
            <item>
              <title>Two</title>
              <dc:creator>Bob</dc:creator>
            </item>
          </channel>
        </rss>
      XML
      exprs: [
        "/rss/channel/item", "//item", "//item/title", "//channel/lastBuildDate",
        "//item/pubDate", "//item[dc:creator='Ada']", "//dc:creator", "count(//item)",
        "//guid/@isPermaLink", "string(//channel/title)", "//item[1]/following-sibling::item",
        "//item[position()=1]/title", "//item[last()]/title",
      ],
    },
    {
      name: "svg",
      ns: { "s" => SVG, "xlink" => XLINK },
      xml: <<~XML,
        <svg xmlns="#{SVG}" xmlns:xlink="#{XLINK}" width="100" height="100">
          <rect x="0" y="0" width="50" height="50"/>
          <g id="grp"><circle r="10"/><circle r="20"/></g>
          <a xlink:href="https://example.com/"><text>link</text></a>
        </svg>
      XML
      exprs: [
        "//s:rect", "//s:circle", "count(//s:circle)", "//s:g/s:circle",
        "//s:rect/@width", "//s:circle[@r='20']", "//s:a/@xlink:href",
        "string(//s:a/@xlink:href)", "//s:g[@id='grp']/s:circle[1]",
        "//s:circle/ancestor::s:g/@id", "namespace-uri(//s:rect)", "//rect",
      ],
    },
    {
      name: "soap",
      ns: { "soap" => SOAP, "m" => STOCK },
      xml: <<~XML,
        <soap:Envelope xmlns:soap="#{SOAP}" xmlns:m="#{STOCK}">
          <soap:Header><m:Trans>234</m:Trans></soap:Header>
          <soap:Body>
            <m:GetPriceResponse>
              <m:Price>34.5</m:Price>
              <m:Currency>USD</m:Currency>
            </m:GetPriceResponse>
          </soap:Body>
        </soap:Envelope>
      XML
      exprs: [
        "//soap:Body", "//soap:Body/m:GetPriceResponse/m:Price",
        "number(//m:Price)", "string(//m:Currency)", "//m:Trans",
        "//soap:Header/m:Trans", "count(//m:*)", "//soap:Envelope/soap:Body",
        "//m:Price/ancestor::soap:Body", "local-name(//m:Price)",
      ],
    },
    {
      name: "mixed",
      ns: {},
      xml: <<~XML,
        <doc xmlns="">
          <p>Hello <b>bold</b> and <i>italic</i> world</p>
          <data><![CDATA[a < b && c > d]]></data>
          <empty/>
          <whitespace>   </whitespace>
          <nums><n>3</n><n>1</n><n>2</n></nums>
        </doc>
      XML
      exprs: [
        "//p", "//p/text()", "string(//p)", "normalize-space(//p)",
        "//b", "//p/b/following-sibling::i", "//data", "string(//data)",
        "//empty", "//nums/n", "sum(//nums/n)", "//nums/n[. > 1]",
        "//p/node()", "count(//p/node())", "//whitespace",
      ],
    },
  ].freeze

  # Structural, prefix-free expressions run against every document (the doc's ns
  # map is still passed, harmlessly, so both engines see identical inputs).
  STRUCTURAL = [
    "//*", "(//*)[1]", "(//*)[last()]", "//*[1]", "count(//*)", "count(//node())",
    "//*/..", "//*[not(*)]", "//*[*]", "//*[@*]", "//*[text()]",
    "//*[position()=1]", "//node()", "//comment()", "//text()",
    "string(/*)", "local-name(/*)", "name(/*)", "//*/ancestor-or-self::*",
    "//*/self::*", "count(//*[1]/following::*)", "//*[last()]/preceding::*",
    "boolean(//*)", "not(//nonexistent)", "true()", "false()", "1 + 2 * 3",
    "count(//*) > 0", "//*[count(@*) > 0]", "//*[string-length(name()) > 3]",
  ].freeze
end
