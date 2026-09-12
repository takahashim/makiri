# Differential probe for xpath/mkr_css.c -> crate::css.
#
# The lowering's whole job is that a CSS selector and its XPath translation
# select the same nodes, so the probe runs every selector against fixtures and
# records the RESULT, not the AST - an AST comparison would pass for two
# translations that both parse and disagree.
#
# Both hosts: the XML engine reaches the lowering through Node#css, and the HTML
# one through the XPath fallback for selectors Lexbor's matcher cannot serve.
# The probes are run from spec/differential/run.rb, which puts lib/ on the
# load path; this makes them work when run directly too. The original line
# here expanded "lib" against THIS directory, which does not exist - it had
# been doing nothing, and the -Ilib on the command line was carrying it.
$LOAD_PATH.unshift File.expand_path("../../../lib", __dir__)
require "makiri"

COUNTS = Hash.new(0)
def note(k) = COUNTS[k] += 1

OUT = []

def try
  yield
rescue => e
  note(:raised)
  "RAISE #{e.class}: #{e.message}"
end

XML = <<~X
  <root xmlns="urn:d" xmlns:p="urn:p">
    <section id="s1" class="a b c" data-n="1" p:mark="x">
      <p class="first">one</p>
      <p class="second b">two</p>
      <p>three</p>
      <span class="b">span</span>
      <div><p class="deep">deep</p></div>
    </section>
    <section id="s2" class="a" data-n="12" lang="en-GB">
      <p>alpha</p>
      <em>em</em>
      <p>beta</p>
      <p:thing p:k="v">prefixed</p:thing>
    </section>
    <section id="s3"><!-- only a comment --></section>
    <section id="s4">text only</section>
  </root>
X

HTML = <<~H
  <!doctype html><html><body>
    <section id="s1" class="a b c" data-n="1">
      <p class="first">one</p>
      <p class="second b">two</p>
      <p>three</p>
      <span class="b">span</span>
      <div><p class="deep">deep</p></div>
    </section>
    <section id="s2" class="a" data-n="12" lang="en-GB">
      <p>alpha</p><em>em</em><p>beta</p>
    </section>
    <section id="s3"><!-- c --></section>
    <section id="s4">text only</section>
  </body></html>
H

SELECTORS = [
  # type, universal, namespace forms
  "p", "*", "section p", "section > p", "section p, section span",
  "|p", "*|p",
  # id and class
  "#s1", ".a", ".b", ".a.b", "p.first", "#s1 .b", "#s1 > .b",
  # attributes, every operator
  "[data-n]", "[data-n=1]", "[data-n='12']", "[class~=b]", "[class^=a]",
  "[class*=' b']", "[class$=c]", "[lang|=en]", "[data-n!=1]",
  # combinators
  "p + p", "p ~ p", "section + section", "div p", "div > p",
  "#s1 p + span", "#s2 p ~ p",
  # structural pseudo-classes
  "p:first-child", "p:last-child", "p:only-child", "section:empty",
  ":root", "p:first-of-type", "p:last-of-type", "p:only-of-type",
  "section:first-of-type", "section:last-of-type",
  # nth family
  "p:nth-child(1)", "p:nth-child(2)", "p:nth-child(odd)", "p:nth-child(even)",
  "p:nth-child(2n)", "p:nth-child(2n+1)", "p:nth-child(3n-1)",
  "p:nth-last-child(1)", "p:nth-last-child(2)",
  "p:nth-of-type(1)", "p:nth-of-type(2n)", "p:nth-last-of-type(1)",
  "section:nth-child(2)",
  # functional pseudo-classes
  "p:not(.first)", "p:not(.first):not(.second)", ":is(p, span)",
  ":where(p, span)", "section:has(p)", "section:has(> p)", "section:has(em)",
  "section:has(p + p)", ":not(section) > p",
  "p:is(.first, .second)", ":is(section .b)",
  # the Lexbor extension
  "p:-lexbor-contains(one)", "p:-lexbor-contains(ONE)",
  # unsupported constructs, which must fail closed
  "p::before", "[data-n=1 i]", "[*|a]", "p:hover", "p:nth-child(2 of .x)",
  "", "   ", ">>>", "p >", "p[", "p:not(",
].freeze

def run(doc, label, sel)
  r = try { doc.css(sel) }
  if r.is_a?(String)
    OUT << "#{label}|#{sel} = #{r}"
    return
  end
  note(:matched) if r.length > 0
  note(:empty) if r.length == 0
  ids = r.map do |n|
    id = (n["id"] rescue nil)
    cls = (n["class"] rescue nil)
    "#{n.name}#{id ? "##{id}" : ""}#{cls ? ".#{cls.split.join(".")}" : ""}"
  end
  OUT << "#{label}|#{sel} = [#{ids.join(" ")}]"
end

xdoc = Makiri::XML(XML)
hdoc = Makiri::HTML(HTML)

SELECTORS.each do |sel|
  run(xdoc, "xml", sel)
  run(hdoc, "html", sel)
  # Also from a non-root context node, where "the first compound is a
  # descendant of the context" actually bites.
  ctx = xdoc.at_css("#s1") || xdoc.root
  run(ctx, "xml@s1", sel) if ctx
  hctx = hdoc.at_css("#s1")
  run(hctx, "html@s1", sel) if hctx
end

# Namespaced selectors need the prefixed form, which only the XML host resolves.
["p|thing", "p|thing[p|k]", "p|thing[p|k=v]"].each do |sel|
  run(xdoc, "xml-ns", sel)
  note(:ns_selector)
end

# The complexity cap: 64 compounds is the bound, so 70 must fail closed.
deep = (1..70).map { "div" }.join(" ")
OUT << "cap = #{try { xdoc.css(deep).length }}"
note(:cap)

# at_css and matches? reach the same lowering.
OUT << "at_css = #{try { xdoc.at_css("p.second")&.text }}"
OUT << "matches = #{try { xdoc.at_css("#s1").matches?("section.a") }}"

puts OUT
puts "---- branch counts ----"
COUNTS.sort.each { |k, v| puts "#{k}=#{v}" }
