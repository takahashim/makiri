# frozen_string_literal: true
#
# Seed corpora for the fuzzer. A representative cross-section of the query
# surface, chosen so byte-level mutation has plenty of valid structure to
# perturb. The generators in grammar.rb add more on top.

SEED_CORPUS = [
  # paths
  "/html", "/html/body/div", "//div", "//*", "//a",
  '//*[@id]', '//div[@class="root"]', "//ul/li[1]", "//ul/li[last()]",
  '//section/p[@class="lead"]', '//section[@id="s1"]//p',

  # axes
  "//p/parent::*", "//p/ancestor::section", "//p/ancestor-or-self::node()",
  "//h1/following-sibling::p", "//h1/following::*",
  "//p/preceding-sibling::*", "//p/preceding::h1",
  "//*[@id]/self::*", "//a/attribute::href", "//a/@href", "//*/@*",

  # predicates
  "//li[1]", "//li[position() mod 2 = 0]", "//li[position() > 1]", "//li[last()]",
  "//*[@class and @id]", "//*[@class or @id]", "//*[not(@class)]",
  "//section[count(p) > 1]", '//section[p[@class="lead"]]',

  # operators
  "//*[1+1=2]", "//*[2 div 1 = 2]", "//*[5 mod 2 = 1]", "//*[-1 = -1]",

  # functions
  "//*[string-length(@class) > 0]", '//*[contains(@class, "lead")]',
  '//*[starts-with(@href, "http")]', '//*[substring(@class, 1, 4) = "lead"]',
  '//*[normalize-space(.) = "x"]', '//*[translate(@class, "abc", "ABC") = "ABc"]',
  '//*[name() = "div"]', '//*[local-name() = "p"]',
  "//*[number(@data-i) = 1]", "//*[floor(1.7) = 1]", "//*[round(1.5) = 2]",
  "//*[sum(.//*/@data-i) > 0]", "//*[count(.//*) > 5]",

  # union / grouping
  "//h1 | //h2", "//a | //img", '//*[@class="lead"] | //*[@class="hot"]',
  "(//div)[1]", "(//div)[last()]", "(//section)[2]//p",

  # scalar XPath
  "1 + 1", 'string-length("abc")', 'concat("a", "b")',
  "true()", "false()", "not(false())", 'number("3.14")',

  # empty / text / comment
  "//nope", "//*[@nope]", "//text()", "//comment()", "//p/text()",

  # namespace prefix that is not registered (must fail closed, not crash)
  "//svg:circle", "//foo:bar",
].freeze

# CSS selectors for --target css / both.
CSS_SEED = [
  "*", "div", "a", "p.lead", "#root", ".item", "div.item",
  "div > p", "section p", "ul li + li", "li ~ li",
  "a[href]", 'a[href="x"]', '[data-i="1"]', '[class~="hot"]', '[lang|="ja"]',
  "li:first-child", "li:last-child", "li:nth-child(2)", "li:nth-of-type(odd)",
  ":first-of-type", ":only-of-type", ":nth-of-type(2n+1)", "*:last-of-type",
  "p:not(.lead)", ":root", "section:has(p)", "div, span, a",
  "svg circle", "header, footer", "*:empty",
  # complex (combinator) arguments inside :is/:where/:not + :has variants
  "a:not(nav a)", ":is(div > p, section a)", ":where(ul li)", "li:has(> a)",
  "p:has(+ span)", "div:not(.x):not(.y)",
  # jQuery-style text containment (Lexbor extension, both hosts)
  'p:lexbor-contains("text")', 'li:lexbor-contains("X" i)', ':lexbor-contains("a")',
].freeze
