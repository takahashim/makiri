# frozen_string_literal: true

module Makiri
  module XML
    # A Nokogiri-compatible DSL for building an XML document (or subtree) from
    # scratch. It is a thin, pure-Ruby layer over the public construction surface
    # (+XML::Document.new+, +Document#create_element+ / +#create_text_node+ /
    # +#create_cdata+ / +#create_comment+, and +Node#add_child+); no C code is
    # involved.
    #
    # @example Block-with-argument form (recommended)
    #   builder = Makiri::XML::Builder.new do |xml|
    #     xml.feed("xmlns" => "urn:a") do
    #       xml.entry do
    #         xml.title("Hello")
    #       end
    #     end
    #   end
    #   builder.to_xml
    #
    # @example instance_eval form (no block argument)
    #   builder = Makiri::XML::Builder.new do
    #     root { child("text") }
    #   end
    #
    # An element is created by calling a method named after the tag. Trailing
    # arguments follow the Nokogiri convention: a Hash sets attributes (including
    # +xmlns+ / +xmlns:prefix+ namespace declarations), any other argument becomes
    # the element's text content, and a block builds nested children.
    #
    # Tag names that collide with a Ruby/Kernel method (or with one of this
    # builder's own helpers below - +text+, +cdata+, +comment+, +doc+, +parent+,
    # +to_xml+, +to_s+) must be written with a trailing underscore, which is
    # stripped: +xml.id_("9")+ produces +<id>9</id>+. This matches Nokogiri.
    #
    # A namespace prefix is selected for the next element with +[]+:
    # +xml["dc"].title+ produces +<dc:title>+ (the prefix must be in scope, i.e.
    # declared via an +"xmlns:dc"+ attribute on an ancestor or on the element
    # itself, exactly as Makiri resolves prefixes at insertion time).
    class Builder
      # The document being built (a {Makiri::XML::Document}).
      attr_reader :doc

      # The node new children are currently appended to. While a nested block is
      # running this is that block's element; otherwise it is {#doc}. Writable so
      # the {NodeBuilder} attribute-shortcut chain can re-root the builder while
      # its own block runs.
      attr_accessor :parent

      # The block form (instance_eval vs yield) chosen for this build, memoised
      # from the first block seen. Read by {NodeBuilder} for nested blocks.
      attr_reader :arity

      # @param options [Hash] accepted for Nokogiri compatibility and ignored - a
      #   Makiri document has no configurable options (it is always UTF-8).
      # @param root [Makiri::XML::Node, nil] when given, build into this node:
      #   top-level calls append to it and its document is used. (This is what
      #   {.with} passes; mirrors +Nokogiri::XML::Builder.new(options, root)+.)
      # @yield [self] when the block takes an argument; otherwise the block is
      #   +instance_eval+'d against the builder.
      def initialize(options = {}, root = nil, &block)
        if root
          @doc = root.document
          @parent = root
        else
          @parent = @doc = Makiri::XML::Document.new
        end
        @ns_prefix = nil
        @arity = nil
        return unless block

        run(&block)
        @parent = @doc # like Nokogiri: after a build block, settle back at the document
      end

      # Build into an existing node: top-level calls append to +node+, using
      # +node+'s document. Mirrors +Nokogiri::XML::Builder.with+.
      #
      # @param node [Makiri::XML::Node]
      # @return [Builder]
      def self.with(node, &block)
        new({}, node, &block)
      end

      # Append a text node to the current parent.
      # @return [NodeBuilder]
      def text(string)
        insert(@doc.create_text_node(string.to_s))
      end

      # Append a CDATA section to the current parent.
      # @return [NodeBuilder]
      def cdata(string)
        insert(@doc.create_cdata(string.to_s))
      end

      # Append a comment node to the current parent.
      # @return [NodeBuilder]
      def comment(string)
        insert(@doc.create_comment(string.to_s))
      end

      # Parse +string+ as an XML fragment (against the document's in-scope
      # namespaces) and append its children to the current parent. The Builder
      # analogue of +Nokogiri::XML::Builder#<<+.
      # @return [self]
      def <<(string)
        @doc.fragment(string).children.to_a.each { |child| insert(child) }
        self
      end

      # Select the namespace prefix for the next element (consumed by the next
      # tag method). Returns self so it reads as +xml["dc"].title+.
      # @return [self]
      def [](ns_prefix)
        @ns_prefix = ns_prefix.to_s
        self
      end

      # Serialize the built document. Forwards to {Document#to_xml} (so +pretty:+
      # works).
      def to_xml(...)
        @doc.to_xml(...)
      end
      alias_method :to_s, :to_xml

      # Any other method name is a tag: create the element and insert it.
      def method_missing(name, *args, &block)
        tag = name.to_s.sub(/[_!]\z/, "")
        prefix = @ns_prefix
        if prefix
          tag = "#{prefix}:#{tag}"
          @ns_prefix = nil
        end
        node = create_element(tag, args)
        check_prefix_defined!(node, prefix) if prefix
        insert(node, &block)
      end

      # Tag methods are open-ended, so report respond_to? truthfully for them
      # (anything that is not already a real method is a candidate tag).
      def respond_to_missing?(_name, _include_private = false)
        true
      end

      private

      # Translate the Nokogiri-style trailing arguments (a Hash is attributes,
      # anything else is text content) into a {Document#create_element} call.
      def create_element(tag, args)
        text = nil
        attributes = nil
        args.each do |arg|
          if arg.is_a?(Hash)
            attributes = attributes ? attributes.merge(arg) : arg
          else
            text = arg
          end
        end
        cargs = []
        cargs << text.to_s unless text.nil?
        cargs << attributes unless attributes.nil?
        @doc.create_element(tag, *cargs)
      end

      # Raise like Nokogiri when a prefix selected via +[]+ resolves nowhere: not
      # in scope at the insertion point (+@parent+ and its ancestors) and not
      # self-declared on +node+ itself (e.g. +xml["foo"].root("xmlns:foo" => ...)+).
      def check_prefix_defined!(node, prefix)
        return if @parent.respond_to?(:namespaces) && @parent.namespaces.key?("xmlns:#{prefix}")
        return if node.namespace_definitions.any? { |ns| ns.prefix == prefix }

        raise ArgumentError, "Namespace #{prefix} has not been defined"
      end

      # Append +node+ to the current parent, then, if a block is given, descend
      # into the inserted node for the duration of that block. Returns a
      # {NodeBuilder} over the inserted node (which may be an imported copy, e.g.
      # from a fragment), so the Nokogiri attribute-shortcut chain works.
      def insert(node, &block)
        node = @parent.add_child(node)
        if block
          previous = @parent
          @parent = node
          begin
            run(&block)
          ensure
            @parent = previous
          end
        end
        NodeBuilder.new(node, self)
      end

      # Run a DSL block, choosing instance_eval vs yield once (from the first
      # block seen), the same way Nokogiri does, so the form is consistent
      # throughout a build.
      def run(&block)
        @arity = block.arity if @arity.nil?
        if @arity <= 0
          instance_eval(&block)
        else
          block.call(self)
        end
      end

      # The value returned by each element call, wrapping the just-inserted node
      # so attributes can be added with the Nokogiri terse-chain syntax:
      #
      #   xml.object.classy.thing!   # => <object class="classy" id="thing"/>
      #
      # A plain method name appends to the +class+ attribute, +name!+ sets +id+
      # (and content if given), +name=+ sets that attribute, a trailing Hash adds
      # each key as an attribute, and a block descends into the node. +[]+ / +[]=+
      # read and write attributes directly. Semantics mirror
      # +Nokogiri::XML::Builder::NodeBuilder+.
      class NodeBuilder
        def initialize(node, doc_builder)
          @node = node
          @doc_builder = doc_builder
        end

        # Read an attribute of the wrapped node.
        def [](key)
          @node[key]
        end

        # Write an attribute of the wrapped node.
        def []=(key, value)
          @node[key] = value
        end

        def method_missing(method, *args, &block)
          opts = args.last.is_a?(Hash) ? args.pop : {}
          case method.to_s
          when /\A(.*)!\z/
            @node["id"] = Regexp.last_match(1)
            @node.content = args.first if args.first
          when /\A(.*)=\z/
            @node[Regexp.last_match(1)] = args.first
          else
            @node["class"] = ((@node["class"] || "").split(/\s/) + [method.to_s]).join(" ")
            @node.content = args.first if args.first
          end

          opts.each do |key, value|
            @node[key.to_s] = ((@node[key.to_s] || "").split(/\s/) + [value]).join(" ")
          end

          if block
            old_parent = @doc_builder.parent
            @doc_builder.parent = @node
            arity = @doc_builder.arity || block.arity
            value = arity <= 0 ? @doc_builder.instance_eval(&block) : block.call(@doc_builder)
            @doc_builder.parent = old_parent
            return value
          end

          self
        end

        def respond_to_missing?(_name, _include_private = false)
          true
        end
      end
    end
  end
end
