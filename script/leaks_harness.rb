# frozen_string_literal: true

# Exercise script for the malloc-leak gate (script/check_leaks.rb runs this
# under macOS `leaks --atExit`). It loops the public surface - parsing, queries,
# serialization, mutation, fragments, the Builder, XPathContext - and, crucially,
# RESCUED FAILURE paths (raises that skip cleanup are where leaks hide; both
# leaks this gate was built on lived there or in a transient-document path).
#
# A per-call leak shows up as a leak stack with ~ITERATIONS instances, which the
# driver flags; one-time/Init allocations stay at 1-2 instances and pass.

require "makiri"

ITERATIONS = Integer(ENV.fetch("LEAKS_ITERATIONS", "120"))

HTML = "<div id=m class='a b'><ul><li class=item>x</li><li>y<svg><path/></svg></li></ul><p>t&amp;</p></div>"
XML  = %(<r xmlns:p="urn:p" xmlns="urn:d"><a id="1">t</a><p:b/><!--c--><![CDATA[z]]></r>)
CSS  = "div.a, #b > span { color: red !important; --v: 1px }\n" \
       "@media (min-width: 600px) { .x { opacity: 0 } }\n@font-face { font-family: y }"

handler = Class.new { def my_fn(nodes) = nodes.length.to_s }.new

ITERATIONS.times do |i|
  # --- HTML: parse / query / serialize / mutate / fragments ---
  d = Makiri::HTML(HTML)
  d.css("li.item"); d.at_css("#m"); d.at_css("li").matches?("li")
  begin d.css("li[") rescue Makiri::CSS::SyntaxError; end             # selector syntax error (engine reset path)
  d.xpath("//li"); d.at_xpath("//p"); d.xpath("count(//li)")
  begin d.xpath("//li[") rescue Makiri::XPath::SyntaxError; end       # parse failure (partial-AST/step cleanup)
  begin d.xpath("//li", handler) rescue nil; end
  d.xpath("//*[local-name()='path']")
  d.to_html; d.at_css("ul").inner_html; d.to_html(pretty: true); d.text
  e = d.at_css("li"); e["k#{i}"] = "v"; e.name = "li2"; e.content = "c"
  e.set_attribute_ns("urn:x", "x:y", "1"); e.remove_attribute_ns("urn:x", "y")
  d.at_css("ul").inner_html = "<li>new</li>"                          # transient fragment document path
  frag = d.fragment("<b>f</b>"); d.at_css("p") << frag
  Makiri::DocumentFragment.parse("<tr><td>1</td></tr>", context: "tbody")
  d.at_css("p").clone_node(true); d.dup
  begin d.fragment("x", context: "no-such-tag") rescue ArgumentError; end

  # --- XML: parse (ok + failures) / query / serialize / mutate / Builder ---
  x = Makiri::XML(XML)
  begin Makiri::XML("<r>\xC3</r>".b) rescue Makiri::XML::SyntaxError; end
  begin Makiri::XML("<r><a></r>")    rescue Makiri::XML::SyntaxError; end
  begin Makiri::XML("<r/>", max_bytes: 1) rescue Makiri::XML::LimitExceeded; end
  begin Makiri::XML(%(<?xml version="1.1"?><r/>)) rescue Makiri::XML::SyntaxError; end
  x.xpath("//d:a", "d" => "urn:d"); x.at_xpath("//p:b", "p" => "urn:p")
  begin x.xpath("//unbound:a") rescue Makiri::Error; end
  x.css("a"); x.at_css("p|b", "p" => "urn:p")
  begin x.css("a[") rescue Makiri::CSS::SyntaxError; end
  x.to_xml; x.to_xml(pretty: true); x.root.canonicalize
  el = x.create_element("n", "t", "k" => "v"); x.root.add_child(el)
  begin x.root.add_child(x.create_element("zz:q")) rescue Makiri::Error; end
  x.root.children.first.replace(x.create_element("rep")); x.fragment("<f1/><f2/>")
  Makiri::XML::Builder.new { |b| b.root("xmlns:d" => "urn:d") { b.item("a"); b["d"].q } }.to_xml

  # --- XPathContext: AST cache / registrations / failing evaluate ---
  ctx = Makiri::XPathContext.new(x)
  ctx.register_namespace("d", "urn:d"); ctx.register_variable("v", "1")
  ctx.evaluate("//d:a[@id=$v]"); ctx.evaluate("//d:a[@id=$v]")
  begin ctx.evaluate("//(") rescue Makiri::XPath::SyntaxError; end

  # --- Lexbor CSS stylesheet parser (per-call parser+stylesheet lifetime,
  # freed under rb_ensure) including the NUL-reject raise path ---
  Makiri::Lexbor::CSS.parse_stylesheet(CSS)
  begin Makiri::Lexbor::CSS.parse_stylesheet("a{}\0x") rescue Makiri::Error; end
end

GC.start
GC.start
puts "leaks harness done (#{ITERATIONS} iterations)"
