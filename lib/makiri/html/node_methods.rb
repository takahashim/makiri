# frozen_string_literal: true

module Makiri
  module HTML
    # The lxb_dom reader/query methods are defined natively on this module and
    # included into every HTML leaf (including the generic Makiri::HTML::Node).
    module NodeMethods
      ReaderAliases.define_on(self)

      private

      # The namespace an unprefixed XPath element name test selects here: the
      # HTML namespace, as in browsers' document.evaluate (see {NodePath#path}).
      def unprefixed_element_namespace
        "http://www.w3.org/1999/xhtml"
      end
    end
  end
end
