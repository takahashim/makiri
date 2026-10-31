# frozen_string_literal: true

module Makiri
  module XML
    # Ruby additions over the native XML node readers, so the XML node surface
    # matches the HTML one (Makiri::HTML::NodeMethods). #ancestors and #key?
    # are native on both, so they cannot drift apart.
    module NodeMethods
      # After #key?, which #has_attribute? aliases.
      ReaderAliases.define_on(self)

      # #css / #at_css / #matches? are native, as HTML's are, with the same
      # arguments: what they mean over XML is in glue/xml_node/css.rs.

      private

      # The namespace an unprefixed XPath element name test selects here: none,
      # per XPath 1.0 (see {NodePath#path}).
      def unprefixed_element_namespace
        nil
      end
    end
  end
end
