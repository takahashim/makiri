# frozen_string_literal: true

module Makiri
  # A detached group of sibling nodes. Build one with {.parse} (its own backing
  # document) or {Makiri::Document#fragment} (bound to an existing document, so
  # its nodes can be spliced in with {Node#add_child} and friends). Inserting a
  # fragment contributes its children, not the fragment node itself.
  #
  # Both +.parse+ and +Document#fragment+ accept a Nokogiri-compatible
  # <tt>context:</tt> keyword naming the element the HTML is parsed inside of
  # (the fragment-parsing algorithm is context-sensitive). It may be:
  #   * a tag-name String (HTML namespace), e.g. <tt>context: "tr"</tt>; the
  #     bare strings <tt>"svg"</tt> / <tt>"math"</tt> name the foreign roots;
  #   * a {Makiri::Node} element - its tag and namespace are used (the way to
  #     reach a foreign non-root context such as an SVG <desc>).
  # The default context is <tt><body></tt>. See also {Makiri::Node#parse}.
  #
  # +.parse+ is defined in C (ext/makiri/glue/ruby_doc.c).
  class DocumentFragment < Node
  end
end
