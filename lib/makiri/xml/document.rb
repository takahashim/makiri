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

      # An independent copy of the whole document (like Nokogiri's Document#dup),
      # made node for node rather than by serialising and re-parsing: the
      # top-level order, every name, namespace URI and namespace state,
      # attributes, the doctype and character data are the original's. Nothing
      # has to be writable as XML, no namespace is resolved again (a re-parse
      # gave an attribute set in a namespace an invented `ns1:` prefix), and the
      # copy keeps the original's +max_bytes+ budget and whether it declared an
      # encoding. Any level argument is ignored, and #clone is this too (see
      # {CloneViaDup}).
      #
      # @return [Makiri::XML::Document]
      def dup(*)
        _copy
      end

      # Always false: an XML document is in no-quirks mode, as the DOM has it.
      # The same name as {Makiri::HTML::Document#quirks_mode?}, so a caller
      # need not ask which kind of document it holds.
      # @return [false]
      def quirks_mode?
        false
      end

      # The DOM's +document.compatMode+, which is always "CSS1Compat" for an
      # XML document.
      # @return [String]
      def compat_mode
        "CSS1Compat"
      end
    end
  end
end
