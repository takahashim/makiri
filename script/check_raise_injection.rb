# frozen_string_literal: true

# Ruby-raise injection sweep (run by `rake oom`, after the allocation sweep).
#
# A Document's content is reached through counted borrow guards
# (`bridge::dom`), and a guard is a Rust `Drop`: a Ruby raise `longjmp`s past
# Rust frames without running it, so a raise while a guard is held would leave
# the document borrowed for good - every later edit (or read) refused.
# `rake unsafe:boundaries` checks statically that no guard is live across a
# call that can raise, but by the text of the code; this checks it by making
# every such call raise.
#
# With the extension built under MAKIRI_ALLOC_INJECT=1, each allocating C call
# of `bridge::raw` consults a hook that can be armed to raise `NoMemoryError`
# in place of the nth call. For each workload we count the calls a disarmed run
# makes and record its result, then re-run it once per call with that call
# raising, and verify after each run:
#
#   - it raised `NoMemoryError`, or returned the baseline result;
#   - no document it touched is left borrowed (`Makiri.__borrowed?`);
#   - every such document still reads and edits.
#
# What it cannot reach: magnus's own allocating calls (`ary_new`, `str_new`,
# `Array#push`, ...), which do not go through `bridge::raw`.
#
#   bundle exec rake oom                                     # rebuild + both sweeps
#   bundle exec ruby -Ilib script/check_raise_injection.rb   # this sweep, current build

require "makiri"

unless Makiri.send(:__alloc_inject?)
  abort "check_raise_injection: extension built without the injection hook - " \
        "rebuild with MAKIRI_ALLOC_INJECT=1 (`rake oom` does this)"
end

LONG = ("a long run of text, longer than the 64 bytes lifted to the stack; " * 3).freeze

HTML_SRC = <<~HTML
  <!DOCTYPE html>
  <html><head><title>t</title></head><body>
  <main id="main" class="c">
    <p id="p1" data-k="v" lang="en">short <b>bold</b> text</p>
    <p id="p2" data-long="#{LONG}">#{LONG}</p>
    <ul>#{(1..40).map { |i| %(<li class="item i#{i}">item #{i}</li>) }.join}</ul>
    <svg xmlns:xlink="http://www.w3.org/1999/xlink"><a xlink:href="#x"><path d="M0"/></a></svg>
    <template><i>in template</i></template>
    <!-- a comment -->
  </main>
  </body></html>
HTML

XML_SRC = <<~XML
  <?xml version="1.0"?>
  <!DOCTYPE r>
  <r xmlns="urn:d" xmlns:p="urn:p" p:a="pv" k="v">
    <p:x long="#{LONG}">#{LONG}</p:x>
    <list>#{(1..40).map { |i| %(<i n="#{i}">#{i}</i>) }.join}</list>
    <?pi data?><!-- c --><![CDATA[cd]]>
  </r>
XML

# A handler for the XPath scenarios: answers strings and node-sets back, so a
# raise can land in the conversion either way.
HANDLER = Class.new do
  def echo(nodes) = nodes
  def label(nodes) = "n#{nodes.size}"
end.new

