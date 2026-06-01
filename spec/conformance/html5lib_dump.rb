# frozen_string_literal: true
#
# Render a Makiri DOM in the html5lib-tests tree-construction dump format so it
# can be string-compared against the suite's expected output.
#
# Format (per the html5lib-tests README): one node per line, prefixed with
# "| " and indented two spaces per depth level. Document children start at
# depth 0.
#
#   | <html>                      element (HTML namespace)
#   |   <svg svg>                 foreign element: namespace prefix + space
#   |     attr="value"            attribute (sorted by name, depth+1)
#   |     xlink href="value"      foreign namespaced attribute: prefix + space
#   |   "text"                    text node (verbatim, newlines kept)
#   |   <!-- comment -->          comment
#   | <!DOCTYPE html>             doctype

module Html5libDump
  # Raised when a test's tree exercises DOM detail Makiri does not expose on
  # the Ruby surface (doctype public/system ids, <template> content). The
  # runner counts these as "unsupported" rather than scoring them as failures.
  class Unsupported < StandardError; end

  HTML_NS  = "http://www.w3.org/1999/xhtml"
  SVG_NS   = "http://www.w3.org/2000/svg"
  MATH_NS  = "http://www.w3.org/1998/Math/MathML"

  # The HTML spec's "adjust foreign attributes" table: when one of these
  # attributes appears on an SVG/MathML element, the parser puts it in a
  # namespace and html5lib dumps it as "prefix localname". On HTML elements the
  # same source text stays a plain attribute ("xlink:href"). Keyed by the raw
  # qualified attribute name Makiri reports.
  FOREIGN_ATTR = {
    "xlink:actuate" => "xlink actuate", "xlink:arcrole" => "xlink arcrole",
    "xlink:href" => "xlink href", "xlink:role" => "xlink role",
    "xlink:show" => "xlink show", "xlink:title" => "xlink title",
    "xlink:type" => "xlink type",
    "xml:lang" => "xml lang", "xml:space" => "xml space",
    "xmlns" => "xmlns xmlns", "xmlns:xlink" => "xmlns xlink",
  }.freeze

  module_function

  # Dump a whole Makiri::Document. Returns the multi-line String (no trailing
  # newline) matching an html5lib #document section.
  def dump_document(doc)
    # Fast path: namespace handling only matters when foreign content exists.
    # Detecting it once lets the common pure-HTML document skip per-node XPath.
    # We can't use doc.css("svg, math") — Lexbor's selector engine does not
    # descend into <template> content, so foreign content nested in a template
    # would be missed — so walk the tree (including template fragments).
    foreign = subtree_has_foreign?(doc)
    lines = []
    doc.children.each { |child| dump_node(child, 0, lines, foreign) }
    lines.join("\n")
  end

  # Dump a fragment's children at depth 0 (the html5lib #document form for a
  # #document-fragment test — no html/head/body wrapper). A fragment parsed in a
  # foreign context can contain SVG/MathML elements with no <svg>/<math> wrapper
  # to detect, so we always resolve each element's namespace here.
  def dump_fragment(frag)
    lines = []
    frag.children.each { |c| dump_node(c, 0, lines, true) }
    lines.join("\n")
  end

  # Does this subtree contain any SVG/MathML element? Descends into <template>
  # content fragments (which CSS/XPath over the host tree do not reach).
  def subtree_has_foreign?(node)
    node.children.each do |c|
      next unless c.node_type == 1 # element

      return true if c.name == "svg" || c.name == "math"
      return true if subtree_has_foreign?(c)
    end
    cf = node.node_type == 1 ? node.content_fragment : nil
    cf ? subtree_has_foreign?(cf) : false
  end

  def dump_node(node, depth, lines, foreign)
    pad = "| " + ("  " * depth)
    case node.node_type
    when 1 # element
      lines << pad + element_open(node, foreign)
      dump_attributes(node, depth + 1, lines, foreign)
      # A <template>'s parsed nodes live in its content fragment, not as
      # children; html5lib renders them under a "content" pseudo-node. Makiri
      # exposes the fragment via Element#content_fragment.
      cf = node.content_fragment
      if cf
        lines << "| " + ("  " * (depth + 1)) + "content"
        cf.children.each { |c| dump_node(c, depth + 2, lines, foreign) }
      else
        node.children.each { |c| dump_node(c, depth + 1, lines, foreign) }
      end
    when 3 # text
      # Raw characters wrapped in double quotes (newlines kept literal) — NOT
      # Ruby String#inspect escaping.
      lines << pad + %("#{node.content}")
    when 8 # comment
      lines << pad + "<!-- #{node.content} -->"
    when 10 # doctype
      lines << pad + doctype(node)
    when 7 # processing instruction (not produced by the HTML parser; be explicit)
      raise Unsupported, "processing instruction"
    else
      raise Unsupported, "node_type #{node.node_type}"
    end
  end

  def element_open(node, foreign)
    name = node.name
    return "<#{name}>" unless foreign

    case namespace_of(node)
    when SVG_NS  then "<svg #{name}>"
    when MATH_NS then "<math #{name}>"
    else "<#{name}>"
    end
  end

  def dump_attributes(node, depth, lines, foreign)
    attrs = node.attribute_nodes
    return if attrs.empty?

    el_foreign = foreign && namespace_of(node) != HTML_NS
    pad = "| " + ("  " * depth)
    rendered = attrs.map do |a|
      key = a.name
      disp = el_foreign ? (FOREIGN_ATTR[key] || key) : key
      [disp, %(#{disp}="#{a.value}")]
    end
    # html5lib sorts attributes by the (possibly namespaced) attribute name.
    rendered.sort_by!(&:first)
    rendered.each { |(_, line)| lines << pad + line }
  end

  def doctype(node)
    # html5lib renders "<!DOCTYPE name>" when the doctype has no public/system
    # id, and "<!DOCTYPE name "public" "system">" (missing side shown as "")
    # when either is present.
    name = node.name.to_s
    pub  = node.public_id
    sys  = node.system_id
    if pub.nil? && sys.nil?
      "<!DOCTYPE #{name}>"
    else
      %(<!DOCTYPE #{name} "#{pub}" "#{sys}">)
    end
  end

  # Namespace URI of an element, via XPath. Only ever called when the document
  # is known to contain foreign content, so the per-node cost is paid on the
  # rare documents that need it, not on pure-HTML ones.
  def namespace_of(node)
    node.xpath("namespace-uri(.)")
  rescue StandardError
    HTML_NS
  end

end
