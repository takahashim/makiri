# frozen_string_literal: true

module Makiri
  module XML
    # Ruby additions over the native XML node readers, so the XML node surface
    # matches the HTML one (Makiri::HTML::NodeMethods).
    module NodeMethods
      # Element ancestors, nearest first, excluding self - matching
      # Makiri::HTML's native #ancestors.
      # @return [Array<Makiri::XML::Element>]
      def ancestors
        out = []
        node = parent
        while node
          out << node if node.element?
          node = node.parent
        end
        out
      end

      # Whether an attribute with the given qualified name is present
      # (case-sensitive, per XML).
      def key?(name)
        wanted = name.to_s
        attribute_nodes.any? { |attr| attr.name == wanted }
      end

      # After #key?, which #has_attribute? aliases.
      ReaderAliases.define_on(self)

      # CSS selector queries over XML, matched by the same engine as HTML's,
      # as a browser's querySelectorAll matches them in an XML document: names
      # and values compare as written, a type selector without a prefix
      # matches its name in any namespace, and the context node's ancestors
      # take part in the match while only its descendants are returned. An
      # +ns+ hash of {prefix => uri} declares the prefixes the selector may
      # use; its "xmlns" entry, if any, is the default namespace a bare type
      # selector is then in.
      #
      #   doc.css("entry")                    # <entry> in any namespace
      #   doc.css("a|entry", "a" => uri)      # <entry> in uri
      #   doc.css("entry", "xmlns" => uri)    # the same, as a default
      #
      # @return [Makiri::NodeSet]
      def css(selector, ns = nil)
        _css(selector.to_s, ns)
      end

      # First descendant matching +selector+, or nil. @return [Makiri::Node, nil]
      def at_css(selector, ns = nil)
        _at_css(selector.to_s, ns)
      end

      # Whether this node matches +selector+ (full selector, combinators included).
      # @return [Boolean]
      def matches?(selector, ns = nil)
        _css_matches(selector.to_s, ns)
      end

      private

      # The namespace an unprefixed XPath element name test selects here: none,
      # per XPath 1.0 (see {NodePath#path}).
      def unprefixed_element_namespace
        nil
      end
    end
  end
end
