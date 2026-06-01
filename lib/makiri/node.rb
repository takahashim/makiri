# frozen_string_literal: true

module Makiri
  # Base class for every DOM node (element, attribute, text, comment, ...).
  # The bulk of the API lives in the C extension; this file defines the
  # Ruby-only conveniences.
  class Node
    # Identity is by wrapped node pointer; defined in C.

    # @return [Boolean]
    def element?
      is_a?(Element)
    end

    # @return [Boolean]
    def text?
      is_a?(Text)
    end

    # @return [Boolean]
    def comment?
      is_a?(Comment)
    end

    # @return [Boolean]
    def attribute?
      is_a?(Attribute)
    end

    # @return [Boolean]
    def document?
      is_a?(Document)
    end

    # @return [Boolean]
    def processing_instruction?
      is_a?(ProcessingInstruction)
    end

    # @return [Boolean]
    def document_fragment?
      is_a?(DocumentFragment)
    end

    # The root element of the owning document (e.g. <html>).
    # @return [Makiri::Element, nil]
    def root
      document.root
    end

    # Attributes as a name => Attribute Hash (empty for non-elements).
    # @return [Hash{String => Makiri::Attribute}]
    def attributes
      attribute_nodes.each_with_object({}) { |attr, h| h[attr.name] = attr }
    end

    # Attributes as a plain name => value Hash (empty for non-elements).
    # @return [Hash{String => String}]
    def to_h
      attribute_nodes.each_with_object({}) { |attr, h| h[attr.name] = attr.value }
    end

    # Query with CSS or XPath, auto-detecting which from the string shape.
    # Strings that look like a location path (start with "/", "./", "..", ".//",
    # "(", "@" or contain "::") are treated as XPath; everything else as CSS.
    # @return [Makiri::NodeSet, String, Float, Boolean]
    def search(path)
      xpath?(path) ? xpath(path) : css(path)
    end

    # First result of {#search}: the first node for a node-set, else the value.
    def at(path)
      result = search(path)
      result.is_a?(NodeSet) ? result.first : result
    end

    # An absolute XPath that locates this node, e.g. "/html/body/p[2]".
    # Element/text/comment steps carry a 1-based position among same-kind
    # siblings (omitted when unique); attributes use "@name". Round-trips
    # through {#at_xpath}.
    # @return [String]
    def path
      return "/" if document?

      segments = []
      node = self
      until node.nil? || node.document?
        segments.unshift(node.send(:path_segment))
        node = node.parent
      end
      "/#{segments.join("/")}"
    end

    # Inspect representation. Avoids dumping the whole subtree.
    def inspect
      "#<#{self.class.name} name=#{name.inspect}>"
    rescue NoMethodError
      "#<#{self.class.name}>"
    end

    private

    # Heuristic used by {#search}: does +path+ look like an XPath location path
    # rather than a CSS selector?
    def xpath?(path)
      s = path.to_s.strip
      s.start_with?("/", "./", "../", ".//", "(", "@") || s.include?("::")
    end

    # One "/"-separated step of {#path} for this node.
    def path_segment
      return "@#{name}" if attribute?

      parent_node = parent
      return step_name unless parent_node

      siblings = parent_node.children.select { |c| same_step_kind?(c) }
      return step_name if siblings.length <= 1

      "#{step_name}[#{siblings.index(self) + 1}]"
    end

    # The node-test portion of a path step (without any position predicate).
    def step_name
      if text?
        "text()"
      elsif comment?
        "comment()"
      else
        name
      end
    end

    # Whether +other+ shares this node's path-step kind (for position counting).
    def same_step_kind?(other)
      if element?
        other.element? && other.name == name
      elsif text?
        other.text?
      elsif comment?
        other.comment?
      else
        false
      end
    end
  end
end
