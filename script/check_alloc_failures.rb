# frozen_string_literal: true

# Allocation-failure injection sweep for the C extension (run via `rake oom`).
#
# The sanitizers and the leak gate prove the happy path is memory-safe; neither
# proves the OOM branches are CORRECT. Makiri's contract is fail-closed: when a
# core C allocation fails, a call must either raise a clean Makiri::Error /
# NoMemoryError or complete with the exact same result as the unfailed run -
# never a truncated/partial result (the property the XPath node-set caps and
# the "build OOM -> walk fallback" designs exist for). This gate machine-checks
# that contract: with the ext built under MAKIRI_ALLOC_INJECT=1, every core
# allocation site routes through a hook that can be armed to fail the nth
# attempt once. For each representative workload we record a failure-free
# BASELINE result and the total number of allocation attempts, then re-run the
# workload once per allocation site with exactly that site failing, and verify
# each run either raised cleanly or returned a baseline-identical value.
#
# A segfault/abort kills this process; the caller (rake/CI) sees the nonzero
# exit, which is the verdict too.
#
#   bundle exec rake oom                                    # rebuild + sweep
#   bundle exec ruby -Ilib script/check_alloc_failures.rb   # sweep current build

require "makiri"

unless Makiri.send(:__alloc_inject?)
  abort "check_alloc_failures: extension built without the injection hook - " \
        "rebuild with MAKIRI_ALLOC_INJECT=1 (`rake oom` does this)"
end

