# frozen_string_literal: true

module Makiri
  module HTML
    # The lxb_dom reader/query methods are defined in C on this module and
    # included into every HTML leaf (including the generic Makiri::HTML::Node).
    # The Nokogiri-compatible aliases over those readers live here (not on
    # Makiri::Node) so they resolve against the HTML readers at definition time.
    module NodeMethods
      alias_method :attr, :[]
      alias_method :get_attribute, :[]
      alias_method :has_attribute?, :key?
      alias_method :remove_attribute, :delete
      alias_method :node_name, :name
      alias_method :node_name=, :name=
      alias_method :type, :node_type
    end
  end
end
