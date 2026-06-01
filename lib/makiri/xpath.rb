# frozen_string_literal: true

module Makiri
  module XPath
    # Raised when an XPath expression fails to parse, or when an
    # evaluation limit (operation count, recursion depth, node-set
    # cap) is exceeded.
    class SyntaxError < ::Makiri::Error; end

    # Raised when an evaluation budget is exhausted. Subclasses
    # SyntaxError for Nokogiri-shaped error compatibility.
    class LimitExceeded < SyntaxError; end
  end
end
