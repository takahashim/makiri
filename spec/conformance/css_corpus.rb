# frozen_string_literal: true
#
# Curated CSS selector corpus for the differential (css_diff.rb). Standard
# selectors (lowercased, so class/id case-sensitivity doesn't add noise) that
# both engines should agree on, plus a few that exercise the vocabulary
# buckets (Level-4 :is/:where -> lexbor-only; jQuery :contains/:gt -> nokogiri-
# only; ::before -> both reject).

module CSSCorpus
  module_function

  def extra_docs
    [
      ["css", <<~HTML],
        <html><body>
          <main id="main" class="container">
            <section class="list">
              <ul>
                <li class="item first" data-id="1"><a href="/p/1">one</a></li>
                <li class="item" data-id="2"><a href="/p/2" rel="next">two</a></li>
                <li class="item last" data-id="3"><span>three</span></li>
              </ul>
            </section>
            <p class="lead">intro</p>
            <p>body</p>
            <div class="empty"></div>
            <svg><circle r="1"/></svg>
          </main>
        </body></html>
      HTML
    ]
  end

  SELECTORS = [
    # type / universal / class / id
    "*", "li", "div", "p", ".item", ".item.first", "#main", "main.container",
    # descendant / child / adjacent / general sibling
    "ul li", "ul > li", "li > a", "p + p", "li ~ li", "section a",
    # attribute selectors (all operators; values quoted)
    "[data-id]", "[href]", "a[rel]", "[data-id='2']", "[href^='/p']",
    "[href$='/2']", "[href*='p/']", "[class~='first']", "[class|='item']",
    "a[href='/p/1']",
    # structural pseudo-classes
    "li:first-child", "li:last-child", "li:only-child", "li:nth-child(2)",
    "li:nth-child(odd)", "li:nth-last-child(1)", "p:first-of-type",
    "p:last-of-type", "p:nth-of-type(2)", "p:only-of-type", "div:empty",
    ":root", "a:empty",
    # negation / matches-any (Level 4 :is/:where are lexbor-only vs Nokogiri)
    "li:not(.first)", "li:not(.first):not(.last)", ":is(p, span)",
    ":where(p, span)", "main:has(> section)", "li:has(a)",
    # grouping
    "p, span", "ul, ol", ".lead, .empty",
    # vocabulary buckets: jQuery extensions (nokogiri-only) and pseudo-elements
    "p:contains('intro')", "li:gt(0)", "li:eq(1)", "li:first", "p::before",
  ].freeze
end
