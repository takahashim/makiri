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
      return self if empty?

      map { |node| node.css(selector) }.reduce(:|)
    end

    # Run an XPath expression against every node and union the node-set results.
    # @return [Makiri::NodeSet]
    def xpath(expr)
      return self if empty?

      map { |node| node.xpath(expr) }.reduce(:|)
    end

    # CSS- or XPath-detecting query against every node (see {Node#search}).
    # @return [Makiri::NodeSet]
    def search(path)
      return self if empty?

      map { |node| node.search(path) }.reduce(:|)
    end

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
  end
end
