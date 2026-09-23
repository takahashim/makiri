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

      # CSS selector queries over XML, lowered to the native XPath engine (so
      # matching is case-sensitive and namespace-aware, unlike a Lexbor HTML
      # matcher). Nokogiri-compatible namespaces: the root element's
      # declarations are collected automatically (a bare type selector binds to
      # the default namespace), unless an +ns+ hash of {prefix => uri} is
      # given, which then replaces them.
      #
      #   doc.css("entry")               # default-namespace bound (Atom/RSS just work)
      #   doc.css("a|entry", "a" => uri) # explicit prefix
      #
      # @return [Makiri::NodeSet]
      def css(selector, ns = nil)
        _css(selector.to_s, _css_namespaces(ns))
      end

      # First descendant matching +selector+, or nil. @return [Makiri::Node, nil]
      def at_css(selector, ns = nil)
        _at_css(selector.to_s, _css_namespaces(ns))
      end

      # Whether this node matches +selector+ (full selector, combinators included).
      # @return [Boolean]
      def matches?(selector, ns = nil)
        _css_matches(selector.to_s, _css_namespaces(ns))
      end

      private

      # The namespace an unprefixed XPath element name test selects here: none,
      # per XPath 1.0 (see {NodePath#path}).
      def unprefixed_element_namespace
        nil
      end

      # Build the {prefix => uri} hash the native primitives register. Matching
      # Nokogiri: with NO explicit namespaces the document's own declarations are
      # collected (the default namespace under the synthetic prefix "xmlns", so a
      # bare type selector binds to it - the RSS/Atom common case); but once the
      # caller passes a namespaces hash, ONLY those prefixes are used and a bare
      # selector resolves to no namespace (Nokogiri disables the default binding
      # the moment an explicit map is given). Reading only the root's
      # declarations is O(root attributes), not the whole-document walk.
      def _css_namespaces(user)
        return user.transform_keys(&:to_s).transform_values(&:to_s) if user && !user.empty?

        reg = {}
        root = document&.root
        root&.namespace_definitions&.each do |ns|
          reg[ns.prefix.nil? ? "xmlns" : ns.prefix] = ns.href
        end
        reg
      end
    end
  end
end
