# frozen_string_literal: true

module Makiri
  module XML
    # A namespace binding: what #namespace, #namespace_definitions and the
    # other namespace queries hand back. +prefix+ is nil for the default
    # namespace. A value object - equal when both fields are equal - and, like
    # Nokogiri's, printable as its URI.
    Namespace = Data.define(:prefix, :href) do
      alias_method :to_s, :href

      def inspect
        "#<Makiri::XML::Namespace prefix=#{prefix.inspect} href=#{href.inspect}>"
      end
    end
  end
end
