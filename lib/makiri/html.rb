# frozen_string_literal: true

module Makiri
  # HTML-specific node leaves (§12). Every concrete HTML node is a Makiri::HTML::*
  # class under the matching abstract base (so is_a?(Makiri::Element) etc. holds),
  # carrying the lxb_dom-backed reader/query methods via the included
  # Makiri::HTML::Node module. XML nodes never inherit these. The classes
  # themselves are defined in C (ext/makiri/glue/ruby_html*.c); the per-class Ruby
  # additions live in this namespace's files (html/node_methods.rb, html/document.rb).
  module HTML
  end
end
