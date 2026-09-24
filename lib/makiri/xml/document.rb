# frozen_string_literal: true

module Makiri
  module XML
    # XML-specific document conveniences. The XML node leaves and the document
    # itself are defined in the extension (ext/makiri/rust/src/glue/xml_doc.rs);
    # construction sugar that is pure composition over the public surface lives here, not on the
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

      # An independent copy of the whole document (like Nokogiri's
      # Document#dup), sharing no nodes with the original. A document with a
      # root is serialised and re-parsed, like {Makiri::HTML::Document#dup}. One
      # without (still being built) is not well-formed, so it cannot be; what it
      # can hold before a root - comments and processing instructions - is
      # imported node by node instead. Any level argument is ignored, and
      # #clone is this too (see {CloneViaDup}).
      #
      # The copy is parsed with the default +max_bytes+, not the original's, and
      # its Node#line numbers are lines of the serialised text, which can
      # differ from the original source's.
      #
      # @return [Makiri::XML::Document]
      def dup(*)
        return self.class.parse(to_xml) if root

        self.class.new.tap do |copy|
          children.each { |child| copy.add_child(copy.import_node(child, true)) }
        end
      end
    end
  end
end
