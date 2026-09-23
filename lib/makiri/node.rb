# frozen_string_literal: true

module Makiri
  # Base class for every DOM node (element, attribute, text, comment, ...).
  # The bulk of the API lives in the extension; this file defines the
  # Ruby-only conveniences.
  class Node
    # Order by document (pre-order) position
    include Comparable

    # Enumerable over its child nodes, like Nokogiri.
    include Enumerable

    # #clone is a deep #dup that honours +freeze:+. A frozen node is genuinely
    # immutable - its mutators raise +FrozenError+.
    include CloneViaDup

    # #path, an XPath that finds this node again.
    include NodePath

    # @yieldparam child [Makiri::Node]
    # @return [self, Enumerator]
    def each(&block)
      return enum_for(:each) { children.length } unless block_given?

      children.each(&block)
      self
    end

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
    def cdata?
      is_a?(CDATASection)
    end

    # @return [Boolean]
    def attribute?
      is_a?(Attr)
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

    # @return [Boolean] true for a blank/whitespace-only text or CDATA node.
    def blank?
      (text? || cdata?) && content.strip.empty?
    end

    # --- Nokogiri-compatible aliases over the core API ---
    #
    # Aliases of the representation-specific readers (#[], #name, ...) are
    # applied to each NodeMethods module by {ReaderAliases}. These two alias
    # representation-independent predicates defined just above, so they stay.
    alias_method :elem?, :element?
    alias_method :fragment?, :document_fragment?

    # Set an attribute (alias for {#[]=}). @return [String]
    def set_attribute(name, value)
      self[name] = value
    end

    # The Attr node named +name+, or nil (cf. {#[]}, which returns the value).
    # @return [Makiri::Attr, nil]
    def attribute(name)
      wanted = name.to_s
      attribute_nodes.find { |attr| attr.name == wanted }
    end

    # --- CSS class helpers (operate on the `class` attribute) ---

    # @return [Array<String>] the element's class names.
    def classes
      self["class"].to_s.split(/\s+/).reject(&:empty?)
    end

    # Add each class in +names+ (space-separated) that is not already present.
    # @return [self]
    def add_class(names)
      have = classes
      have.concat(names.to_s.split(/\s+/).reject { |c| c.empty? || have.include?(c) })
      self["class"] = have.join(" ")
      self
    end

    # Append each class in +names+ unconditionally (duplicates allowed).
    # @return [self]
    def append_class(names)
      self["class"] = (classes + names.to_s.split(/\s+/).reject(&:empty?)).join(" ")
      self
    end

    # Remove each class in +names+ (or every class when +names+ is nil); drops
    # the `class` attribute entirely when none remain.
    # @return [self]
    def remove_class(names = nil)
      if names.nil?
        delete("class")
      else
        remaining = classes - names.to_s.split(/\s+/)
        remaining.empty? ? delete("class") : (self["class"] = remaining.join(" "))
      end
      self
    end

    # Yield this node and every descendant, depth-first, children before self
    # (post-order, matching Nokogiri).
    # @return [self]
    def traverse(&block)
      children.each { |child| child.traverse(&block) }
      block.call(self)
      self
    end

    # The root element of the owning document (e.g. <html>).
    # @return [Makiri::Element, nil]
    def root
      document.root
    end

    # Attributes as a name => Attr Hash (empty for non-elements).
    # @return [Hash{String => Makiri::Attr}]
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

    # Inspect representation. Avoids dumping the whole subtree.
    def inspect
      "#<#{self.class.name} name=#{name.inspect}>"
    end

    # An independent copy of this node, detached from any parent and owned by the
    # same document (like Nokogiri's Node#dup). Deep by default; +level+ 0 makes
    # a shallow copy (matching Nokogiri's level argument). The native allocator
    # is undef'd to keep wrappers memory-safe, so #dup/#clone delegate to
    # {#clone_node} rather than Ruby's default allocate-and-copy (which would
    # otherwise raise "allocator undefined"). #clone is this, deep, via
    # {CloneViaDup}.
    def dup(level = 1)
      clone_node(level != 0)
    end

    private

    # Heuristic used by {#search}: does +path+ look like an XPath location path
    # rather than a CSS selector?
    def xpath?(path)
      s = path.to_s.strip
      s.start_with?("/", "./", "../", ".//", "(", "@") || s.include?("::")
    end
  end
end
