# frozen_string_literal: true

# Convenience navigation/query helpers: Node#root / #ancestors / #path and the
# NodeSet set operations + accessors.
RSpec.describe "Makiri convenience API" do
  let(:doc) do
    Makiri::HTML(<<~HTML)
      <html><body>
        <div id="m">
          <p>a</p>
          <p>b</p>
          <span>s</span>
        </div>
      </body></html>
    HTML
  end

  describe "Node#root" do
    it "returns the document root element from any node" do
      expect(doc.at_css("span").root.name).to eq("html")
      expect(doc.root.name).to eq("html")
    end
  end

  describe "Node#ancestors" do
    it "lists ancestor elements nearest-first" do
      p = doc.css("p").last
      expect(p.ancestors.map(&:name)).to eq(%w[div body html])
    end

    it "is empty for the root element" do
      expect(doc.root.ancestors.to_a).to eq([])
    end
  end

  describe "Node#path" do
    it "indexes among same-name siblings and round-trips through at_xpath" do
      second_p = doc.css("p").last
      expect(second_p.path).to eq("/html/body/div/p[2]")
      expect(doc.at_xpath(second_p.path)).to eq(second_p)
    end

    it "omits the index for a unique element" do
      span = doc.at_css("span")
      expect(span.path).to eq("/html/body/div/span")
      expect(doc.at_xpath(span.path)).to eq(span)
    end

    it "uses @name for attributes" do
      attr = doc.at_css("#m").attribute_nodes.first
      expect(attr.path).to eq("/html/body/div/@id")
    end

    it "round-trips a text node" do
      text = doc.at_css("p").child
      expect(doc.at_xpath(text.path)).to eq(text)
    end

    # XPath has no CDATA or PI name test of the node's own name: text() selects
    # a CDATA section together with the text beside it, and a PI is reached by
    # processing-instruction('target'). The path has to count what the engine
    # will count.
    it "round-trips CDATA, processing instructions and the text beside them" do
      xml = Makiri::XML("<r><a/><![CDATA[x]]><?pi d?>t<?pi e?><?other f?></r>")
      paths = xml.root.children.map(&:path)
      expect(paths).to eq(%w[/r/a /r/text()[1] /r/processing-instruction('pi')[1] /r/text()[2]
                             /r/processing-instruction('pi')[2] /r/processing-instruction('other')])
      xml.root.children.each { |child| expect(xml.at_xpath(child.path)).to eq(child) }
    end

    it "round-trips an HTML processing instruction" do
      html = Makiri.HTML("<p>a</p><?pi d?>")
      pi = html.at_css("body").children.find(&:processing_instruction?)
      expect(pi.path).to eq("/html/body/processing-instruction('pi')")
      expect(html.at_xpath(pi.path)).to eq(pi)
    end

    # No step from the document reaches a detached node, and the absolute
    # path its ancestors spell can name an ATTACHED node instead: here, the
    # document's own /div/p. Nokogiri answers "/div/p".
    it "answers ? for a detached node rather than a path to another node" do
      xml = Makiri::XML("<div><p>attached</p></div>")
      div = xml.create_element("div")
      div.add_child(xml.create_element("p"))
      expect(div.path).to eq("?")
      expect(div.first_element_child.path).to eq("?")
      expect(Makiri::Element.new("div", doc).path).to eq("?")

      removed = doc.at_css("span").tap(&:remove)
      expect(removed.path).to eq("?")
    end

    # A doctype has no XPath node type, so its name must not make it count
    # among the elements of that name: "/html[2]/body/p" finds nothing.
    it "does not count a doctype among the root element's siblings" do
      html = Makiri.HTML("<!DOCTYPE html><p>a</p>")
      p_el = html.at_css("p")
      expect(p_el.path).to eq("/html/body/p")
      expect(html.at_xpath(p_el.path)).to eq(p_el)

      xml = Makiri::XML("<!DOCTYPE r><r><a/></r>")
      a = xml.at_xpath("//a")
      expect(a.path).to eq("/r/a")
      expect(xml.at_xpath(a.path)).to eq(a)
    end

    it "answers ? for a node XPath cannot reach, as Nokogiri does" do
      html = Makiri.HTML("<!DOCTYPE html><p>a</p>")
      expect(html.children.first.path).to eq("?") # the doctype
      expect(html.fragment("<p>x</p><p>y</p>").children.last.path).to eq("?")

      xml = Makiri::XML(%(<r xmlns:x="urn:x"/>))
      expect(xml.root.attribute_nodes.first.path).to eq("?") # a namespace declaration

      # (A PI target no XPath literal can quote cannot be made: the factory
      # requires an XML Name, and the parser reads such a target as a comment.)
    end

    # An unprefixed name test selects only the HTML namespace in HTML and no
    # namespace in XML, and a prefix would need registering: anything else is
    # named by expanded name, which evaluates with no setup.
    describe "for namespaced nodes" do
      xmlns = "http://www.w3.org/2000/xmlns/"

      # Every node round-trips, except a declaration in the XMLNS namespace,
      # which is no attribute to XPath and answers "?". An `xmlns:v` on an HTML
      # element is an ordinary attribute there, and must round-trip.
      define_method(:expect_round_trip) do |doc|
        nodes = []
        walk = lambda do |n|
          n.children.each do |c|
            nodes << c
            nodes.concat(c.attribute_nodes.to_a) if c.element?
            walk.(c)
          end
        end
        walk.(doc)
        nodes.each do |node|
          if node.attribute? && node.namespace_uri == xmlns
            expect(node.path).to eq("?")
          else
            expect(doc.at_xpath(node.path)).to eq(node), node.path
          end
        end
      end

      it "round-trips SVG and MathML in HTML, and their namespaced attributes" do
        html = Makiri.HTML(<<~HTML)
          <svg xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 1 1"><a xlink:href="#"/><a/></svg>
          <a>h</a><math><mi>x</mi></math>
        HTML
        svg_a = html.at_xpath("//*[local-name()='a'][1]")
        expect(svg_a.path).to eq("/html/body/*[local-name()='svg' and namespace-uri()='http://www.w3.org/2000/svg']" \
                                  "/*[local-name()='a' and namespace-uri()='http://www.w3.org/2000/svg'][1]")
        expect(html.at_css("body > a").path).to eq("/html/body/a") # not counted with the SVG <a>s
        expect_round_trip(html)
      end

      # The HTML parser takes names XPath cannot write bare: a colon reads as a
      # prefix to resolve ("o:p" from Word, "xml:lang", "v-on:x"), and "@click"
      # or "x!y" is no name at all.
      it "round-trips HTML names that are no plain NCName" do
        html = Makiri.HTML(<<~HTML)
          <html xmlns:v="urn:v" xml:lang="ja"><p>Word<o:p></o:p></p>
          <button @click="go" :href="u" v-on:x="1" data-é="2">b</button><x!y>z</x!y></html>
        HTML
        o_p = html.at_xpath("//*[local-name()='o:p']")
        expect(o_p.path).to eq("/html/body/p/*[local-name()='o:p' and namespace-uri()='http://www.w3.org/1999/xhtml']")
        click = html.at_css("button").attribute_nodes.first
        expect(click.path).to eq("/html/body/button/@*[local-name()='@click' and namespace-uri()='']")
        expect(html.at_css("button").path).to eq("/html/body/button") # a plain name stays bare
        expect_round_trip(html)
      end

      it "round-trips default-namespace and prefixed XML" do
        xml = Makiri::XML(%(<r xmlns="urn:a" xmlns:y="urn:y"><a/><y:a y:k="1" k="2"/><a/><s xmlns=""><a/></s></r>))
        expect(xml.root.path).to eq("/*[local-name()='r' and namespace-uri()='urn:a']")
        expect(xml.at_xpath("//*[local-name()='s']/*").path).to end_with("/a")
        expect_round_trip(xml)
      end
    end
  end

  describe "NodeSet operations" do
    let(:ps)    { doc.css("p") }
    let(:spans) { doc.css("span") }
    let(:all)   { doc.css("p, span") }

    it "unions with | (deduped)" do
      expect((ps | spans).map(&:name)).to eq(%w[p p span])
      expect((ps | ps).length).to eq(2)
    end

    it "concatenates with + (duplicates kept)" do
      expect((ps + ps).length).to eq(4)
    end

    it "intersects with &" do
      expect((all & ps).map(&:name)).to eq(%w[p p])
    end

    it "differences with -" do
      expect((all - ps).map(&:name)).to eq(%w[span])
    end

    it "rejects a non-NodeSet operand" do
      expect { ps | [1, 2] }.to raise_error(TypeError)
    end

    it "stays correct on large operands (pointer-hash path, not O(n^2))" do
      # Above the hash threshold the operators switch from a linear scan to a
      # pointer hash; exercise both the result and that path here.
      big = Makiri::HTML("<ul>#{(1..2000).map { |i| "<li>#{i}</li>" }.join}</ul>")
      lis = big.css("li")
      expect(lis.length).to eq(2000)
      expect((lis | lis).length).to eq(2000)          # union dedup
      expect((lis & lis).length).to eq(2000)          # intersection
      expect((lis - lis).length).to eq(0)             # difference
      half = big.css("li:nth-child(-n+1000)")         # half-overlap
      expect((lis & half).length).to eq(half.length)
      expect((lis - half).length).to eq(2000 - half.length)
    end
  end

  describe "NodeSet accessors" do
    it "exposes first / last / at" do
      ps = doc.css("p")
      expect(ps.first.text).to eq("a")
      expect(ps.last.text).to eq("b")
      expect(ps.at(1).text).to eq("b")
      expect(ps.at(-1).text).to eq("b")
    end
  end
end
