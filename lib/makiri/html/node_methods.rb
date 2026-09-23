# frozen_string_literal: true

module Makiri
  module HTML
    # The lxb_dom reader/query methods are defined natively on this module and
    # included into every HTML leaf (including the generic Makiri::HTML::Node).
    module NodeMethods
      ReaderAliases.define_on(self)

    end
  end
end
