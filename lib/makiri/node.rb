# frozen_string_literal: true

module Makiri
  # Base class for every DOM node (element, attribute, text, comment, ...).
  # The bulk of the API lives in the C extension; this file defines the
  # Ruby-only conveniences.
  class Node
    # Order by document (pre-order) position via the native #<=>, so nodes can
    # be sorted; #<=> is nil (incomparable) across documents or for attributes.
    include Comparable

    # A Node is Enumerable over its child nodes (#each yields each child), so
    # node.map / node.select / node.find / node.to_a, etc. work — like Nokogiri.
    # (#to_h is unaffected: Node defines its own, returning the attribute hash.)
    include Enumerable

    # Identity is by wrapped node pointer; defined in C.

    # Yield each child node in document order. Iterates a snapshot of the
    # children (taken when called), so the block may safely move or remove the
    # current node. Returns an Enumerator when no block is given.
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

    # @return [Boolean] true for a blank/whitespace-only text or CDATA node.
    def blank?
      (text? || is_a?(CData)) && content.strip.empty?
    end

    # --- Nokogiri-compatible aliases over the core API ---
    #
    # Aliases of the representation-specific reader methods (#[], #name, ...) live
    # with those methods on the per-kind node behaviour (Makiri::HTML::Node), not
    # here, since alias_method resolves its target at definition time and the
    # readers are defined on the leaves' included module. These two alias
    # representation-independent predicates defined just above, so they stay.
    alias_method :elem?, :element?
    alias_method :fragment?, :document_fragment?

    # Set an attribute (alias for {#[]=}). @return [String]
    def set_attribute(name, value)
      self[name] = value
    end

    # The Attribute node named +name+, or nil (cf. {#[]}, which returns the value).
    # @return [Makiri::Attribute, nil]
    def attribute(name)
      attributes[name.to_s]
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

    # An independent copy of this node, detached from any parent and owned by the
    # same document (like Nokogiri's Node#dup). Deep by default; +level+ 0 makes
    # a shallow copy (matching Nokogiri's level argument). The native allocator
    # is undef'd to keep wrappers memory-safe, so #dup/#clone delegate to
    # {#clone_node} rather than Ruby's default allocate-and-copy (which would
    # otherwise raise "allocator undefined").
    def dup(level = 1)
      clone_node(level != 0)
    end

    # Like {#dup}, always a deep copy, and honouring Ruby's +freeze:+ keyword:
    # +true+ returns a frozen copy, +false+ an unfrozen one, the default (+nil+)
    # copies the receiver's frozen state. A frozen node is genuinely immutable —
    # its mutators raise +FrozenError+ (see Makiri's mutation methods).
    def clone(freeze: nil)
      copy = clone_node(true)
      copy.freeze if freeze || (freeze.nil? && frozen?)
      copy
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
