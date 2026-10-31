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

    # The kind predicates ask the class with `Class#===`, which is what
    # `is_a?` does, minus a method dispatch on the receiver - they run once a
    # node in a tree walk.

    # @return [Boolean]
    def element?
      Element === self
    end

    # @return [Boolean]
    def text?
      Text === self
    end

    # @return [Boolean]
    def comment?
      Comment === self
    end

    # @return [Boolean]
    def cdata?
      CDATASection === self
    end

    # @return [Boolean]
    def attribute?
      Attr === self
    end

    # @return [Boolean]
    def document?
      Document === self
    end

    # @return [Boolean]
    def processing_instruction?
      ProcessingInstruction === self
    end

    # @return [Boolean]
    def document_fragment?
      DocumentFragment === self
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
    #
    # The nodes are listed once, before the first is yielded (one NodeSet,
    # rather than a `children` set per level - about five times faster on a
    # large tree), so a block that adds or removes nodes does not change which
    # nodes are visited.
    # @return [self]
    def traverse(&block)
      _traverse_order.each(&block)
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
    # Strings that look like a location path (start with "/", "./", "..", "("
    # or "@", are "." alone, or contain "::") are treated as XPath; everything
    # else as CSS. No CSS selector starts with "." followed by "." or "/", or
    # is "." alone.
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
      s == "." || s.start_with?("/", "./", "..", "(", "@") || s.include?("::")
    end
  end
end
