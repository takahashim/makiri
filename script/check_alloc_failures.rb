# frozen_string_literal: true

# Allocation-failure injection sweep for the extension (run via `rake oom`).
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

  # The mutation surface: create_*, insertion on every side, content,
  # attributes, replace, remove, and a fragment splice.
  "xml_mutate" => lambda do
    doc = Makiri::XML::Document.parse("<root><old>x</old><gone/></root>")
    el = doc.create_element("made")
    el.add_child(doc.create_text_node("inner"))
    el["k"] = "v"
    doc.root.add_child(el)
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
      # time and is ALLOWED to fail: lexbor/adapter/post_parse.rs documents the
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

  # Cross-representation import, which no other scenario reaches: it allocates
  # in BOTH arenas at once and synthesizes xmlns declarations, and its
  # fail-closed model is "abandon the partial subtree in the destination arena".
  # A partial import that still returned a node would be a silently truncated
  # tree - the shape this whole sweep is looking for.
  "cross_import" => lambda do
    hdoc = Makiri::HTML::Document.parse(<<~HTML)
      <html><body><div id="a" class="c">text<b>bold</b>
        <svg viewBox="0 0 1 1"><a xlink:href="#z"><path d="M0 0"/></a></svg>
        <template><i>inside</i></template>
        <p>a &amp; b</p>
      </div></body></html>
    HTML
    xdoc = Makiri::XML::Document.parse("<root xmlns='urn:d'><keep/></root>")

    # HTML -> XML, then LINKED: the synthesized declarations only resolve at
    # link time, so a half-built one shows up here rather than in the copy.
    imported = xdoc.import_node(hdoc.at_css("#a"), true)
    xdoc.root.add_child(imported)

    # XML -> HTML, both directions in one scenario.
    src = Makiri::XML::Document.parse(
      "<r xmlns='urn:d' xmlns:p='urn:p'><p:a p:k='v'>t</p:a><b>u</b></r>"
    )
    back = hdoc.import_node(src.root, true)
    hdoc.at_css("body").add_child(back)

    xdoc.to_xml + "|" + hdoc.to_html
  end,

  # Invalid UTF-8 input, which is the ONLY path that reaches the sanitiser's
  # buffer: every other scenario feeds valid UTF-8, where `utf8_sanitize`
  # (lexbor/adapter/utf8_input.rs) short-circuits and allocates nothing. The 3x growth and the steal are what
  # is being swept here, and a truncated document is exactly the failure the
  # property forbids.
  "html_invalid_utf8" => lambda do
    bad = (1..200).map { |i| "<p id='p#{i}'>a\xC3(b \xE0\x80\x80 \xF4\x90\x80\x80 \xED\xA0\x80</p>" }.join
    doc = Makiri::HTML::Document.parse("<html><body>#{bad}</body></html>".dup.force_encoding("BINARY"))
    frag = doc.fragment("<i>\xC3\x28</i><b>\xF1\x80</b>".dup.force_encoding("BINARY"))
    doc.text + "|" + frag.to_html
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

  # CSS on XML: a different engine from the HTML one above - the selector is
  # lowered (css/) into an XPath AST, every node of which is a falloc site. One
  # selector per lowering shape: combinators, attribute operators, the nth and
  # of-type arithmetic, the selector-list pseudo-classes, :lexbor-contains and
  # namespaced type/universal selectors.
  "xml_css" => lambda do
    doc = Makiri::XML::Document.parse(<<~XML)
      <root xmlns:p="urn:p">
        <a id="x" class="c d" lang="en-GB">alpha</a>
        <b v="pre-mid-suf"/><a>Beta</a><p:c><a>nested</a></p:c>
      </root>
    XML
    ns = { "p" => "urn:p" }
    [
      "root > a + b ~ a, p|c a",
      "#x.d[lang|=en][class~=c]",
      "[v^=pre][v$=suf][v*=mid]",
      ":nth-child(2n+1):not(:last-child)",
      "a:nth-last-of-type(1), :first-of-type:only-of-type",
      ":is(root > a, p|*):where(a) :empty",
      "root:has(> b + a) a:lexbor-contains(\"BETA\" i)",
      "*|a, |b, p|*",
    ].map { |s| doc.css(s, ns).map { |n| n.name }.join(",") }.join("\n")
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
  # inner_html= / outer_html= are all or nothing: an allocation failure part-way
  # must leave the tree - and the text read through its index - as it was.
  # A partial edit raises a RuntimeError here, which is not a clean raise.
  "html_inner_html" => lambda do
    d = Makiri::HTML::Document.parse(
      "<html><body><div id=a><p>old</p> text</div><div id=b><p>keep</p></div></body></html>"
    )
    edits = [
      [d.at_css("#a"), :inner_html=, "<i>n</i><b>e</b><u>w</u>"],
      [d.at_css("#b p"), :outer_html=, "<s>x</s><em>y</em>"],
    ]
    edits.each do |node, verb, src|
      before = [d.to_html, d.text]
      begin
        node.public_send(verb, src)
      rescue *ALLOWED
        raise "#{verb} left a partial edit" unless [d.to_html, d.text] == before

        raise
      end
    end
    [d.to_html, d.text].join("|")
  end,
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

