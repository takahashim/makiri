# frozen_string_literal: true

module Makiri
  # A detached group of sibling nodes. Build one with {.parse} (its own backing
  # document) or {Makiri::Document#fragment} (bound to an existing document, so
  # its nodes can be spliced in with {Node#add_child} and friends). Inserting a
  # fragment contributes its children, not the fragment node itself.
  #
  # The concrete classes are {Makiri::HTML::DocumentFragment} and
  # {Makiri::XML::DocumentFragment}; their fragment-parsing context differs:
  #
  # * HTML is context-sensitive: <tt>.parse</tt> / <tt>Document#fragment</tt>
  #   accept a Nokogiri-compatible <tt>context:</tt> keyword naming the element
  #   the HTML is parsed inside of - a tag-name String (HTML namespace, e.g.
  #   <tt>context: "tr"</tt>; the bare strings <tt>"svg"</tt> / <tt>"math"</tt>
  #   name the foreign roots), or a {Makiri::Node} element whose tag and namespace
  #   are used. The default context is <tt><body></tt>. (Defined in C, ruby_html_*.c.)
  # * XML is namespace-context-based (no <tt>context:</tt> keyword):
  #   {Makiri::XML::DocumentFragment.parse} is self-contained (a prefix must be
  #   declared within the fragment itself), while {Makiri::XML::Document#fragment}
  #   resolves names against the document's in-scope namespaces. (C: ruby_xml.c.)
  #
  # See also {Makiri::Node#parse}.
  class DocumentFragment < Node
  end
end
