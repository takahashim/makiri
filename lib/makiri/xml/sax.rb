# frozen_string_literal: true

module Makiri
  module XML
    # A SAX parser for XML: the handler's methods are called as the document is
    # read, with the shape Nokogiri::XML::SAX gives them, and no document is
    # built. The input is checked as Makiri::XML checks it - well-formedness,
    # namespaces, duplicate attributes, the DTD constructs Makiri refuses - so
    # the two accept the same documents.
    #
    #   class Titles < Makiri::XML::SAX::Document
    #     def start_element(name, attrs = [])
    #       @in_title = (name == "title")
    #     end
    #
    #     def characters(string)
    #       puts string if @in_title
    #     end
    #   end
    #
    #   Makiri::XML::SAX::Parser.new(Titles.new).parse(File.open("feed.xml", "rb"))
    #
    # Differences from Nokogiri are in NOKOGIRI_DIFFERENCES.md.
    module SAX
      # The events a parse reports. Subclass it and override the ones you want;
      # each default does nothing, but the namespaced element events, which the
      # parser calls, hand on to #start_element and #end_element with prefixed
      # names, as Nokogiri's do.
      class Document
        # The XML declaration: +version+, and +encoding+ and +standalone+ as
        # written, or nil.
        def xmldecl(version, encoding, standalone); end

        # Before anything else but #xmldecl.
        def start_document; end

        # Once the input is read - or refused, after #error.
        def end_document; end

        # An element's start: its name as written (prefixed) and an assoc
        # list of its attributes, namespace declarations included.
        def start_element(name, attrs = []); end

        # An element's end, its name as written.
        def end_element(name); end

        # An element's start: its local name, its attributes as
        # Parser::Attribute (localname, prefix, uri, value), its prefix and
        # namespace URI (nil when absent), and its namespace declarations as
        # [prefix, uri] pairs (prefix nil for the default namespace).
        def start_element_namespace(name, attrs = [], prefix = nil, uri = nil, ns = []) # rubocop:disable Metrics/ParameterLists
          name = [prefix, name].compact.join(":")
          attributes = ns.map { |ns_prefix, ns_uri| [["xmlns", ns_prefix].compact.join(":"), ns_uri] } +
                       attrs.map { |attr| [[attr.prefix, attr.localname].compact.join(":"), attr.value] }
          start_element(name, attributes)
        end

        # An element's end: its local name, prefix and namespace URI.
        def end_element_namespace(name, prefix = nil, uri = nil)
          end_element([prefix, name].compact.join(":"))
        end

        # The character data between two tags, its references expanded - one
        # call per run, where Nokogiri may split a run around a reference.
        def characters(string); end

        # A comment's content.
        def comment(string); end

        # Never called: Makiri's parser has no warnings. Kept for handlers
        # written for Nokogiri.
        def warning(string); end

        # The input was refused (malformed, over a limit, or a DTD construct
        # Makiri does not apply); #end_document follows, and the parse returns.
        def error(string); end

        # A CDATA section's content.
        def cdata_block(string); end

        # A processing instruction's target and content.
        def processing_instruction(name, content); end
      end

      # Reads a document and calls a SAX::Document's methods as it goes.
      #
      # The handler runs while the parse is under way: what it raises (or
      # throws) stops the parse and leaves #parse as it is. A refused input is
      # reported to the handler's #error instead, and #parse returns.
      class Parser
        # An attribute, as #start_element_namespace receives it.
        Attribute = Struct.new(:localname, :prefix, :uri, :value)

        # The handler the events go to.
        attr_accessor :document

        # The encoding to read the input as, or nil to take it from the
        # String, its BOM or its declaration (as Makiri::XML does).
        attr_accessor :encoding

        def initialize(document = Makiri::XML::SAX::Document.new, encoding = nil)
          @document = document
          @encoding = encoding
        end

        # Parse +input+, a String or anything answering #read.
        def parse(input, &block)
          if input.respond_to?(:read)
            parse_io(input, &block)
          else
            parse_memory(input, &block)
          end
        end

        # Parse an IO. It is read whole first: the parse itself streams, but
        # not its input yet.
        def parse_io(io, encoding = @encoding, &block)
          parse_memory(io.read, encoding, &block)
        end

        # Parse a String.
        def parse_memory(input, encoding = @encoding)
          raise NotImplementedError, "Makiri's SAX parser has no ParserContext to yield" if block_given?

          input = String(input)
          input = input.dup.force_encoding(encoding) if encoding
          SAX._parse(@document, input, Attribute)
        end

        # Parse the file at +filename+.
        def parse_file(filename, encoding = @encoding, &block)
          raise ArgumentError, "no filename provided" unless filename

          parse_memory(File.binread(filename), encoding, &block)
        end
      end
    end
  end
end
