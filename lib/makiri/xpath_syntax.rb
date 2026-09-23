# frozen_string_literal: true

module Makiri
  # Writing XPath 1.0 source text: the pure string half of {NodePath}, which
  # knows nothing about nodes.
  module XPathSyntax
    # The names written bare as a name test: a conservative, ASCII-only subset
    # of NCName. A name outside it may still be an NCName (non-ASCII letters),
    # but a bare name that is NOT one is a syntax error ("@click"), or a
    # prefix to resolve ("o:p", "xml:lang", "v-on:click") - so anything else is
    # written by expanded name, which is longer but never wrong.
    PLAIN_NAME = /\A[A-Za-z_][A-Za-z0-9_.-]*\z/

    module_function

    # @return [Boolean] whether +name+ can be written bare as a name test.
    def plain_name?(name)
      PLAIN_NAME.match?(name)
    end

    # +string+ as a string literal, or nil when it holds both quote
    # characters: an XPath 1.0 literal has no escape.
    # @return [String, nil]
    def literal(string)
      return "'#{string}'" unless string.include?("'")

      %("#{string}") unless string.include?('"')
    end

    # A name test by expanded name, which needs no prefix registered:
    # +axis+ "" gives "*[local-name()='a' and namespace-uri()='u']", "@" the
    # attribute form. nil when a part cannot be quoted.
    # @return [String, nil]
    def expanded_name_test(axis, local, uri)
      local_literal = literal(local)
      uri_literal = literal(uri)
      return unless local_literal && uri_literal

      "#{axis}*[local-name()=#{local_literal} and namespace-uri()=#{uri_literal}]"
    end
  end
  private_constant :XPathSyntax
end
