# frozen_string_literal: true

module Makiri
  # XPath error types, defined by the extension (init.rs) with every other
  # Makiri error class, so the hierarchy has one source:
  #
  # * Makiri::XPath::SyntaxError (< Makiri::Error) - an expression fails to
  #   parse.
  # * Makiri::XPath::LimitExceeded (< SyntaxError, for Nokogiri-shaped
  #   rescue) - an evaluation budget (operation count, recursion depth,
  #   node-set cap) is exhausted.
  module XPath
  end
end
