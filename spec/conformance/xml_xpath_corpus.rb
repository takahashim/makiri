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
        # //name[N] is per-context (the Nth name-child of EACH parent), not
        # (//name)[N]; these exercise that across multiple parents (each book is
        # a separate parent of one author/title) vs single-parent //book[2].
        "//author[1]", "//author[2]", "//title[1]", "//tag[1]", "//tag[3]", "//tag[4]",
        "//book[1]/following-sibling::book", "//book[3]/preceding-sibling::book",
        "//book[2]/ancestor::catalog", "//title/ancestor::book/@id",
        "//book/parent::*", "//price[@currency='EUR']/..",
        "string(//book[1]/title)", "string(//book[1]/price)", "number(//book[1]/price)",
        "sum(//price)", "//book[@id='b1']/title/@lang",
        "normalize-space(//book[2]/title)", "//author[. = 'Ada']",
        "//book[contains(title,'e')]", "//book[starts-with(@id,'b')]",
        "concat(//book[1]/@id,'-',//book[2]/@id)", "//book[not(@available)]",
        "//@currency", "//@id", "//book[count(tags/tag)=3]",
        # string functions over node string-values
        "substring(//book[1]/title,1,5)", "substring(//book[3]/title,2)",
        "substring-before(//book[1]/title,' ')", "substring-after(//book[1]/title,'& ')",
        "translate(//book[2]/@id,'b','B')", "string-length(//book[1]/title)",
        # operators / comparisons / arithmetic
        "//book[price != 0]", "//book[price >= 9.99]", "//book[price <= 10]",
        "//book[@id != 'b1']", "//book[position() != last()]", "//book[last()-1]/@id",
        "//book[price mod 1 = 0]", "sum(//price) div count(//book)",
        "count(//book) - count(//book[@available])",
        "//book[1]/title = //book[3]/title", "//title | //author",
        "//book[1]/@* | //book[2]/@*", "//tag[position() mod 2 = 1]",
        # explicit axes
        "//book/child::title", "//book/attribute::id", "/descendant::book",
        "//catalog/child::book[2]/descendant::tag",
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
        # lang() with xml:lang inheritance from <feed xml:lang="en">
        "//a:entry[lang('en')]", "//a:title[lang('en')]", "//a:entry[lang('fr')]",
        "count(//a:entry[lang('EN')])", # lang() is case-insensitive
        # union / positional arithmetic / string fns
        "//a:entry/a:title | //a:feed/a:title", "//a:link/@href | //a:entry/a:id",
        "//a:entry[position()=last()-1]", "substring(//a:entry[1]/a:title,1,3)",
        "//a:feed/child::a:entry", "//a:entry/descendant::a:id",
        "count(//a:entry/a:link) - count(//a:entry)",
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
        "//item | //channel/title", "//item[position() != 1]",
        "translate(//dc:creator[1],'Aa','aA')", "count(//item) mod 2",
        "substring-after(//channel/link,'https://')", "//item[dc:creator != 'Ada']",
        "//channel/descendant::dc:creator",
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
        "//s:circle | //s:rect", "//s:circle[@r > 15]", "//s:circle[@r != '10']",
        "//s:circle[position()=last()]", "substring(//s:a/@xlink:href,1,5)",
        "//s:g/child::s:circle", "number(//s:rect/@width) + number(//s:rect/@height)",
        "//s:rect/@width div //s:svg/@width", "//s:*[@r]",
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
        "//m:Price | //m:Currency", "number(//m:Price) + 1", "number(//m:Price) * 2",
        "round(number(//m:Price))", "floor(number(//m:Price))", "ceiling(number(//m:Price))",
        "number(//m:Trans) mod 100", "//m:* | //soap:*",
        "substring-before(string(//m:Price),'.')",
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
          <meta><?render fast?><!-- a note --><?render slow?></meta>
        </doc>
      XML
      exprs: [
        "//p", "//p/text()", "string(//p)", "normalize-space(//p)",
        "//b", "//p/b/following-sibling::i", "//data", "string(//data)",
        "//empty", "//nums/n", "sum(//nums/n)", "//nums/n[. > 1]",
        "//p/node()", "count(//p/node())", "//whitespace",
        # element-level processing instructions + comments
        "//processing-instruction()", "count(//processing-instruction())",
        "//processing-instruction('render')", "name(//meta/processing-instruction()[1])",
        "string(//meta/processing-instruction()[2])", "//meta/comment()",
        "count(//meta/node())", "//meta/node()[2]",
        # number/string fns over content
        "sum(//nums/n) div count(//nums/n)", "//nums/n[. != 2]",
        "//nums/n[position() mod 2 = 1]", "translate(string(//p),'lo','LO')",
        "substring-after(string(//data),'< ')", "//nums/n | //p/b",
        "number(//nums/n[1]) - number(//nums/n[2])",
      ],
    },
    {
      name: "misc",
      ns: {},
      # Comments and PIs in the prolog/epilog (around the root) are children of
      # the document node, like Nokogiri / the XPath data model - with realistic
      # whitespace between them to pin that prolog whitespace is not a text node.
      xml: <<~XML,
        <?xml version="1.0"?>
        <?xml-stylesheet type="text/xsl" href="a.xsl"?>
        <!-- top comment -->
        <doc xmlns="">
          <?run inside?>
          <item>x</item>
          <!-- inner -->
        </doc>
        <!-- tail comment -->
        <?after epilog?>
      XML
      exprs: [
        "//comment()", "count(//comment())", "//processing-instruction()",
        "count(//processing-instruction())", "//processing-instruction('run')",
        "/comment()", "/processing-instruction()", "count(/node())", "/node()[1]",
        "/node()[2]", "name(/processing-instruction()[1])",
        "//comment()[1]/parent::node()", "//doc/preceding-sibling::node()",
        "//doc/following-sibling::node()", "//doc/preceding-sibling::comment()",
        "string(/processing-instruction()[1])", "//node()[last()]",
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

    # --- union ----------------------------------------------------------
    "//* | //@*", "count(//* | //node())", "//*[1] | //*[last()]",
    "count(//* | //*)", # union of identical sets dedups
    # --- comparison operators ------------------------------------------
    "count(//*) != 0", "count(//*) >= 1", "count(//*) <= 100000", "1 != 2",
    "2 <= 2", "3 >= 4", "//*[position() != 1]", "//*[position() <= 2]",
    # --- arithmetic -----------------------------------------------------
    "5 - 3", "7 div 2", "7 mod 3", "-3 + 1", "10 - 2 * 3", "(10 - 2) * 3",
    "6 div 4", "count(//*) - 1", "count(//*) mod 2",
    # --- boolean --------------------------------------------------------
    "true() and false()", "true() or false()", "not(false())", "boolean(0)",
    "boolean('')", "boolean('x')", "count(//*) > 0 and true()",
    # --- number functions ----------------------------------------------
    "floor(1.9)", "ceiling(1.1)", "round(2.5)", "round(-2.5)", "round(0.5)",
    "floor(-1.1)", "ceiling(-1.9)", "round(2.4)",
    # --- number conversion / formatting (NaN, Infinity, -0) -------------
    "number('  42 ')", "string(number('xyz'))", "string(1 div 0)",
    "string(-1 div 0)", "string(0 div 0)", "string(1 div 3)", "string(0.5)",
    "string(-0.5 + 0.5)", "number('3.14')", "string(123456789)", "string(0.1)",
    # --- string functions ----------------------------------------------
    "substring('hello',2,3)", "substring('hello',2)", "substring('hello',0,3)",
    "substring('hello',-1,3)", "substring-before('a-b-c','-')",
    "substring-after('a-b-c','-')", "substring-before('abc','x')",
    "translate('Bar','abcr','ABCR')", "translate('abcabc','ac','x')",
    "string-length('hello')", "normalize-space('  a   b  c ')",
    "concat('a','b','c','d','e')", "contains('','')", "starts-with('abc','')",
    # --- nested predicates ---------------------------------------------
    "//*[*][@*]", "//*[position()=1][1]", "//*[last()][1]",
    # --- explicit axes --------------------------------------------------
    "/child::*", "//*/self::node()", "//*/descendant-or-self::node()[1]",
  ].freeze
end
