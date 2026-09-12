# Differential probe for dom_adapter/cross_import.c -> dom_adapter::cross_import.
#
# Document#import_node across the two representations, in both directions, over
# the shapes that reach a distinct branch: namespaces (default, redeclared,
# undeclared, prefixed attributes, xml:), <template> content, every node type
# including the two that fail closed, deep vs shallow, and the DOM-loose element
# name that is valid HTML but not a well-formed XML QName.
#
# Branch counts are printed, so a run that exercised none of them cannot pass as
# agreement.
# The probes are run from spec/differential/run.rb, which puts lib/ on the
# load path; this makes them work when run directly too. The original line
# here expanded "lib" against THIS directory, which does not exist - it had
# been doing nothing, and the -Ilib on the command line was carrying it.
$LOAD_PATH.unshift File.expand_path("../../../lib", __dir__)
require "makiri"

COUNTS = Hash.new(0)
def note(k) = COUNTS[k] += 1

OUT = []
def say(tag, v) = OUT << "#{tag} = #{v}"

def try
  yield
rescue => e
  note(:raised)
  "RAISE #{e.class}: #{e.message}"
end

def show(n)
  case n
  when nil then "nil"
  when String then n.inspect
  when Makiri::Node
    body = try { n.respond_to?(:to_xml) ? n.to_xml : n.to_html }
    "#{n.class.name.split("::").last}:#{body.inspect}"
  else "#{n.class}:#{n.inspect}"
  end
end

# ---- HTML -> XML ---------------------------------------------------------
HTML_CASES = {
  "plain" => "<div id='a' class='c'>text<b>bold</b></div>",
  "nested-deep" => "<div><p><span><i><u>deep</u></i></span></p></div>",
  "svg-default-ns" => "<div><svg viewBox='0 0 1 1'><path d='M0 0'/></svg></div>",
  "svg-xlink-attr" => "<div><svg><a xlink:href='#z'>t</a></svg></div>",
  "svg-xml-lang" => "<div><svg><text xml:lang='en'>t</text></svg></div>",
  "math" => "<div><math><mi>x</mi></math></div>",
  "mixed-ns" => "<div><svg><foreignObject><p>back in html</p></foreignObject></svg></div>",
  "template" => "<div><template><i>inside</i>tail</template></div>",
  "template-empty" => "<div><template></template></div>",
  "template-nested" => "<div><template><template><b>deep</b></template></template></div>",
  "comment" => "<div>a<!-- c -->b</div>",
  "custom-el" => "<my-widget my-attr='1'>c</my-widget>",
  "entities" => "<div>a &amp; b &lt; c</div>",
  "empty" => "<div></div>",
  "attr-order" => "<div z='1' a='2' m='3'>x</div>",
}.freeze

xdoc = Makiri::XML("<root/>")

HTML_CASES.each do |name, html|
  hdoc = Makiri::HTML("<body>#{html}</body>")
  src = hdoc.at_css("body").children.first
  note(:h2x)
  [true, false].each do |deep|
    imported = try { xdoc.import_node(src, deep) }
    say("h2x[#{name}].deep=#{deep}", show(imported))
    next if imported.is_a?(String)

    note(:h2x_ok)
    # Linking is where the synthesized xmlns declarations actually resolve, so
    # serialize AFTER linking too - a declaration that is merely present but
    # does not resolve would look right until this step.
    holder = try { xdoc.create_element("h") }
    unless holder.is_a?(String)
      try { holder.add_child(imported) }
      say("h2x[#{name}].deep=#{deep}.linked", show(holder))
      say("h2x[#{name}].deep=#{deep}.ns",
          try { holder.children.map { |c| [c.name, c.namespace_uri] }.inspect })
      note(:h2x_linked)
    end
  end
end

# Text, comment and PI nodes on their own.
hdoc = Makiri::HTML("<p>t<!--c--></p>")
[hdoc.at_css("p").children.first, hdoc.at_css("p").children.last].each_with_index do |n, i|
  say("h2x.leaf[#{i}]", show(try { xdoc.import_node(n, true) }))
end
pi = hdoc.create_processing_instruction("tgt", "data")
say("h2x.pi", show(try { xdoc.import_node(pi, true) }))
say("h2x.fragment", show(try { xdoc.import_node(hdoc.fragment("<i>f</i><b>g</b>"), true) }))
say("h2x.doctype", show(try { xdoc.import_node(hdoc.create_document_type("html"), true) }))
note(:h2x_leaves)

# A name that is a valid HTML element name but NOT a well-formed XML QName:
# the loose-name escape hatch.
%w[0bad x:y:z].each do |bad|
  el = try { hdoc.create_element(bad) }
  next if el.is_a?(String)

  say("h2x.loose[#{bad}]", show(try { xdoc.import_node(el, false) }))
  note(:h2x_loose)
end

# ---- XML -> HTML ---------------------------------------------------------
XML_CASES = {
  "plain" => "<r><a id='1'>t</a></r>",
  "default-ns" => "<r xmlns='urn:d'><a>t</a></r>",
  "prefixed" => "<r xmlns:p='urn:p'><p:a p:k='v'>t</p:a></r>",
  "mixed-ns" => "<r xmlns='urn:d' xmlns:p='urn:p'><a><p:b>t</p:b></a></r>",
  "cdata" => "<r><![CDATA[raw < data]]></r>",
  "comment" => "<r>a<!-- c -->b</r>",
  "pi" => "<r><?tgt some data?></r>",
  "deep" => "<r><a><b><c><d>x</d></c></b></a></r>",
  "empty-el" => "<r><a/></r>",
  "xml-lang" => "<r><a xml:lang='en'>t</a></r>",
}.freeze

hdoc2 = Makiri::HTML("<body><div id='dst'></div></body>")

XML_CASES.each do |name, xml|
  xd = Makiri::XML(xml)
  note(:x2h)
  [true, false].each do |deep|
    imported = try { hdoc2.import_node(xd.root, deep) }
    say("x2h[#{name}].deep=#{deep}", show(imported))
    next if imported.is_a?(String)

    note(:x2h_ok)
    say("x2h[#{name}].deep=#{deep}.ns",
        try { [imported.name, imported.namespace_uri].inspect })
    say("x2h[#{name}].deep=#{deep}.attrs",
        try { imported.attribute_nodes.map { |a| [a.name, a.namespace_uri, a.value] }.inspect })
  end
  # The CDATA child specifically: HTML has no CDATA section, so importing one
  # must fail closed rather than degrade to text.
  cdata = xd.root.children.find { |c| c.is_a?(Makiri::XML::CDATASection) }
  next unless cdata

  say("x2h[#{name}].cdata_child", show(try { hdoc2.import_node(cdata, true) }))
  note(:x2h_cdata)
end

# An XML subtree whose root is a fragment.
xf = Makiri::XML("<r/>")
frag = try { xf.fragment("<a>1</a><b>2</b>") }
say("x2h.fragment", show(try { hdoc2.import_node(frag, true) })) unless frag.is_a?(String)
note(:x2h_fragment)

# Round trip: HTML -> XML -> HTML.
rt_src = Makiri::HTML("<body><div><svg><path d='M0 0'/></svg><p>t</p></div></body>").at_css("div")
rt_x = try { xdoc.import_node(rt_src, true) }
unless rt_x.is_a?(String)
  say("roundtrip.xml", show(rt_x))
  say("roundtrip.html", show(try { hdoc2.import_node(rt_x, true) }))
  note(:roundtrip)
end

puts OUT
puts "---- branch counts ----"
COUNTS.sort.each { |k, v| puts "#{k}=#{v}" }
