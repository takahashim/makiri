# frozen_string_literal: true

module Makiri
  # {#path}: an absolute XPath that finds the node again. Each node class says
  # how an XPath step selects its kind by overriding the private #path_step;
  # this module walks the ancestors and adds the positions.
  module NodePath
    # An absolute XPath that locates this node, e.g. "/html/body/p[2]". Each
    # step carries a 1-based position among the siblings its node test also
    # selects (omitted when unique). A name that an unprefixed name test would
    # not reach - a namespaced node, or one whose name is no plain NCName, such
    # as "o:p" or "@click" - is written by expanded name. Round-trips through
    # {Node#at_xpath}. A node XPath cannot reach - a doctype, a namespace
    # declaration, anything inside a DocumentFragment, or anything not attached
    # to its document - has no such path, and answers "?" rather than a path
    # that finds nothing or something else. (Nokogiri answers "?" for the
    # first three but "/div/p" for a detached subtree, which the document's own
    # /div/p would answer.)
    # @return [String]
    def path
      return "/" if document?

      segments = []
      node = self
      until node.document?
        segment = node.path_segment
        return "?" unless segment

        segments.unshift(segment)
        node = node.parent
        return "?" unless node # detached: no step from the document reaches it
      end
      "/#{segments.join("/")}"
    end

    protected

    # One "/"-separated step of {#path} for this node: its node test, plus a
    # position when a sibling passes the same test; nil when there is no test.
    # Comparing the tests themselves is what makes a CDATA section count among
    # the text() siblings, exactly as the XPath engine will when the path is
    # evaluated - and a doctype named "html", whose test is nil, not count
    # among the <html> elements.
    def path_segment
      test = path_node_test
      return unless test

      parent_node = parent
      return test unless parent_node # a detached top: #path answers "?" anyway

      siblings = parent_node.children.select { |c| c.path_node_test == test }
      return test if siblings.length <= 1

      "#{test}[#{siblings.index(self) + 1}]"
    end

    # The node test of this node's {#path} step, readable on a sibling. Why a
    # protected wrapper over a private hook rather than one overridden
    # protected method: Ruby checks a protected call against the class that
    # DEFINES the method, so a Text#path_node_test would refuse an Element
    # comparing itself with the text beside it. Override #path_step, never this.
    def path_node_test
      path_step
    end

    private

    # The node test that selects this kind of node, or nil when no XPath step
    # can reach it. nil is the default, so a kind nobody taught {#path} answers
    # "?" rather than a path to something else: Element, Attr, Text,
    # CDATASection, Comment and ProcessingInstruction override it.
    def path_step
      nil
    end

    # The name test for an element (+axis+ "") or attribute (+axis+ "@"): the
    # bare name when an unprefixed test reaches this node - it is in
    # +plain_namespace+ and its name is a plain NCName - else by expanded name.
    def name_step(axis, plain_namespace)
      uri = namespace_uri.to_s
      return "#{axis}#{name}" if uri == plain_namespace.to_s && XPathSyntax.plain_name?(name)

      XPathSyntax.expanded_name_test(axis, local_name, uri)
    end

    # The namespace an unprefixed XPath element name test selects. A
    # representation's NodeMethods module states it (HTML: the HTML namespace;
    # XML: none); a new one that forgets is told so here, not by a NoMethodError
    # from inside #path.
    def unprefixed_element_namespace
      raise NotImplementedError, "#{self.class} does not state its unprefixed element namespace"
    end
  end
  private_constant :NodePath
end
