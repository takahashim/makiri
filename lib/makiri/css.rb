# frozen_string_literal: true

module Makiri
  # CSS selector support. Queries are served by Lexbor's selector engine via
  # {Makiri::Node#css} / {Makiri::Node#at_css}. See ext/makiri/rust/src/glue/css.rs.
  #
  # Makiri::CSS::SyntaxError (< Makiri::Error) is raised when a selector fails to
  # parse. It is defined by the extension (init.rs), with every other Makiri
  # error class, so the hierarchy has one source.
  module CSS
  end
end
