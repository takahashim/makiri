# frozen_string_literal: true

module Makiri
  module HTML
    # Root container for a parsed HTML document. Construction, serialization and
    # the HTML-only conveniences (body/head/title/encoding) live here, not on the
    # abstract Makiri::Document.
    class Document
      # Parse +source+ as HTML5 and return a Makiri::HTML::Document.
      #
      # +source+ may be a String or any object responding to +#read+ (e.g. an
      # IO). The native parser (#_parse) expects UTF-8 bytes. Source locations
      # for {Node#line} are always tracked (the cost is negligible).
      #
      # +max_tree_depth+ bounds how deeply elements may nest, as in
      # Nokogiri::HTML5: the depth counts elements from the root, +<html>+
      # being 1, and a document deeper than the limit raises Makiri::Error
      # ("document tree depth limit exceeded (400)"). The default is 400; a
      # negative Integer disables the limit.
      #
      # @param source [String, #read]
      # @param max_tree_depth [Integer, nil] nil for the default
      # @return [Makiri::HTML::Document]
      def self.parse(source, max_tree_depth: nil)
        source = source.read if source.respond_to?(:read)
        _parse(String(source), max_tree_depth)
      end

      # An independent copy of the whole document (like Nokogiri's Document#dup):
      # an empty document in the same quirks mode, given a deep copy of each of
      # this one's children (+import_node+, which carries a <template>'s
      # contents too), so the copy shares no nodes with the original and has
      # the same tree - whatever shape it was built into, which a re-parse of
      # +to_html+ would not keep (a Document.new without an <html> root gained
      # the html/head/body shell and quirks mode). Node#dup's clone_node
      # delegation is wrong for a document node, hence this override. Any
      # level argument is ignored. #clone is this too (see {CloneViaDup}).
      #
      # Node#line is nil on the copy: its nodes were not parsed from a source.
      def dup(*)
        _empty_copy.tap do |copy|
          children.each { |child| copy.add_child(copy.import_node(child, true)) }
        end
      end

      # Whether the document is in quirks mode - the mode a missing or legacy
      # doctype puts it in. Limited-quirks mode is not quirks mode here, as the
      # DOM's +compatMode+ has it. (#quirks_mode is the raw value: 0 no-quirks,
      # 1 quirks, 2 limited-quirks.)
      # @return [Boolean]
      def quirks_mode?
        quirks_mode == 1
      end

      # The DOM's +document.compatMode+: "BackCompat" in quirks mode, else
      # "CSS1Compat".
      # @return [String]
      def compat_mode
        quirks_mode? ? "BackCompat" : "CSS1Compat"
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

      # Set the document title, creating <title> (in <head>) if absent. A
      # document with no <head> - a Document.new one before it has one - is
      # left as it is, as the DOM's title setter leaves it.
      # @param text [String]
      # @return [String]
      def title=(text)
        ensure_in_head("title", "title")&.content = text
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

      # Set (or insert, in <head>) a <meta charset> declaration. Like #title=,
      # it leaves a document with no <head> as it is.
      # @param value [String]
      # @return [String]
      def meta_encoding=(value)
        ensure_in_head("meta[charset]", "meta")&.[]=("charset", value)
        value
      end

      private

      # The first node matching +css_query+, or a freshly created <+tag+>
      # appended to <head>; nil when there is neither a match nor a head - never
      # an element put straight under the root, which the DOM's title setter
      # does not do either. Shared by #title= and #meta_encoding=, which then
      # set content / attributes on it.
      def ensure_in_head(css_query, tag)
        found = at_css(css_query)
        return found if found

        parent = head
        parent && Element.new(tag, self).tap { |el| parent.add_child(el) }
      end
    end
  end
end
