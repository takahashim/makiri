# frozen_string_literal: true

require_relative "makiri/version"

# Native extension (a Rust crate). Located at lib/makiri/<ruby_abi>/makiri.{so,bundle}
# (created by rake-compiler). Loading is gated so the gem can be required
# in environments where the binary is not yet built (the require error
# is then surfaced clearly).
begin
  RUBY_VERSION =~ /(\d+\.\d+)/
  require_relative "makiri/#{Regexp.last_match(1)}/makiri"
rescue LoadError
  require_relative "makiri/makiri"
end

# The fallback above loads lib/makiri/makiri.{so,bundle} whichever version of
# this gem's Ruby code is asking: a checkout (or a Bundler `path:` gem) keeps
# one binary from its last compile, and one from another gem version lacks or
# misnames methods. The extension states what it was built as, and a
# mismatch stops here. (A binary for another Ruby is refused earlier, by the
# extension itself before it defines anything.) A binary older than the
# constants defines neither, which is a mismatch too.
module Makiri
  native_ruby = RUBY_VERSION[/\A\d+\.\d+/]
  native_version = const_defined?(:NATIVE_VERSION, false) ? NATIVE_VERSION : "(unknown)"
  native_api = const_defined?(:NATIVE_RUBY_API_VERSION, false) ? NATIVE_RUBY_API_VERSION : "(unknown)"
  if native_version != VERSION || native_api != native_ruby
    raise LoadError,
          "makiri's native extension was built as makiri #{native_version} for Ruby " \
          "#{native_api}, but was loaded by makiri #{VERSION} on Ruby " \
          "#{native_ruby}; rebuild it (`bundle exec rake clean compile` in a checkout, " \
          "or reinstall the gem)"
  end
end

require_relative "makiri/error"
require_relative "makiri/clone_via_dup"
require_relative "makiri/xpath_syntax"
require_relative "makiri/node_path"
require_relative "makiri/node"
require_relative "makiri/reader_aliases"
require_relative "makiri/document"
require_relative "makiri/html"
require_relative "makiri/html/node_methods"
require_relative "makiri/html/document"
require_relative "makiri/xml"
require_relative "makiri/xml/namespace"
require_relative "makiri/xml/node_methods"
require_relative "makiri/xml/document"
require_relative "makiri/xml/builder"
require_relative "makiri/element"
require_relative "makiri/attr"
require_relative "makiri/text"
require_relative "makiri/comment"
require_relative "makiri/cdata_section"
require_relative "makiri/processing_instruction"
require_relative "makiri/document_type"
require_relative "makiri/document_fragment"
require_relative "makiri/node_set"
require_relative "makiri/xpath_context"
require_relative "makiri/xpath"
require_relative "makiri/css"
require_relative "makiri/compat_aliases"

# The error classes (Makiri::Error < StandardError and everything under it,
# plus Makiri::InternalError < Exception) are defined by the extension, in
# init.rs, so their hierarchy has a single source.
module Makiri
  # Convenience constructor mirroring Nokogiri.
  #
  # @param source [String] HTML source (UTF-8).
  # @param opts [Hash] +max_tree_depth:+ - see {Makiri::HTML::Document.parse}
  # @return [Makiri::HTML::Document]
  def self.HTML(source, **opts) # rubocop:disable Naming/MethodName
    HTML::Document.parse(source, **opts)
  end

  # Alias for {.HTML}.
  def self.parse(source, **opts)
    HTML::Document.parse(source, **opts)
  end

  # Convenience XML constructor mirroring Nokogiri::XML(source). A method named
  # XML on the Makiri module, coexisting with the Makiri::XML constant (the
  # module), as Nokogiri::XML does. Delegates to {Makiri::XML::Document.parse},
  # exactly as {.HTML} delegates to {Makiri::HTML::Document.parse}.
  #
  # @param source [String, #read] XML source (its String encoding is honoured).
  # @return [Makiri::XML::Document]
  def self.XML(source, **opts) # rubocop:disable Naming/MethodName
    XML::Document.parse(source, **opts)
  end
end
