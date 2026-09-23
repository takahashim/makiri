# frozen_string_literal: true

module Makiri
  # An ordered collection of nodes, returned by xpath/css queries.
  class NodeSet
    include Enumerable

    # #clone is a new set over the same nodes (the native #dup), honouring
    # +freeze:+.
    include CloneViaDup

    # @return [Integer]
    def size
      length
    end

    # @return [Boolean]
    def empty?
      length.zero?
    end

    # @return [Makiri::Node, nil]
    def first
      self[0]
    end

    # @return [Makiri::Node, nil]
    def last
      self[length - 1]
    end

    # Index access; alias for #[].
    # @return [Makiri::Node, nil]
    def at(index)
      self[index]
    end

    # Concatenated outer HTML of every node in the set.
    # @return [String]
    def to_html
      map(&:to_html).join
    end
    alias to_s to_html

    # Concatenated text content of every node in the set.
    # @return [String]
    def text
      map(&:text).join
    end
    alias inner_text text

    # Run a CSS selector against every node and return the unioned matches.
    # Further arguments (an XML namespace map) reach each node's {Node#css}.
    # @return [Makiri::NodeSet]
    def css(selector, *args, **opts)
      union_query(:css, selector, *args, **opts)
    end

    # Run an XPath expression against every node and union the node-set results.
    # An expression that evaluates to a string, number or boolean has no union,
    # so it raises ArgumentError (as Nokogiri does) rather than picking one
    # node's value. Further arguments (namespaces, a handler,
    # +namespace_matching:+) reach each node's {Node#xpath}.
    # @return [Makiri::NodeSet]
    def xpath(expr, *args, **opts)
      union_query(:xpath, expr, *args, **opts)
    end

    # First node matching the CSS selector across the set, or nil. The union is
    # in encounter order, so this is the first node's first match - found
    # without querying the rest of the set.
    # @return [Makiri::Node, nil]
    def at_css(selector, *args, **opts)
      first_hit(:at_css, selector, *args, **opts)
    end

    # First node matching the XPath expression across the set, or nil; a
    # scalar-valued expression raises, as with {#xpath}.
    # @return [Makiri::Node, nil]
    def at_xpath(expr, *args, **opts)
      first_hit(:at_xpath, expr, *args, **opts)
    end

    # CSS- or XPath-detecting query against every node (see {Node#search}).
    # @return [Makiri::NodeSet]
    def search(path)
      union_query(:search, path)
    end

    # Remove the named attribute from every node in the set.
    # @return [self]
    def remove_attr(name)
      each { |node| node.delete(name) }
      self
    end
    alias remove_attribute remove_attr

    # Detach every node in the set from its tree.
    # @return [self]
    def remove
      to_a.each(&:remove)
      self
    end
    alias unlink remove

    def inspect
      "#<#{self.class.name} length=#{length}>"
    end

    private

    # Run +method+(+query+, ...) on every node in the set and union the
    # per-node results. An empty set returns self unchanged, so it stays a
    # NodeSet (the shared shape behind #css / #xpath / #search).
    def union_query(method, query, *args, **opts)
      return self if empty?

      map do |node|
        result = node.public_send(method, query, *args, **opts)
        raise scalar_result(query, result) unless result.is_a?(NodeSet)

        result
      end.reduce(:|)
    end

    # The first node's +method+(+query+, ...) hit (an +at_*+ query), or nil.
    def first_hit(method, query, *args, **opts)
      each do |node|
        hit = node.public_send(method, query, *args, **opts)
        next if hit.nil?
        raise scalar_result(query, hit) unless hit.is_a?(Node)

        return hit
      end
      nil
    end

    # A per-node query over a set can only combine node-sets.
    def scalar_result(expr, value)
      ArgumentError.new("#{expr.inspect} evaluates to a #{value.class}, not a node-set")
    end
  end
end