# Each scenario runs a workload and returns a String to compare with the
# baseline; every Document it makes goes into `docs` as soon as it exists, so a
# run that raised midway is still checked.
SCENARIOS = {
  "html_read" => lambda do |docs|
    doc = Makiri::HTML(HTML_SRC)
    docs << doc
    out = []
    main = doc.at_css("#main")
    p1 = doc.at_css("#p1")
    p2 = doc.at_css("#p2")
    path = doc.at_css("path")
    a = doc.at_xpath("//svg:a", "svg" => "http://www.w3.org/2000/svg")
    ul = doc.at_css("ul")
    [p1, p2, path, a].compact.each do |n|
      out << [n.name, n.local_name, n.prefix, n.namespace_uri, n.tag_name].inspect
      out << [n.keys, n.values, n.attribute_nodes.map(&:name)].inspect
    end
    out << [p1["id"], p2["data-long"], p1.key?("lang"), p1.attribute_by_qualified_name("data-k")&.value].inspect
    out << a&.attribute_value_ns("http://www.w3.org/1999/xlink", "href").inspect
    3.times { out << p2.text << doc.text.size.to_s } # the text index comes due
    out << ul.children.size.to_s << ul.element_children.size.to_s << path.ancestors.size.to_s
    out << [p1.parent&.name, p1.next&.name, p2.previous&.name, ul.child&.name, ul.last_child&.name].inspect
    out << [ul.child_count, ul.child_at(39)&.text, p1.line].inspect
    out << main.inner_html << p1.to_html << doc.at_css("template").content_fragment.to_html
    out << doc.css("li.item").size.to_s << doc.css("p").map(&:text).join("|")
    out << [doc.at_css("li:nth-child(7)")&.text, p1.matches?("p#p1")].inspect
    out << doc.xpath("//li[@class='item i3']").map(&:text).join
    out << doc.xpath("string(//p[@id='p2'])") << doc.xpath("count(//li)").to_s
    ctx = Makiri::XPathContext.new(doc)
    out << ctx.evaluate("echo(//li)", HANDLER).size.to_s << ctx.evaluate("label(//p)", HANDLER)
    out << (ul.children | doc.css("p")).size.to_s
    out.join("\n")
  end,

  "html_edit" => lambda do |docs|
    doc = Makiri::HTML(HTML_SRC)
    other = Makiri::HTML("<div id='o'><span>moved</span><em>kept</em></div>")
    docs << doc << other
    p1 = doc.at_css("#p1")
    p1["data-new"] = "x"
    p1["data-long2"] = LONG
    doc.at_css("svg a").set_attribute_ns("http://www.w3.org/1999/xlink", "xlink:title", "t")
    doc.at_css("#p2").content = "replaced"
    doc.at_css("ul").inner_html = "<li>one</li><li>two #{LONG}</li>"
    doc.at_css("b").remove
    p1.delete("lang")
    p1.add_child(other.at_css("span"))                       # adopted from another document
    doc.at_css("main").add_child(doc.create_element("section"))
    doc.at_css("section").add_child(doc.create_text_node("t"))
    doc.at_css("main") << doc.import_node(other.at_css("em"), true)
    doc.at_css("main").add_child(doc.at_css("#p2").clone_node(true))
    doc.at_css("main").add_child(doc.fragment("<i>f</i><i>g</i>"))
    doc.to_html + other.to_html
  end,

  "xml_read" => lambda do |docs|
    doc = Makiri::XML(XML_SRC)
    docs << doc
    out = []
    root = doc.root
    x = root.element_children.first
    list = root.element_children[1]
    [root, x].each do |n|
      out << [n.name, n.local_name, n.prefix, n.namespace_uri, n.tag_name].inspect
      out << [n.keys, n.values, n.attribute_nodes.map(&:name)].inspect
    end
    out << [root["k"], x["long"], root.attribute_value_ns("urn:p", "a")].inspect
    out << [root.namespace&.href, root.namespaces.inspect, doc.collect_namespaces.inspect,
            root.namespace_definitions.size].inspect
    out << x.text << root.text.size.to_s
    out << list.children.size.to_s << list.element_children.size.to_s
    out << list.children.last.ancestors.size.to_s
    out << [x.parent&.name, x.next&.name, list.first_element_child&.name, list.last_element_child&.name].inspect
    out << [list.child_count, list.child_at(39)&.text].inspect
    out << doc.css("i").size.to_s << doc.at_css("i")&.text.to_s
    out << doc.xpath("//d:i[@n='3']", "d" => "urn:d").map(&:text).join
    out << doc.xpath("string(//p:x)", "p" => "urn:p")
    ctx = Makiri::XPathContext.new(doc)
    ctx.register_namespace("d", "urn:d")
    out << ctx.evaluate("echo(//d:i)", HANDLER).size.to_s
    out << doc.to_xml << doc.canonicalize
    out.join("\n")
  end,

  "xml_edit" => lambda do |docs|
    doc = Makiri::XML(XML_SRC)
    other = Makiri::XML("<o><m>moved</m><k>kept</k></o>")
    html = Makiri::HTML("<p id='h'>html <b>node</b></p>")
    docs << doc << other << html
    root = doc.root
    root["new"] = "x"
    root["long2"] = LONG
    root.set_attribute_ns("urn:p", "p:b", "pb")
    root.element_children.first.content = "replaced #{LONG}"
    root.delete("k")
    root.add_child(other.root.element_children.first)          # adopted
    root.add_child(doc.create_element("e"))
    root << doc.import_node(other.root.element_children.first, true)
    root << doc.import_node(html.at_css("p"), true)
    root << doc.fragment("<f1/><f2>t</f2>")
    root.element_children[1].remove
    doc.to_xml + other.to_xml
  end,
}

# A document a run left still works: it reads, and it takes an edit.
def check_usable(doc)
  if doc.is_a?(Makiri::XML::Document)
    root = doc.root or return
    root.add_child(doc.create_element("probe"))
    doc.to_xml
  else
    host = doc.at_css("body") || doc.root or return
    host.add_child(doc.create_element("probe"))
    doc.to_html
  end
end

failures = []
SCENARIOS.each do |name, run|
  Makiri.__raise_inject(0)
  baseline = run.call([])
  calls = Makiri.__raise_inject_calls
  raised = 0
  (1..calls).each do |n|
    docs = []
    outcome = nil
    Makiri.__raise_inject(n)
    begin
      result = run.call(docs)
      outcome = result == baseline ? :same : :differs
    rescue NoMemoryError
      outcome = :raised
      raised += 1
    rescue Exception => e # rubocop:disable Lint/RescueException
      outcome = e
    ensure
      Makiri.__raise_inject(0)
    end
    failures << "#{name} ##{n}: the result differs from the baseline" if outcome == :differs
    failures << "#{name} ##{n}: raised #{outcome.class}: #{outcome.message}" if outcome.is_a?(Exception)
    docs.each_with_index do |doc, i|
      if Makiri.__borrowed?(doc)
        failures << "#{name} ##{n}: document #{i} left borrowed"
        next
      end
      begin
        check_usable(doc)
      rescue Exception => e # rubocop:disable Lint/RescueException
        failures << "#{name} ##{n}: document #{i} unusable after: #{e.class}: #{e.message}"
      end
    end
  end
  printf("%-12s calls=%-5d raised=%d\n", name, calls, raised)
  failures << "#{name}: no raising call to inject into" if calls.zero?
end

if failures.empty?
  puts "check_raise_injection: OK - every injected raise left every document unborrowed and usable"
else
  puts failures.first(40)
  abort "check_raise_injection: #{failures.size} failure(s)"
end
