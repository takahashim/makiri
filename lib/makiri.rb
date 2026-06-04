# frozen_string_literal: true

require_relative "makiri/version"

# Native C extension. Located at lib/makiri/<ruby_abi>/makiri.{so,bundle}
# (created by rake-compiler). Loading is gated so the gem can be required
# in environments where the binary is not yet built (the require error
# is then surfaced clearly).
begin
  RUBY_VERSION =~ /(\d+\.\d+)/
  require_relative "makiri/#{Regexp.last_match(1)}/makiri"
rescue LoadError
  require_relative "makiri/makiri"
end

require_relative "makiri/node"
require_relative "makiri/document"
require_relative "makiri/html"
require_relative "makiri/element"
require_relative "makiri/attribute"
require_relative "makiri/text"
require_relative "makiri/comment"
require_relative "makiri/cdata"
require_relative "makiri/processing_instruction"
require_relative "makiri/document_type"
require_relative "makiri/document_fragment"
require_relative "makiri/node_set"
require_relative "makiri/xpath_context"
require_relative "makiri/xpath"
require_relative "makiri/css"

module Makiri
  # Base exception class for Makiri-specific errors.
  class Error < StandardError; end

  # Convenience constructor mirroring Nokogiri.
  #
  # @param source [String] HTML source (UTF-8).
  # @return [Makiri::HTML::Document]
  def self.HTML(source) # rubocop:disable Naming/MethodName
    HTML::Document.parse(source)
  end

  # Alias for {.HTML}.
  def self.parse(source)
    HTML::Document.parse(source)
  end
end
