# frozen_string_literal: true

module Makiri
  module XML
    # XML-specific document conveniences. The XML node leaves and the document
    # itself are defined in C (ext/makiri/glue/ruby_xml*.c); construction sugar
    # that is pure composition over the public surface lives here, not on the
    # abstract Makiri::Document (which carries no construction).
    class Document
      # Set (or replace) the document's root element: with an existing root it
      # replaces that root, otherwise it appends one (subject to the single-root
      # rule). Pure composition over {Node#replace} / {Node#add_child};
      # Nokogiri-compatible. XML only - an HTML5 document has a fixed
      # html/head/body structure, so a free-form root is not meaningful there.
      #
      # @param node [Makiri::XML::Element]
      # @return [Makiri::XML::Element] the node
      def root=(node)
        r = root
        r ? r.replace(node) : add_child(node)
      end
    end
  end
end
