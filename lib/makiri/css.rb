# frozen_string_literal: true

module Makiri
  # CSS selector support. Queries are served by Lexbor's selector engine via
  # {Makiri::Node#css} / {Makiri::Node#at_css}; this module only defines the
  # error type. See ext/makiri/glue/ruby_css.c.
  module CSS
    # Raised when a CSS selector string fails to parse.
    class SyntaxError < ::Makiri::Error; end
  end
end
