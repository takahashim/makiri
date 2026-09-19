# Differential probe for glue/ruby_html_node.c -> glue::html_node.
#
# Prints one line per (fixture, node, method) so two builds can be diffed
# byte-for-byte. It also COUNTS the interesting branches, so a run that exercises
# none of them cannot pass as agreement - the counts are part of the output.
# The probes are run from spec/differential/run.rb, which puts lib/ on the
# load path; this makes them work when run directly too. The original line
# here expanded "lib" against THIS directory, which does not exist - it had
# been doing nothing, and the -Ilib on the command line was carrying it.
$LOAD_PATH.unshift File.expand_path("../../../lib", __dir__)
require "makiri"

FIXTURES = {
  "plain" => <<~H,
    <!doctype html>
    <html><head><title>T</title></head>
    <body>
      <div id="a" class="x y" DATA-X="1">A<span>B<i>C</i></span>D</div>
      <div id="b">café\nüñ<p>déép<b>!</b></p></div>
      <div id="empty"></div>
      <!-- a comment -->
      <?pi-target pi data?>
    </body></html>
  H
  "doctype-public" => %(<!DOCTYPE html PUBLIC "-//W3C//DTD HTML 4.01//EN" "http://www.w3.org/TR/html4/strict.dtd"><p>x</p>),
  "doctype-system" => %(<!DOCTYPE html SYSTEM "about:legacy-compat"><p>x</p>),
  "doctype-bare"   => %(<!DOCTYPE html><p>x</p>),
  "svg"            => %(<div><svg viewBox="0 0 1 1"><path d="M0 0"/><a xlink:href="#z" xml:lang="en">t</a></svg></div>),
  "math"           => %(<div><math><mi>x</mi></math></div>),
  "template"       => %(<div><template><i>inside</i>text</template></div>),
  "template-empty" => %(<div><template></template></div>),
  "custom"         => %(<my-widget my-attr="1">c</my-widget>),
  "detached"       => %(<div id="r"><p id="p1">1</p><p id="p2">2</p></div>),
}.freeze

COUNTS = Hash.new(0)

def note(k) = COUNTS[k] += 1

def show(v)
  case v
  when nil then "nil"
  when String then "#{v.encoding.name}:#{v.inspect}"
  when Array then "[#{v.map { |e| show(e) }.join(", ")}]"
  when Makiri::NodeSet then "NodeSet(#{v.length})[#{v.map { |n| show_node_id(n) }.join(",")}]"
  else "#{v.class}:#{v.inspect}"
  end
end

def show_node_id(n)
  return "nil" if n.nil?
  "#{n.class.name.split("::").last}/#{begin n.name rescue "?" end}"
end

def try(label)
  yield
rescue => e
  "RAISE #{e.class}: #{e.message}"
end

READERS_0 = %i[
  name namespace_uri prefix local_name tag_name target node_type
  content text inner_text value line
  keys values
].freeze

NAV_0 = %i[
  parent next next_sibling previous previous_sibling next_element previous_element
  child children element_children elements first_element_child last_element_child
  ancestors
].freeze

def walk(node, &blk)
  blk.call(node)
  node.children.each { |c| walk(c, &blk) }
  node.attribute_nodes.each { |a| blk.call(a) } if node.respond_to?(:attribute_nodes)
end

out = []

