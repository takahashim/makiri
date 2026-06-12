# frozen_string_literal: true
#
# CSS selector corpus for the XML differential (spec/conformance/xml_css_diff.rb).
#
# Purpose-built documents (rather than the XPath corpus's) so the bare-type-
# selector default-namespace binding can be compared fairly: the XPath docs use
# `xmlns=""` (an explicit empty default namespace), which Nokogiri's CSS handles
# differently from its own XPath - a Nokogiri quirk, not a Makiri difference - so
# those documents are unsuitable for a strict CSS differential. The docs here use
# either a real default namespace, prefixed namespaces, or none at all.

module XmlCssCorpus
  module_function

  ATOM  = "http://www.w3.org/2005/Atom"
  DC    = "http://purl.org/dc/elements/1.1/"
  SVG   = "http://www.w3.org/2000/svg"
  XLINK = "http://www.w3.org/1999/xlink"

  DOCS = [
    {
      name: "plain", ns: {},
      xml: <<~XML,
        <catalog>
          <book id="b1" class="lead big"><title lang="en">A</title><author>Ada</author></book>
          <book id="b2" class="big"><title lang="fr">B</title><author>Bob</author></book>
          <note/>
        </catalog>
      XML
    },
    {
      name: "atom", ns: { "a" => ATOM },
      xml: <<~XML,
        <feed xmlns="#{ATOM}">
          <title>Feed</title>
          <entry id="e1"><title>One</title><link href="/1"/></entry>
          <entry id="e2"><title>Two</title><link href="/2"/></entry>
        </feed>
      XML
    },
    {
      name: "rss", ns: { "dc" => DC },
      xml: <<~XML,
        <rss version="2.0" xmlns:dc="#{DC}">
          <channel>
            <title>Ch</title>
            <item><title>I1</title><dc:date>2025</dc:date></item>
            <item><title>I2</title><dc:creator>X</dc:creator></item>
          </channel>
        </rss>
      XML
    },
    {
      name: "svg", ns: { "s" => SVG, "xlink" => XLINK },
      xml: <<~XML,
        <svg xmlns="#{SVG}" xmlns:xlink="#{XLINK}">
          <g><circle fill="red"/><rect/></g>
          <circle fill="blue"/>
        </svg>
      XML
    },
  ].freeze

  # No-prefix selectors run on every document (bare type binds to the default ns).
  GENERIC = [
    "*", "title", "entry", "item", "book", "link", "circle", "rect",
    "feed > entry", "entry title", "channel item", "g circle", "book title",
    "[id]", "[id='e1']", "[class~='big']", "[href]", "[fill]", "[lang='en']",
    ".big", ".lead", "#b1", "#e2",
    ":root", "feed > *:first-child", "feed > *:last-child",
    "entry:first-child", "entry:last-child", "title:empty",
    "feed > *:nth-child(2)", "entry:nth-child(odd)", "book:nth-of-type(2)",
    ":is(title, link)", "feed :not(title)", "entry:has(title)",
    "entry:has(> link)", "title + link", "entry ~ entry", "book:not(.lead)",
    # Makiri-unsupported by design (XPath-1.0 / standards-only) -> tallied:
    "*|entry", "|title", "|entry", "|book", "title[lang='EN' i]", "title:contains('x')",
    # (Untyped of-type like `*:first-of-type` is NOT here: Makiri now supports it
    #  correctly, but Nokogiri mistranslates it to first-child, so it would
    #  diverge. Makiri's correct behaviour is pinned in spec/xml_css_spec.rb.)
  ].freeze

  # Selectors using a document's registered prefixes, run only on that document.
  PER_DOC = {
    "atom" => ["a|entry", "a|feed > a|entry", "a|entry a|title", "a|link[href]"],
    "rss"  => ["dc|date", "dc|creator", "item dc|date", "channel > item"],
    "svg"  => ["s|circle", "s|svg s|rect", "s|circle[fill]", "s|g s|circle"],
  }.freeze

  def selectors_for(name)
    GENERIC + (PER_DOC[name] || [])
  end
end
