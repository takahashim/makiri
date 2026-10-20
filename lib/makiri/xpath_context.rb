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
    # +#evaluate+ is defined in the extension and runs under the GVL (XPath never releases
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

    # The Fiber an evaluation's handler calls run in, made once per
    # evaluation that calls one (bridge/xpath/handler.rs). The evaluator's
    # frames stay in the caller's Fiber: a handler that switches Fibers -
    # Fiber.yield, an Enumerator's yielder - suspends only its own frames
    # here, and the evaluation fails instead of being left suspended with the
    # document locked. Each op is [call?, handler, method, args]; each answer
    # is [this Fiber, result], so a value the handler yields itself is told
    # from an answer.
    def self.__handler_fiber # :nodoc:
      Fiber.new(blocking: true) do |op|
        me = Fiber.current
        loop do
          out = op[0] ? op[1].__send__(op[2], *op[3]) : op[1].respond_to?(op[2])
          op = Fiber.yield([me, out])
        end
      end
    end
    private_class_method :__handler_fiber

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
