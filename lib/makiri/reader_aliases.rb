# frozen_string_literal: true

module Makiri
  # The Nokogiri-compatible aliases over the per-representation readers
  # (#[], #name, #key?, #node_type), stated once for both behaviour modules.
  #
  # They cannot live on Makiri::Node: alias_method resolves its target where it
  # is called, and the readers are defined on Makiri::HTML::NodeMethods and
  # Makiri::XML::NodeMethods, which the leaves include AHEAD of Node. So each
  # module applies them to itself, once its own readers exist - an alias, not a
  # delegating method, because a native reader aliased costs nothing per call.
  module ReaderAliases
    ALIASES = {
      attr: :[],
      get_attribute: :[],
      has_attribute?: :key?,
      node_name: :name,
      "node_name=": :name=,
      type: :node_type
    }.freeze

    # @param mod [Module] a NodeMethods module that defines every target
    def self.define_on(mod)
      ALIASES.each { |alias_name, target| mod.alias_method(alias_name, target) }
    end
  end
  # Load-time plumbing, not API: reached by name only from inside Makiri.
  private_constant :ReaderAliases
end
