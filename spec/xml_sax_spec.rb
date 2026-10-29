# frozen_string_literal: true

require "spec_helper"
require "stringio"
require "tempfile"
require "timeout"

# Makiri::XML::SAX: the handler's Ruby runs in the middle of a parse, which
# nothing else in the extension does. These hold the shape of the events
# (Nokogiri's) and every way the handler can leave the parse - raise, throw,
# an interrupt, a Fiber switch, a nested parse - to an ordinary outcome
# (notes/xml_sax_plan.ja.md, section 4).
RSpec.describe Makiri::XML::SAX do
  let(:recorder) do
    Class.new(Makiri::XML::SAX::Document) do
      attr_reader :events

      def initialize
        super
        @events = []
      end

      def xmldecl(*args) = @events << [:xmldecl, *args]
      def start_document = @events << [:start_document]
      def end_document = @events << [:end_document]

      def start_element_namespace(name, attrs = [], prefix = nil, uri = nil, ns = [])
        @events << [:start, name, attrs.map(&:to_a), prefix, uri, ns]
      end

      def end_element_namespace(name, prefix = nil, uri = nil) = @events << [:end, name, prefix, uri]
      def characters(string) = @events << [:characters, string]
      def cdata_block(string) = @events << [:cdata, string]
      def comment(string) = @events << [:comment, string]
      def processing_instruction(name, content) = @events << [:pi, name, content]
      def error(string) = @events << [:error, string]
    end
  end

  def events_of(xml)
    handler = recorder.new
    Makiri::XML::SAX::Parser.new(handler).parse(xml)
    handler.events
  end

  describe "the events" do
    it "reports each node as Nokogiri's SAX does, namespaces resolved" do
      xml = %(<?xml version="1.0" encoding="UTF-8"?><r xmlns:f="u:f" a="1"><f:x f:b="2">t&amp;x</f:x>) +
            %(<![CDATA[c]]><!--c--><?pi d?></r>)
      expect(events_of(xml)).to eq([
        [:xmldecl, "1.0", "UTF-8", nil],
        [:start_document],
        [:start, "r", [["a", nil, nil, "1"]], nil, nil, [["f", "u:f"]]],
        [:start, "x", [["b", "f", "u:f", "2"]], "f", "u:f", []],
        [:characters, "t&x"],
        [:end, "x", "f", "u:f"],
        [:cdata, "c"],
        [:comment, "c"],
        [:pi, "pi", "d"],
        [:end, "r", nil, nil],
        [:end_document]
      ])
    end

    it "hands the namespaced events on to start_element / end_element, as Nokogiri's Document does" do
      handler = Class.new(Makiri::XML::SAX::Document) do
        attr_reader :seen

        def start_element(name, attrs = []) = (@seen ||= []) << [:start, name, attrs]
        def end_element(name) = (@seen ||= []) << [:end, name]
      end.new
      Makiri::XML::SAX::Parser.new(handler).parse(
        %(<root xmlns:foo='http://foo.example.com/' xmlns='http://example.com/'>) +
        %(<foo:bar foo:quux="xxx">hello</foo:bar></root>)
      )
      expect(handler.seen).to eq([
        [:start, "root", [["xmlns:foo", "http://foo.example.com/"], ["xmlns", "http://example.com/"]]],
        [:start, "foo:bar", [["foo:quux", "xxx"]]],
        [:end, "foo:bar"],
        [:end, "root"]
      ])
    end

    it "gives each value as a fresh UTF-8 String" do
      strings = events_of("<r a='é'>ü</r>").flatten.grep(String)
      expect(strings.map(&:encoding).uniq).to eq([Encoding::UTF_8])
      expect(strings.none?(&:frozen?)).to be(true)
    end

    it "reads an IO, a file and an encoding given to the parser" do
      handler = recorder.new
      Makiri::XML::SAX::Parser.new(handler).parse(StringIO.new("<r>io</r>"))
      expect(handler.events).to include([:characters, "io"])

      Tempfile.create(["sax", ".xml"]) do |f|
        f.write("<r>file</r>")
        f.flush
        handler = recorder.new
        Makiri::XML::SAX::Parser.new(handler).parse_file(f.path)
        expect(handler.events).to include([:characters, "file"])
      end

      handler = recorder.new
      Makiri::XML::SAX::Parser.new(handler, "ISO-8859-1").parse("<r>\xE9</r>".b)
      expect(handler.events).to include([:characters, "é"])
    end

    it "refuses a block, having no ParserContext" do
      expect { Makiri::XML::SAX::Parser.new.parse("<r/>") { |_ctx| nil } }
        .to raise_error(NotImplementedError)
    end
  end

  describe "a refused input" do
    it "is reported to #error, then #end_document, and the parse returns" do
      expect(events_of("<r><a></b></r>")).to eq([
        [:start_document],
        [:start, "r", [], nil, nil, []],
        [:start, "a", [], nil, nil, []],
        [:error, "malformed XML"],
        [:end_document]
      ])
    end

    it "is refused as Makiri::XML refuses it, a DTD construct included" do
      ["<r>", "<r/><r/>", "<p:r/>", "<r a='1' a='2'/>",
       "<!DOCTYPE r [<!ENTITY e 'x'>]><r>&e;</r>"].each do |xml|
        expect { Makiri::XML(xml) }.to raise_error(Makiri::XML::SyntaxError)
        expect(events_of(xml).map(&:first)).to include(:error), xml
      end
    end
  end

  describe "leaving the parse from the handler" do
    let(:stopper) do
      Class.new(Makiri::XML::SAX::Document) do
        def initialize(how) = (super(); @how = how)

        def start_element(name, _attrs = [])
          return unless name == "stop"

          case @how
          when :raise then raise ArgumentError, "stopped"
          when :throw then throw :sax_done, :thrown
          end
        end
      end
    end

    it "lets a raise out as it is, and the parser is reusable" do
      parser = Makiri::XML::SAX::Parser.new(stopper.new(:raise))
      expect { parser.parse("<r><stop/></r>") }.to raise_error(ArgumentError, "stopped")
      parser.document = recorder.new
      parser.parse("<r/>")
      expect(parser.document.events.map(&:first)).to eq(%i[start_document start end end_document])
    end

    it "lets a throw reach its catch" do
      result = catch(:sax_done) do
        Makiri::XML::SAX::Parser.new(stopper.new(:throw)).parse("<r><stop/><after/></r>")
        :finished
      end
      expect(result).to eq(:thrown)
    end

    it "lets an interrupt from another thread (Timeout) stop it" do
      slow = Class.new(Makiri::XML::SAX::Document) { def start_element(*) = sleep(0.01) }
      xml = "<r>#{'<e/>' * 10_000}</r>"
      expect { Timeout.timeout(0.05) { Makiri::XML::SAX::Parser.new(slow.new).parse(xml) } }
        .to raise_error(Timeout::Error)
    end
  end

  describe "the handler's own Ruby" do
    it "may run another SAX parse inside a handler method" do
      inner = recorder.new
      outer = Class.new(Makiri::XML::SAX::Document) do
        def initialize(inner) = (super(); @inner = inner)
        def start_element(name, _attrs = []) = (Makiri::XML::SAX::Parser.new(@inner).parse("<in/>") if name == "r")
      end
      Makiri::XML::SAX::Parser.new(outer.new(inner)).parse("<r/>")
      expect(inner.events.map(&:first)).to eq(%i[start_document start end end_document])
    end

    it "may switch Fibers mid-parse and be resumed" do
      yielding = Class.new(Makiri::XML::SAX::Document) do
        attr_reader :names

        def start_element(name, _attrs = [])
          (@names ||= []) << name
          Fiber.yield name
        end
      end.new
      fiber = Fiber.new { Makiri::XML::SAX::Parser.new(yielding).parse("<a><b/><c/></a>") }
      seen = []
      seen << fiber.resume while fiber.alive?
      expect(yielding.names).to eq(%w[a b c])
      expect(seen.first(3)).to eq(%w[a b c])
    end

    it "survives a Fiber abandoned mid-parse, and a collection after it" do
      abandoning = Class.new(Makiri::XML::SAX::Document) { def start_element(*) = Fiber.yield }
      5.times do
        Fiber.new { Makiri::XML::SAX::Parser.new(abandoning.new).parse("<r><x/></r>") }.resume
      end
      GC.start
      expect(events_of("<r/>").map(&:first)).to eq(%i[start_document start end end_document])
    end

    it "keeps every value alive under GC.stress" do
      xml = "<r xmlns:p='u:p'>#{(1..40).map { |i| "<p:e p:a='#{i}'>t#{i}<!--c--></p:e>" }.join}</r>"
      want = events_of(xml)
      GC.stress = true
      begin
        got = events_of(xml)
      ensure
        GC.stress = false
      end
      expect(got).to eq(want)
    end
  end
end
