# frozen_string_literal: true

module Makiri
  # Per-query XPath evaluation context. Holds the context node, namespace
  # bindings, and variable bindings, and evaluates expressions against them.
  #
  #   ctx = Makiri::XPathContext.new(doc)
  #   ctx.register_namespace("svg", "http://www.w3.org/2000/svg")
  #   ctx.evaluate("//svg:circle")            # => Makiri::NodeSet
  #
  # +evaluate+ returns a NodeSet for node-set expressions, and a String,
  # Float, or boolean for the corresponding scalar XPath types.
  #
  # The bulk of the implementation lives in the extension (see
  # ext/makiri/rust/src/glue/xpath_context.rs and ext/makiri/rust/src/xpath/).
  class XPathContext
    # +#evaluate+ is defined in the extension and runs under the GVL (XPath
    # never releases it), so it cannot corrupt memory under concurrency: the GVL
    # serialises all of +evaluate+ / +register_*+ / +node=+ and any tree
    # mutation of the queried document. No Ruby runs during an evaluate - there
    # are no custom-function handlers; an unknown function is an
    # XPath::SyntaxError - so nothing re-enters the context mid-walk.

    # Nokogiri-compatible name for {#register_namespace}.
    alias register_ns register_namespace

    # Register several prefix => URI namespace bindings at once.
    # @param bindings [Hash{String => String}]
    # @return [self]
    def register_namespaces(bindings)
      bindings.each { |prefix, uri| register_namespace(prefix.to_s, uri.to_s) }
      self
    end

    # Register several name => value variable bindings at once.
    # @param bindings [Hash{String => Object}]
    # @return [self]
    def register_variables(bindings)
      bindings.each { |name, value| register_variable(name.to_s, value) }
      self
    end
  end
end