STATEFUL_SCENARIOS = {
  "html_text_index" => {
    setup: lambda do
      body = (1..20).map { |i| "<p>para #{i} <em>em#{i}</em> tail</p>" }.join
      doc = Makiri::HTML::Document.parse("<html><body>#{body}</body></html>")
      expected = (1..20).map { |i| "para #{i} em#{i} tail" }.join
      { doc: doc, expected: expected }
    end,
    action: ->(state) { state[:doc].text },
    check: lambda do |state, _outcome|
      doc = state[:doc]
      raise "text changed after allocation failure" unless doc.text == state[:expected]
      raise "element index changed after allocation failure" unless doc.xpath("//em").length == 20

      GC.compact
      doc.at_css("p").content = "after"
      raise "document was not reusable after text-index failure" unless doc.at_css("p").text == "after"
    end,
  },
  "xml_context" => {
    setup: lambda do
      doc = Makiri::XML::Document.parse(<<~XML)
        <root xmlns:p="urn:p">
          <a v="1">alpha</a><a v="2">beta</a><p:c><a v="9">nested</a></p:c>
        </root>
      XML
      ctx = Makiri::XPathContext.new(doc)
      ctx.register_namespace("p", "urn:p")
      ctx.register_variable("warm", "9")
      raise "XPathContext warmup failed" unless ctx.evaluate("count(//p:c/a[@v=$warm])") == 1.0

      { doc: doc, ctx: ctx, before: doc.to_xml }
    end,
    action: ->(state) { state[:ctx].evaluate("//p:c/a").length },
    snapshot: ->(state) { state[:doc].to_xml },
    unchanged: :always,
    check: lambda do |state, _outcome|
      raise "document changed during XPath evaluation" unless state[:doc].to_xml == state[:before]

      GC.compact
      ctx = state[:ctx]
      raise "context became unreadable after evaluation failure" unless ctx.evaluate("//a[@v='1']").length == 1
      raise "cached expression failed after evaluation failure" unless ctx.evaluate("//a[@v='2']").length == 1
      raise "prefixed expression failed after evaluation failure" unless ctx.evaluate("//p:c/a").length == 1

      ctx.register_variable("later", "9")
      raise "context could not register after evaluation failure" unless ctx.evaluate("//p:c/a[@v=$later]").length == 1
      state[:doc].root["data-after"] = "ok"
      raise "document was not mutable after evaluation failure" unless state[:doc].root["data-after"] == "ok"
    end,
  },
  "xml_content" => {
    setup: lambda do
      doc = Makiri::XML::Document.parse(%(<root xmlns:p="urn:p"><old id="i" p:k="v"><child>old</child></old></root>))
      target = doc.root.at_xpath("old")
      child = target.children.first
      attr = target.attribute_nodes.find { |a| a.name == "p:k" }
      before = [doc.to_xml, doc.text, target.name, child.name, child.text,
                attr.name, attr.value, target.parent.name, attr.parent.name]
      { doc: doc, target: target, child: child, attr: attr, before: before }
    end,
    action: ->(state) { state[:target].content = "rewritten" },
    snapshot: lambda do |state|
      [state[:doc].to_xml, state[:doc].text, state[:target].name,
       state[:child].name, state[:child].text, state[:attr].name,
       state[:attr].value, state[:target].parent&.name, state[:attr].parent&.name]
    end,
    unchanged: true,
    check: lambda do |state, outcome|
      current = [state[:doc].to_xml, state[:doc].text, state[:target].name,
                 state[:child].name, state[:child].text, state[:attr].name,
                 state[:attr].value, state[:target].parent&.name, state[:attr].parent&.name]
      if outcome == :raised && current != state[:before]
        raise "content= changed the tree before allocation failed"
      end
      if outcome == :success && (state[:target].children.length != 1 || state[:target].text != "rewritten")
        raise "content= returned success without the replacement text"
      end
      if outcome == :success
        raise "detached child was not detached" unless state[:child].parent.nil?
      elsif !state[:child].parent.equal?(state[:target])
        raise "failed content= detached a child"
      end
      raise "detached child changed" unless state[:child].text == "old"
      raise "retained attribute changed" unless state[:attr].value == "v"
      raise "retained attribute changed owner" unless state[:attr].parent.equal?(state[:target])

      GC.compact
      raise "retained child became unreadable" unless state[:child].text == "old"
      state[:target]["data-after"] = "ok"
      raise "document was not reusable after content= failure" unless state[:doc].xpath("//old[@data-after='ok']").length == 1
    end,
  },
  "cross_import_state" => {
    setup: lambda do
      html = Makiri::HTML::Document.parse(<<~HTML)
        <html><body><div id="a"><span data-x="1">text</span><svg><path/></svg><template><i>inside</i></template></div></body></html>
      HTML
      xml = Makiri::XML::Document.parse(%(<root xmlns="urn:d"><keep/></root>))
      source = html.at_css("#a")
      { html: html, xml: xml, source: source, before: [html.to_html, xml.to_xml] }
    end,
    action: lambda do |state|
      imported = state[:xml].import_node(state[:source], true)
      state[:imported] = imported
      imported.to_xml
    end,
    snapshot: ->(state) { [state[:html].to_html, state[:xml].to_xml] },
    unchanged: :always,
    check: lambda do |state, outcome|
      current = [state[:html].to_html, state[:xml].to_xml]
      raise "import changed a visible document" unless current == state[:before]
      if state[:imported]
        raise "imported subtree is linked" unless state[:imported].parent.nil?
        span = state[:imported].children.find { |child| child.name == "span" }
        raise "imported subtree changed" unless span&.text == "text"
      end

      GC.compact
      raise "source wrapper became unreadable" unless state[:source].text.include?("text")
      state[:html].at_css("#a")["data-after"] = "ok"
      state[:xml].root["data-after"] = "ok"
      raise "HTML document was not reusable after import failure" unless state[:html].css("#a[data-after]").length == 1
      raise "XML document was not reusable after import failure" unless state[:xml].root["data-after"] == "ok"
    end,
  },
}.freeze

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
    puts "    scenario performed ZERO core allocations - workload not reaching the Rust core"
  end