FIXTURES.each do |fname, html|
  doc = Makiri::HTML(html)
  nodes = []
  # From the DOCUMENT's children, not the root element: the doctype is a sibling
  # of <html>, so a walk rooted at <html> reaches no DocumentType at all - which
  # is how the first run of this probe reported doctype=0 while claiming to cover
  # the doctype readers.
  doc.children.each { |c| walk(c) { |n| nodes << n } }
  nodes.unshift(doc)

  nodes.each_with_index do |n, i|
    tag = "#{fname}[#{i}]#{show_node_id(n)}"

    note(:document) if n.is_a?(Makiri::HTML::Document)
    note(:element) if n.is_a?(Makiri::HTML::Element)
    note(:attr) if n.is_a?(Makiri::HTML::Attr)
    note(:text) if n.is_a?(Makiri::HTML::Text)
    note(:comment) if n.is_a?(Makiri::HTML::Comment)
    note(:pi) if n.is_a?(Makiri::HTML::ProcessingInstruction)
    note(:doctype) if n.is_a?(Makiri::HTML::DocumentType)
    note(:cdata) if n.is_a?(Makiri::HTML::CDATASection)
    note(:fragment) if n.is_a?(Makiri::HTML::DocumentFragment)

    READERS_0.each do |m|
      next unless n.respond_to?(m)
      v = try("#{tag}.#{m}") { n.public_send(m) }
      note(:"prefixed_#{m}") if m == :prefix && v.is_a?(String)
      note(:ns_nonnil) if m == :namespace_uri && v.is_a?(String)
      out << "#{tag}.#{m} = #{show(v)}"
    end

    NAV_0.each do |m|
      next unless n.respond_to?(m)
      out << "#{tag}.#{m} = #{show(try("#{tag}.#{m}") { n.public_send(m) })}"
    end

    if n.respond_to?(:attribute_nodes)
      out << "#{tag}.attribute_nodes = #{show(try(tag) { n.attribute_nodes })}"
    end

    %w[id class DATA-X data-x href xlink:href xml:lang viewBox my-attr missing d].each do |k|
      %i[[] key? attribute_by_qualified_name attribute_value_by_qualified_name].each do |m|
        next unless n.respond_to?(m)
        v = try("#{tag}.#{m}(#{k})") { n.public_send(m, k) }
        note(:qname_hit) if m == :attribute_by_qualified_name && !v.nil? && !v.is_a?(String)
        note(:qvalue_hit) if m == :attribute_value_by_qualified_name && v.is_a?(String)
        out << "#{tag}.#{m}(#{k.inspect}) = #{show(v)}"
      end
    end

    if n.respond_to?(:public_id)
      note(:doctype_ids)
      out << "#{tag}.public_id = #{show(try(tag) { n.public_id })}"
      out << "#{tag}.external_id = #{show(try(tag) { n.external_id })}"
      out << "#{tag}.system_id = #{show(try(tag) { n.system_id })}"
    end

    if n.respond_to?(:content_fragment)
      cf = try(tag) { n.content_fragment }
      note(:content_fragment) unless cf.nil? || cf.is_a?(String)
      out << "#{tag}.content_fragment = #{show(cf)}"
      unless cf.nil? || cf.is_a?(String)
        note(:fragment_text)
        out << "#{tag}.content_fragment.text = #{show(try(tag) { cf.text })}"
        out << "#{tag}.content_fragment.children = #{show(try(tag) { cf.children })}"
      end
    end
  end

  # Document order across every pair, including the nil (incomparable) cases.
  pairs = nodes.first(14)
  pairs.each_with_index do |a, i|
    pairs.each_with_index do |b, j|
      v = try("cmp") { a <=> b }
      note(:cmp_nil) if v.nil?
      note(:cmp_nonnil) unless v.nil?
      out << "#{fname}.cmp[#{i},#{j}] = #{show(v)}"
    end
  end

  # Identity, which must stay pointer-based and shared with XML.
  a = doc.at_css("p") || doc.root
  out << "#{fname}.eq_self = #{a == doc.at_css("p") || a == doc.root}"
  out << "#{fname}.hash_eq = #{a.hash == (doc.at_css("p") || doc.root).hash}"
end

# A ProcessingInstruction, which HTML5 parsing never yields (it tokenises
# <?...?> as a bogus comment), so #target's non-nil branch is reachable only
# through the factory. The first run of this probe reported pi=0.
pidoc = Makiri::HTML("<p>x</p>")
pi = pidoc.create_processing_instruction("xml-stylesheet", %(href="s.css"))
pidoc.at_css("p").add_child(pi)
%i[name target node_type content text value prefix local_name namespace_uri tag_name line].each do |m|
  out << "pi.#{m} = #{show(try("pi") { pi.public_send(m) })}"
end
out << "pi.parent = #{show_node_id(try("pi") { pi.parent })}"
out << "pi.cmp = #{show(try("pi") { pi <=> pidoc.at_css("p") })}"
note(:pi)

# Cross-representation and error paths.
xdoc = Makiri::XML("<r><c/></r>")
hdoc = Makiri::HTML("<p>x</p>")
out << "xml_vs_html.cmp = #{show(try("x") { hdoc.at_css("p") <=> xdoc.root })}"
out << "html_vs_nonnode.cmp = #{show(try("x") { hdoc.at_css("p") <=> 42 })}"
out << "html_reader_on_xml = #{show(try("x") { Makiri::HTML::NodeMethods.instance_method(:name).bind(xdoc.root).call })}"
note(:xml_cmp)

# A detached subtree: no common root.
d = Makiri::HTML("<div><p id='p1'>1</p><p id='p2'>2</p></div>")
p1 = d.at_css("#p1")
p1.unlink
out << "detached.cmp = #{show(try("x") { p1 <=> d.at_css("#p2") })}"
out << "detached.text = #{show(try("x") { p1.text })}"
out << "detached.parent = #{show_node_id(try("x") { p1.parent })}"
note(:detached)

# A fragment is not in the text index, so #text must take the walk fallback.
frag = Makiri::HTML::DocumentFragment.parse("<b>f1</b><i>f2</i>")
out << "fragment.text = #{show(try("x") { frag.text })}"
out << "fragment.children = #{show(try("x") { frag.children })}"
note(:fragment_walk)

# Invalid UTF-8 in an attribute name must raise, not truncate.
out << "bad_utf8_attr = #{show(try("x") { hdoc.at_css("p")["\xff".dup.force_encoding("UTF-8")] })}"
note(:bad_utf8)

puts out
puts "---- branch counts ----"
COUNTS.sort.each { |k, v| puts "#{k}=#{v}" }
