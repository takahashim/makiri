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
  # The bulk of the implementation lives in C (see
  # ext/makiri/glue/ruby_xpath.c and ext/makiri/xpath/).
  class XPathContext
    # +#evaluate+ is defined in C and runs under the GVL (XPath never releases
    # it), so it cannot corrupt memory under concurrency. Two distinct hazards,
    # handled differently:
    #
    # * Cross-thread: the GVL serialises all of +evaluate+ / +register_*+ /
    #   +node=+ and any tree mutation of the queried document, so threads can
    #   never run two of them at once.
    # * Same-thread re-entrancy: a custom function handler runs mid-evaluate and
    #   could call back into this same context. A re-entrant +register_namespace+
    #   / +register_variable+ / +node=+ is refused (raises) while an evaluate is
    #   in progress, since it would free/swap state the suspended evaluator still
    #   borrows; a nested +evaluate+ is allowed.

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