# Each scenario runs one workload END-TO-END and returns a canonical String, so
# an injected run's result can be compared (==) against the baseline. Fixtures
# are built INSIDE the lambda (unless reuse is the point) so the sweep covers
# their parse/build allocations too.
SCENARIOS = {
  # XML parse covering the syntax surface: declaration, DOCTYPE (SYSTEM id +
  # internal subset), default + prefixed namespaces, prefixed attributes,
  # references, comment, CDATA, PI, nesting, CRLF normalization in an attr.
  "xml_parse" => lambda do
    src = <<~XML
      <?xml version="1.0" encoding="UTF-8"?>
      <!DOCTYPE root SYSTEM "urn:example:dtd" [<!ENTITY local "subset">]>
      <root xmlns="urn:d" xmlns:p="urn:p" p:pa="pv" mixed="A&amp;&#x41; x\r\ny">
        <!-- a comment -->
        <p:branch><leaf depth="2">text &amp; &#x41; refs</leaf></p:branch>
        <![CDATA[raw < cdata & bytes]]>
        <?pi-target some data?>
        <empty/>
      </root>
    XML
    Makiri::XML::Document.parse(src).to_xml
  end,

  # Fragment parse + serialize + splice into a host document.
  "xml_fragment" => lambda do
    doc  = Makiri::XML::Document.parse("<r xmlns='urn:d'><keep>k</keep></r>")
    frag = doc.fragment("<a xmlns:p='u'><p:b>t</p:b></a>text")
    out  = frag.to_xml
    doc.root.add_child(frag)
    out + doc.to_xml
  end,

  # XPath battery: predicates, functions, a union, axes, plus an XPathContext
  # evaluation with a registered namespace and variable.
  "xml_xpath" => lambda do
    doc = Makiri::XML::Document.parse(<<~XML)
      <root xmlns:p="urn:p">
        <a v="1">alpha</a>
        <a v="2">beta</a>
        <b n="3"> gamma  delta </b>
        <p:c><a v="9">nested</a></p:c>
      </root>
    XML
    canon = lambda do |r|
      r.is_a?(Makiri::NodeSet) ? r.map(&:to_xml).join("|") : r.inspect
    end
    exprs = [
      "//a[@v='2']",
      "//a[position()=2]",
      "//a[last()]",
      "count(//a)",
      "sum(//b/@n)",
      "concat(string(//a[1]), '-', substring('abcdef', 2, 3))",
      "translate('abc', 'abc', 'xyz')",
      "normalize-space(//b)",
      "contains(//a[1], 'alp')",
      "starts-with(//b, ' g')",
      "//a | //b",
      "//a[1]/ancestor::root",
      "//a[1]/following-sibling::b",
      "//root/descendant-or-self::a",
    ]
    parts = exprs.map { |e| canon.call(doc.xpath(e)) }
    ctx = Makiri::XPathContext.new(doc)
    ctx.register_namespace("p", "urn:p")
    ctx.register_variable("want", "9")
    parts << canon.call(ctx.evaluate("//p:c/a[@v=$want]"))
    parts.join("\n")
  end,

  # Same battery shape over an HTML5-parsed document.
  "html_xpath" => lambda do
    doc = Makiri::HTML::Document.parse(<<~HTML)
      <html><body>
        <div id="top"><p class="x">one</p><p class="y">two</p></div>
        <ul><li data-n="1">a</li><li data-n="2"> b  c </li></ul>
      </body></html>
    HTML
    canon = lambda do |r|
      r.is_a?(Makiri::NodeSet) ? r.map(&:to_html).join("|") : r.inspect
    end
    exprs = [
      "//p[@class='y']",
      "//li[position()=2]",
      "//li[last()]",
      "count(//p)",
      "sum(//li/@data-n)",
      "concat(string(//p[1]), '+', substring(//p[2], 1, 2))",
      "translate('one', 'one', 'uno')",
      "normalize-space(//li[2])",
      "contains(//p[1], 'on')",
      "starts-with(//p[2], 'tw')",
      "//p | //li",
      "//p[1]/ancestor::div",
      "//p[1]/following-sibling::p",
      "//div/descendant-or-self::p",
    ]
    parts = exprs.map { |e| canon.call(doc.xpath(e)) }
    ctx = Makiri::XPathContext.new(doc)
    ctx.register_variable("cls", "x")
    parts << canon.call(ctx.evaluate("//p[@class=$cls]"))
    parts.join("\n")
  end,

  # The mutation surface: create_*, insertion on every side, rename, content,
  # attributes, replace, remove, and a fragment splice.
  "xml_mutate" => lambda do
    doc = Makiri::XML::Document.parse("<root><old>x</old><gone/></root>")
    el = doc.create_element("made")
    el.add_child(doc.create_text_node("inner"))
    el["k"] = "v"
    doc.root.add_child(el)
    el.name = "renamed"
    el.content = "rewritten"
    el.add_previous_sibling(doc.create_element("before"))
    el.add_next_sibling(doc.create_element("after"))
    doc.root.at_xpath("old").replace(doc.create_element("new"))
    doc.root.at_xpath("gone").remove
    doc.root.add_child(doc.fragment("<f1/>tail<f2 a='b'/>"))
    doc.to_xml
  end,

  # Serialization over a non-trivial tree (~200 elements built inside the
  # lambda, so the parse is swept too), both tree and deep serializers.
  "html_serialize" => lambda do
    body = (1..50).map { |i|
      "<div id='d#{i}' class='row'><p>cell #{i}</p><span>tail &amp; #{i}</span></div>"
    }.join
    doc = Makiri::HTML::Document.parse("<html><body>#{body}</body></html>")
    doc.to_html + doc.at_css("body").inner_html
  end,

  # Full-document text extraction (exercises the text-index build, and its
  # fail-closed OOM -> walk fallback).
  "html_text" => lambda do
    body = (1..80).map { |i| "<p>para #{i} <em>em#{i}</em> tail</p>" }.join
    doc = Makiri::HTML::Document.parse("<html><body>#{body}</body></html>")
    doc.text
  end,

  # The HTML node readers, which reach three lazily-built C structures the other
  # scenarios do not: the attr->owner index (Attribute#parent), the line table
  # (#line) and the NodeSet builder (#children / #ancestors / #attribute_nodes).
  # Each has its own OOM branch, and each must fail closed - a NodeSet that
  # silently loses a member reads exactly like a correct shorter one.
  "html_node_read" => lambda do
    doc = Makiri::HTML::Document.parse(<<~HTML)
      <!DOCTYPE html PUBLIC "-//W3C//DTD HTML 4.01//EN" "http://www.w3.org/TR/html4/strict.dtd">
      <html><body>
        <div id="a" class="x y" data-n="1">A<span>B<i>C</i></span>D</div>
        <div id="b">café<p>déép<b>!</b></p></div>
        <svg viewBox="0 0 1 1"><a xlink:href="#z" xml:lang="en">t</a></svg>
        <template><i>inside</i></template>
      </body></html>
    HTML
    out = []
    stack = doc.children.to_a
    until stack.empty?
      n = stack.pop
      stack.concat(n.children.to_a)
      # #line is deliberately absent here. Its line table is built once at parse
      # time and is ALLOWED to fail: dom_adapter/post_parse.c documents the
      # degradation, and Node#line's own contract is "an Integer, or nil when no
      # line is available", so answering nil after an allocation failure is
      # within the contract rather than a wrong result. Attribute#parent is the
      # opposite case - nil there means "no parent", a navigation answer with no
      # such allowance - so it IS swept, and now raises instead of degrading.
      out << [n.name, n.local_name, n.prefix, n.namespace_uri, n.node_type,
              n.text].inspect
      next unless n.is_a?(Makiri::HTML::Element)
      out << n.keys.inspect << n.values.inspect
      out << n.attribute_nodes.map { |a| [a.name, a.value, a.parent&.name] }.inspect
      out << n.ancestors.map(&:name).inspect
      out << n.attribute_by_qualified_name("xlink:href")&.value.inspect
      out << n.attribute_value_by_qualified_name("data-n").inspect
      out << n.content_fragment&.text.inspect
    end
    dt = doc.children.find { |c| c.is_a?(Makiri::HTML::DocumentType) }
    out << [dt&.public_id, dt&.system_id].inspect
    out.join("\n")
  end,

  # The HTML mutation surface: the factories, insertion on every side, the
  # fragment splice, cross-document adopt, rename, content, the namespaced
  # attribute setters and inner_html=. Its XML twin (xml_mutate) covers the
  # other backend; this one reaches the Lexbor arena, the <template> fixup and
  # the transient-document free that a raise must not skip.
  "html_mutate" => lambda do
    d = Makiri::HTML::Document.parse("<html><body><div id='a'><p>one</p></div></body></html>")
    a = d.at_css("#a")
    a.add_child(d.create_element("made"))
    a.add_child(d.create_text_node("inner"))
    a.add_child(d.create_comment(" note "))
    a.add_child(d.create_processing_instruction("tgt", "pd"))
    a.add_child(d.fragment("<i>i</i><u>u</u>"))
    p1 = d.at_css("p")
    p1.add_previous_sibling(d.create_element("prev"))
    p1.add_next_sibling(d.create_element("next"))
    p1.name = "h1"
    p1.content = "renamed"
    p1["data-n"] = "1"
    p1.set_attribute_ns("http://www.w3.org/1999/xlink", "xlink:href", "#x")
    p1.set_attribute_ns(nil, "plain", "v")
    p1.remove_attribute_ns("http://www.w3.org/1999/xlink", "href")
    p1.delete("data-n")
    a.inner_html = "<b>B</b><template><i>f</i></template>"

    src = Makiri::HTML::Document.parse("<html><body><section id='s'><em>e</em></section></body></html>")
    a.add_child(src.at_css("#s"))
    d.at_css("b").outer_html = "<strong>S</strong>"
    d.at_css("strong").replace(d.create_element("r"))
    d.at_css("r").remove

    dt = d.create_document_type("html", "-//X//EN", "urn:s")
    d.root.add_previous_sibling(dt)
    d.to_html + "|" + src.to_html
  end,

  # CSS: a comma list with combinators through the reused engine, plus the
  # at_css first-match path.
  "css" => lambda do
    doc = Makiri::HTML::Document.parse(<<~HTML)
      <html><body>
        <p class="c">one</p><p>skip</p><p class="c">two</p>
        <div><span>in</span></div><span>out</span>
        <section id="x">target</section>
      </body></html>
    HTML
    doc.css("p.c, div > span").map { |n| n.name }.join(",") +
      doc.at_css("#x")&.name.to_s
  end,

  # The HTML fragment pipeline: parsing in a context, importing the result into
  # a document, and the <template>-content fixup that import_node omits.
  #
  # Nothing else here reaches it. `xml_fragment` and `xml_mutate` call
  # `doc.fragment`, but on an XML document, which is a different code path
  # entirely - so the whole of glue/fragment.rs, including the worklist its
  # template fixup allocates, had no scenario. The nesting is deliberate: a
  # template inside a template makes the fixup queue a second subtree, which is
  # the allocation worth failing.
  "html_fragment" => lambda do
    doc = Makiri::HTML::Document.parse(<<~HTML)
      <html><body>
        <div id=d><p>a</p></div>
        <template><i>x</i><template><b>deep</b></template></template>
      </body></html>
    HTML
    parts = []
    parts << doc.fragment("<template><i>f</i></template>").to_html
    parts << doc.fragment("<td>cell</td>", context: "tr").to_html
    parts << doc.at_css("div").parse("<span>s</span>").map(&:name).join(",")
    parts << Makiri::DocumentFragment.parse("<p>standalone</p>").to_html

    other = Makiri::HTML::Document.parse("<html><body></body></html>")
    parts << other.import_node(doc.at_css("div"), true).to_html
    # A template nested in a template: the fixup queues the inner subtree, which
    # is the allocation this scenario exists to fail.
    parts << doc.at_css("template").clone_node(true).to_html
    parts.join("|")
  end,

  # The stylesheet binding (Makiri::Lexbor::CSS.parse_stylesheet). Its own
  # layer allocates for every selector, declaration and at-rule name, and it
  # walks Lexbor's parsed tree into owned values BEFORE building any Ruby - so
  # a failure in the middle has a half-built intermediate to abandon, which is
  # exactly the shape this sweep exists to check.
  "css_stylesheet" => lambda do
    css = <<~CSS
      div.a, p#b > span { color: red; margin: 0 !important }
      p::before { content: "x" }
      @media (min-width: 600px) { .x { color: blue } }
      @font-face { font-family: F; src: url(f.woff) }
      @namespace svg url(http://www.w3.org/2000/svg);
    CSS
    Makiri::Lexbor::CSS.parse_stylesheet(css).inspect
  end,

  # The Builder DSL (pure Ruby over create_*/add_child, so this sweeps the
  # construction factories).
  "xml_builder" => lambda do
    Makiri::XML::Builder.new do |xml|
      xml.feed("xmlns" => "urn:a", "xmlns:dc" => "urn:dc") do
        xml.entry do
          xml.title("Hello")
          xml["dc"].id_("42")
          xml.cdata("a < b")
          xml.comment(" note ")
        end
      end
    end.to_xml
  end,
}.freeze

ALLOWED = [Makiri::Error, NoMemoryError].freeze
TRUNCATE = 120

def disarm = Makiri.send(:__alloc_inject, 0)

failures_total = 0

SCENARIOS.each do |name, work|
  # Warm twice with injection off: process-global engines (CSS) and lazy
  # builds settle, so the counted run below is representative and stable.
  disarm
  2.times { work.call }

  # Counted baseline run: __alloc_inject(0) also resets the counter, so the
  # calls reading right after is exactly this run's allocation-attempt total.
  disarm
  baseline = work.call
  total = Makiri.send(:__alloc_inject_calls)

  ok_raised = 0
  ok_identical = 0
  failures = []

  (1..total).each do |n|
    Makiri.send(:__alloc_inject, n)
    begin
      result = work.call
      if result == baseline
        ok_identical += 1
      else
        failures << [n, "truncated/wrong result",
                     "baseline=#{baseline.to_s[0, TRUNCATE].inspect} " \
                     "got=#{result.to_s[0, TRUNCATE].inspect}"]
      end
    rescue *ALLOWED
      ok_raised += 1
    rescue Exception => e # rubocop:disable Lint/RescueException -- the wrong class IS the finding
      failures << [n, "wrong exception class",
                   "#{e.class}: #{e.message.to_s[0, TRUNCATE]}"]
    ensure
      disarm
    end
  end

  failures_total += failures.size
  puts format("%-16s allocations=%-5d raised=%-5d identical=%-5d failed=%d",
              name, total, ok_raised, ok_identical, failures.size)
  failures.each do |n, kind, detail|
    puts "    n=#{n} #{kind}: #{detail}"
  end
  if total.zero?
    # A scenario that never reaches a core allocation sweeps nothing - that is
    # a broken scenario, not a pass.
    failures_total += 1
    puts "    scenario performed ZERO core allocations - workload not reaching the C core"
  end
end

if failures_total.zero?
  puts "check_alloc_failures: OK - every injected allocation failure failed closed " \
       "(clean raise or baseline-identical result)"
else
  puts "check_alloc_failures: FAILED - #{failures_total} injected failure(s) " \
       "produced a wrong exception or a non-baseline result"
  exit 1
end
