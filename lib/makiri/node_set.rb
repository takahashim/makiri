# frozen_string_literal: true

module Makiri
  # An ordered collection of nodes, returned by xpath/css queries.
  class NodeSet
    include Enumerable

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
    # @return [Makiri::NodeSet]
    def css(selector)
      union_query(:css, selector)
    end

    # Run an XPath expression against every node and union the node-set results.
    # @return [Makiri::NodeSet]
    def xpath(expr)
      union_query(:xpath, expr)
    end

    # First node matching the CSS selector across the set, or nil.
    # @return [Makiri::Node, nil]
    def at_css(selector)
      css(selector).first
    end

    # First node matching the XPath expression across the set (or the scalar
    # value for a non-node-set result).
    def at_xpath(expr)
      result = xpath(expr)
      result.is_a?(NodeSet) ? result.first : result
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

    # Like {#dup} (a new set over the same nodes), honouring Ruby's +freeze:+
    # keyword. (#dup is the native copy.)
    def clone(freeze: nil)
      copy = dup
      copy.freeze if freeze || (freeze.nil? && frozen?)
      copy
    end

    def inspect
      "#<#{self.class.name} length=#{length}>"
    end

    private

    # Run +method+(+arg+) on every node in the set and union the per-node
    # results. An empty set returns self unchanged, so it stays a NodeSet (the
    # shared shape behind #css / #xpath / #search).
    def union_query(method, arg)
      return self if empty?

      map { |node| node.public_send(method, arg) }.reduce(:|)
    end
  end
end
