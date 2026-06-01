# frozen_string_literal: true
#
# Curated XPath 1.0 corpus for the differential harness (xpath_diff.rb). A
# deterministic baseline that walks the spec's surface — every axis, the node
# tests, position/predicate semantics, the built-in function library, and the
# operators — so a regression shows up without relying on the random generator.
# Add a case here whenever a divergence is found and fixed.

module XPathCorpus
  module_function

  # Documents that exercise structure the fuzzer fixtures don't: mixed siblings
  # for the sibling/following/preceding axes, repeated names for position(),
  # numeric-looking text for number coercion, and foreign (SVG) content.
  def extra_docs
    [
      ["axes", <<~HTML],
        <!DOCTYPE html><html><body>
          <div id="d1"><h1>title</h1><p>a</p><p class="k">b</p><span>s</span><!--c--></div>
          <div id="d2" data-n="3"><ul><li>1</li><li>2</li><li>3</li></ul></div>
          <a href="/x" title="t">link</a><a href="/y">link2</a>
        </body></html>
      HTML
      ["nums", <<~HTML],
        <!DOCTYPE html><html><body>
          <span class="n">10</span><span class="n">2</span><span class="n">7.5</span>
          <span class="n">-3</span><span class="n">x</span>
        </body></html>
      HTML
      ["svg", <<~HTML],
        <!DOCTYPE html><html><body>
          <p>before</p>
          <svg width="10"><g><circle r="1"/><rect/></g></svg>
          <math><mi>x</mi></math>
        </body></html>
      HTML
    ]
  end

  EXPRESSIONS = [
    # --- absolute / relative location paths ---
    "/html/body", "/html/body/div", "//div", "//p", "//*",
    ".//p", "//div//p", "/html/body/*",

    # --- axes ---
    "//div/child::p", "//div/descendant::*", "//div/descendant-or-self::div",
    "//p/parent::*", "//p/ancestor::div", "//p/ancestor-or-self::*",
    "//h1/following-sibling::p", "//span/preceding-sibling::p",
    "//h1/following::a", "//a/preceding::h1",
    "//div/self::div", "//a/attribute::href", "//*/@*",

    # --- node tests ---
    "//text()", "//comment()", "//node()", "//div/node()",
    "//*[local-name()='p']", "//*[name()='div']",

    # --- predicates: position ---
    "//p[1]", "//p[2]", "//p[last()]", "//p[last()-1]",
    "//li[position()=2]", "//li[position()>1]", "//li[position() mod 2 = 1]",
    "(//p)[1]", "(//div//p)[last()]", "//div[2]//li[1]",

    # --- predicates: attribute ---
    "//*[@id]", "//*[@class]", "//div[@id='d1']", "//a[@href='/x']",
    "//*[@data-n]", "//div[@data-n='3']", "//a[@title]", "//*[not(@class)]",
    "//*[@id='d1' or @id='d2']", "//a[@href and @title]",

    # --- predicates: nested / value ---
    "//div[p]", "//div[.//li]", "//div[count(.//li)=3]",
    "//div[p[@class='k']]", "//li[text()='2']", "//p[contains(text(),'b')]",
    "//div[@id][1]",

    # --- string functions ---
    "string(//h1)", "string(//div[1])", "concat('a','-',//h1)",
    "//*[starts-with(@id,'d')]", "//*[contains(@class,'k')]",
    "substring('hello',2,3)", "substring-before('a/b/c','/')",
    "substring-after('a/b/c','/')", "string-length(//h1)",
    "normalize-space('  a   b ')", "translate('bar','abc','ABC')",
    "//*[substring(@id,1,1)='d']",

    # --- boolean functions ---
    "boolean(//p)", "boolean(//nothere)", "not(//nothere)",
    "true()", "false()", "//p[true()]",

    # --- number functions / arithmetic ---
    "count(//p)", "count(//li)", "count(//*)", "count(//@*)",
    "sum(//span[@class='n'][number(.)=number(.)])",
    "1+2*3", "(1+2)*3", "7 mod 3", "10 div 4", "-5 + 3",
    "floor(7.5)", "ceiling(7.5)", "round(7.5)", "round(-2.5)",
    "number('42')", "number(//span[1])",
    "//span[number(.) > 5]", "//li[number(.) = position()]",

    # --- unions / operators ---
    "//h1 | //p", "//a | //li", "(//p | //li)[1]",
    # element + its own attributes in a union: element must sort first (§5.1)
    "//div | //div/@id | //div/@data-n", "//a | //@href | //@title",
    "//p[@class='k'] = 'b'", "//div/@id = 'd1'",
    "count(//p) + count(//li)", "//*[@id='d1'] and //*[@id='d2']",

    # --- comparisons / type coercion ---
    "//li[. = 2]", "//li[. >= '2']", "//span[. < 8]",
    "//*[@data-n > 2]", "'10' < '9'", "10 < 9",

    # --- edge / empty results ---
    "//absent", "//div[@id='nope']", "//p[99]", "/nonsense/path",
    "//*[@id='d1']/following-sibling::*[1]",
  ].freeze
end
