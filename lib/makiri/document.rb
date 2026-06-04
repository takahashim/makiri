# frozen_string_literal: true

module Makiri
  # Root container for a parsed HTML document.
  class Document < Node
    # Parse +source+ as HTML5 and return a Document.
    #
    # +source+ may be a String or any object responding to +#read+ (e.g. an
    # IO). The native parser (#_parse) expects UTF-8 bytes. Source locations
    # for {Node#line} are always tracked (the cost is negligible).
    #
    # @param source [String, #read]
    # @return [Makiri::Document]
    def self.parse(source)
      source = source.read if source.respond_to?(:read)
      _parse(String(source))
    end

    # An independent copy of the whole document (like Nokogiri's Document#dup).
    # Built by serialising and re-parsing, so the copy shares no nodes with the
    # original — Node#dup's clone_node delegation is wrong for a document node,
    # hence this override. (A DOM mutated into a shape the HTML parser would not
    # itself produce, e.g. a foster-parented table cell, may be re-normalised on
    # re-parse; a freshly parsed document round-trips unchanged.) Any level /
    # freeze argument is ignored.
    def dup(*)
      Makiri.parse(to_html)
    end

    # Like {#dup}: an independent copy of the document, honouring Ruby's
    # +freeze:+ keyword (a frozen document's nodes raise +FrozenError+ on
    # mutation).
    def clone(freeze: nil)
      copy = Makiri.parse(to_html)
      copy.freeze if freeze || (freeze.nil? && frozen?)
      copy
    end

    # The document's <body> element, or nil.
    # @return [Makiri::Element, nil]
    def body
      at_css("body")
    end

    # The document's <head> element, or nil.
    # @return [Makiri::Element, nil]
    def head
      at_css("head")
    end

    # Set the document title, creating <title> (in <head>) if absent.
    # @param text [String]
    # @return [String]
    def title=(text)
      t = at_css("title")
      unless t
        t = Element.new("title", self)
        (head || root).add_child(t)
      end
      t.content = text
      text
    end

    # Makiri parses and stores everything as UTF-8 (callers decode bytes before
    # parsing), so the in-memory encoding is always UTF-8.
    # @return [String]
    def encoding
      "UTF-8"
    end

    # The charset declared in the document's markup, or nil. Reads
    # <meta charset> first, then <meta http-equiv="Content-Type">.
    # @return [String, nil]
    def meta_encoding
      if (m = at_css("meta[charset]"))
        return m["charset"]
      end

      css("meta").each do |meta|
        http_equiv = meta["http-equiv"]
        next unless http_equiv&.downcase == "content-type"

        content = meta["content"].to_s
        return Regexp.last_match(1) if content =~ /charset\s*=\s*"?([^\s;"]+)/i
      end
      nil
    end

    # Set (or insert) a <meta charset> declaration.
    # @param value [String]
    # @return [String]
    def meta_encoding=(value)
      meta = at_css("meta[charset]")
      unless meta
        meta = Element.new("meta", self)
        (head || root).add_child(meta)
      end
      meta["charset"] = value
      value
    end
  end
end