end

STATEFUL_SCENARIOS.each do |name, spec|
  disarm
  baseline_state = spec[:setup].call
  disarm
  baseline = spec[:action].call(baseline_state)
  total = Makiri.send(:__alloc_inject_calls)

  ok_raised = 0
  ok_identical = 0
  failures = []

  begin
    spec[:check].call(baseline_state, :success)
  rescue Exception => e # rubocop:disable Lint/RescueException -- post-check failures are findings
    failures << [0, "baseline post-check failed", "#{e.class}: #{e.message.to_s[0, TRUNCATE]}"]
  end

  total.times do |index|
    n = index + 1
    disarm
    state = spec[:setup].call
    before = spec[:snapshot]&.call(state)
    disarm
    outcome = :success
    result = nil
    action_error = nil

    Makiri.send(:__alloc_inject, n)
    begin
      result = spec[:action].call(state)
    rescue *ALLOWED => e
      outcome = :raised
      action_error = e
    rescue Exception => e # rubocop:disable Lint/RescueException -- the wrong class IS the finding
      outcome = :wrong
      action_error = e
    ensure
      disarm
    end

    if outcome == :success
      if result == baseline
        ok_identical += 1
      else
        failures << [n, "truncated/wrong stateful result",
                     "baseline=#{baseline.to_s[0, TRUNCATE].inspect} " \
                     "got=#{result.to_s[0, TRUNCATE].inspect}"]
      end
    elsif outcome == :raised
      ok_raised += 1
    else
      failures << [n, "wrong exception class",
                   "#{action_error.class}: #{action_error.message.to_s[0, TRUNCATE]}"]
    end

    begin
      if spec[:unchanged] == :always || (spec[:unchanged] && outcome == :raised)
        current = spec[:snapshot].call(state)
        if current != before
          failures << [n, "state changed on failure", "before != after"]
        end
      end
      spec[:check].call(state, outcome)
    rescue Exception => e # rubocop:disable Lint/RescueException -- reuse failure IS the finding
      failures << [n, "same-object post-check failed", "#{e.class}: #{e.message.to_s[0, TRUNCATE]}"]
    end
  end

  failures_total += failures.size
  puts format("%-16s allocations=%-5d raised=%-5d identical=%-5d failed=%d",
              name, total, ok_raised, ok_identical, failures.size)
  failures.each do |n, kind, detail|
    puts "    n=#{n} #{kind}: #{detail}"
  end
  if total.zero?
    failures_total += 1
    puts "    scenario performed ZERO core allocations - stateful workload did not reach the Rust core"
  end
end

if failures_total.zero?
  puts "check_alloc_failures: OK - every injected allocation failure failed closed " \
       "(clean raise or baseline-identical result, with stateful objects reusable)"
else
  puts "check_alloc_failures: FAILED - #{failures_total} injected failure(s) " \
       "produced a wrong exception, result, or post-failure state"
  exit 1
end
