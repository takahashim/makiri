# frozen_string_literal: true

# A mutator converts its arguments with #to_s, which is arbitrary Ruby. A query
# made there rebuilds the document's indexes; if the edit had already dropped
# them, it then changed the tree under indexes built from the tree before it.
# `#text` read text storage the edit had released, and `//p` found removed
# nodes. So the indexes are dropped at the change, after every conversion.
#
# Every example here warms each index from inside #to_s, then checks that the
# indexed answers match a fresh parse of what the edit produced.
RSpec.describe "Mutation argument conversion" do
  # An argument whose #to_s queries +doc+ - rebuilding every index - first.
  def hostile(doc, value)
    Object.new.tap do |o|
      o.define_singleton_method(:to_s) do
        doc.xpath("//*")
        doc.xpath("//@*")
        doc.xpath("//p")
        doc.text
        value
      end
    end
  end

  def snapshot(doc)
    {
      elements: doc.xpath("//*").map(&:name),
      attributes: doc.xpath("//@*").map { |a| [a.name, a.value] },
      p: doc.xpath("//p").map(&:to_s),
      text: doc.root.text
    }
  end

  describe "HTML" do
    let(:doc) { Makiri.HTML(%(<div id="d"><p class="a">hello</p><p>two</p></div>)) }

    def expect_indexes_current(doc)
      expect(snapshot(doc)).to eq(snapshot(Makiri.HTML(doc.to_html)))
    end

    {
      "[]=" => ->(d, x) { d.at_css("p")["title"] = x.call("t") },
      "set_attribute_ns" => ->(d, x) { d.at_css("p").set_attribute_ns(nil, x.call("lang"), "en") },
      "remove_attribute_ns" => ->(d, x) { d.at_css("p").remove_attribute_ns(nil, x.call("class")) },
      "delete" => ->(d, x) { d.at_css("p").delete(x.call("class")) },
      "name=" => ->(d, x) { d.at_css("p").name = x.call("span") },
      "content= on an element" => ->(d, x) { d.at_css("p").content = x.call("x" * 5000) },
      "content= on a text node" => ->(d, x) { d.at_css("p").children.first.content = x.call("y" * 5000) },
      "inner_html=" => ->(d, x) { d.at_css("div").inner_html = x.call("<p>new1</p><p>new2</p>") },
      "outer_html=" => ->(d, x) { d.at_css("p").outer_html = x.call("<p>n</p><p>m</p><b>b</b>") }
    }.each do |name, edit|
      it "keeps the indexes current across #{name}" do
        edit.call(doc, ->(v) { hostile(doc, v) })
        expect_indexes_current(doc)
      end
    end

    it "reads the text an edit wrote, not the storage it released" do
      text = doc.at_css("p").children.first
      text.content = hostile(doc, "z" * 5000)
      expect(doc.at_css("p").text.bytesize).to eq(5000)
    end

    # The parent was read before #to_s ran; a #to_s that removed the receiver
    # left the new content nowhere, and the call reported success.
    it "outer_html= checks the parent after converting its argument" do
      target = doc.at_css("p")
      arg = Object.new.tap { |o| o.define_singleton_method(:to_s) { target.remove && "<b>x</b>" } }
      expect { target.outer_html = arg }.to raise_error(Makiri::Error, /parent/)
      expect(doc.at_css("b")).to be_nil
    end
  end

  # The frozen check ran before the conversion only, so a #to_s that froze the
  # receiver still had its edit go through.
  describe "a receiver frozen by its argument's #to_s" do
    def freezing(node, value)
      Object.new.tap { |o| o.define_singleton_method(:to_s) { node.freeze && value } }
    end

    it "refuses the HTML edit" do
      p_el = Makiri.HTML("<p>t</p>").at_css("p")
      expect { p_el.name = freezing(p_el, "span") }.to raise_error(FrozenError)
      expect(p_el.name).to eq("p")
      expect { p_el["k"] = freezing(p_el, "v") }.to raise_error(FrozenError)
      expect(p_el["k"]).to be_nil
    end

    it "refuses the XML edit" do
      a = Makiri::XML("<r><a>x</a></r>").at_xpath("//a")
      expect { a.content = freezing(a, "zzz") }.to raise_error(FrozenError)
      expect(a.content).to eq("x")
    end
  end

  describe "XML" do
    let(:doc) { Makiri::XML(%(<r><p a="1">hello</p><p>two</p></r>)) }

    def expect_indexes_current(doc)
      expect(snapshot(doc)).to eq(snapshot(Makiri::XML(doc.to_xml)))
    end

    {
      "[]=" => ->(d, x) { d.at_xpath("//p")["b"] = x.call("2") },
      "set_attribute_ns" => ->(d, x) { d.at_xpath("//p").set_attribute_ns(nil, x.call("c"), "3") },
      "remove_attribute_ns" => ->(d, x) { d.at_xpath("//p").remove_attribute_ns(nil, x.call("a")) },
      "delete" => ->(d, x) { d.at_xpath("//p").delete(x.call("a")) },
      "name=" => ->(d, x) { d.at_xpath("//p").name = x.call("q") },
      "content=" => ->(d, x) { d.at_xpath("//p").content = x.call("x" * 5000) }
    }.each do |name, edit|
      it "keeps the name index current across #{name}" do
        edit.call(doc, ->(v) { hostile(doc, v) })
        expect_indexes_current(doc)
      end
    end
  end
end
