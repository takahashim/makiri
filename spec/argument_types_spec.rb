# frozen_string_literal: true

# Makiri converts no argument: a string argument is a String, and a name (an
# attribute or element name, a prefix, a selector, an expression) a String or
# a Symbol. A value that only answers #to_s or #to_str is a TypeError, raised
# before anything changes - a conversion would be the caller's Ruby, run in
# the middle of the call.
RSpec.describe "argument types" do
  let(:to_s_only) { Class.new { def to_s = "v" }.new }
  let(:to_str_only) { Class.new { def to_str = "v" }.new }

  describe "HTML" do
    let(:doc) { Makiri.HTML(%(<div id="d"><p class="a">hello</p></div>)) }
    let(:p_el) { doc.at_css("p") }

    it "refuses a non-String attribute value, leaving the element as it was" do
      [to_s_only, to_str_only, 1, nil].each do |v|
        expect { p_el["title"] = v }.to raise_error(TypeError, /expected String\); call #to_s to convert it/)
      end
      expect(p_el.key?("title")).to be(false)
    end

    it "refuses a non-String, non-Symbol name" do
      expect { p_el[to_s_only] }.to raise_error(TypeError, /expected String or Symbol/)
      expect { p_el.delete(1) }.to raise_error(TypeError)
      expect { doc.create_element(to_str_only) }.to raise_error(TypeError)
      expect(p_el["class"]).to eq("a")
    end

    it "takes a Symbol name" do
      p_el[:title] = "t"
      expect(p_el[:title]).to eq("t")
      expect(p_el.key?(:title)).to be(true)
      expect(doc.at_css(:p)).to eq(p_el)
    end

    it "refuses non-String content and markup" do
      expect { p_el.content = to_s_only }.to raise_error(TypeError)
      expect { doc.at_css("div").inner_html = to_s_only }.to raise_error(TypeError)
      expect { p_el.outer_html = to_str_only }.to raise_error(TypeError)
      expect { doc.fragment(to_s_only) }.to raise_error(TypeError)
      expect(doc.at_css("p").text).to eq("hello")
    end

    it "refuses a non-String query or namespace binding" do
      expect { doc.xpath(to_s_only) }.to raise_error(TypeError)
      expect { doc.css(to_str_only) }.to raise_error(TypeError)
      expect { doc.xpath("//p", { "s" => to_s_only }) }.to raise_error(TypeError)
      expect(doc.xpath("//s:p", s: "http://www.w3.org/1999/xhtml").size).to eq(1)
    end
  end

  describe "XML" do
    let(:doc) { Makiri::XML(%(<r><p a="1">hello</p></r>)) }

    it "refuses a non-String value and takes a Symbol name" do
      p_el = doc.at_xpath("//p")
      expect { p_el["b"] = 2 }.to raise_error(TypeError)
      p_el[:b] = "2"
      expect(p_el["b"]).to eq("2")
      expect { doc.create_element("e", "k" => to_s_only) }.to raise_error(TypeError)
      expect(doc.create_element("e", k: "v")["k"]).to eq("v")
    end

    it "refuses a non-String variable value" do
      ctx = Makiri::XPathContext.new(doc)
      expect { ctx.register_variable("v", 1) }.to raise_error(TypeError)
      ctx.register_variable("v", "1")
      expect(ctx.evaluate("count(//p[@a = $v])")).to eq(1.0)
    end

    it "takes an Encoding, a String or a Symbol as to_xml's encoding" do
      [Encoding::ISO_8859_1, "ISO-8859-1", :"ISO-8859-1"].each do |e|
        expect(doc.to_xml(encoding: e)).to include("ISO-8859-1")
      end
      expect { doc.to_xml(encoding: to_str_only) }.to raise_error(TypeError)
    end

    it "still reads an IO source at parse, as HTML does" do
      require "stringio"
      expect(Makiri::XML(StringIO.new("<r/>")).root.name).to eq("r")
    end
  end

  describe "Integer arguments" do
    let(:to_int_only) { Class.new { def to_int = 0 }.new }
    let(:doc) { Makiri.HTML("<ul><li>a</li><li>b</li></ul>") }

    it "takes an Integer index and refuses anything else, calling no to_int" do
      ul = doc.at_css("ul")
      expect(ul.child_at(1).text).to eq("b")
      expect(ul.element_child_at(-1)).to be_nil
      expect { ul.child_at(to_int_only) }.to raise_error(TypeError, /expected Integer\); call #to_i/)
      expect { ul.child_at(1.0) }.to raise_error(TypeError)
      set = doc.css("li")
      expect(set[1].text).to eq("b")
      expect(set[0, 1].size).to eq(1)
      expect(set[0..].size).to eq(2)
      expect(set[..0].size).to eq(1)
      expect { set[to_int_only] }.to raise_error(TypeError)
      expect { set[0, to_int_only] }.to raise_error(TypeError)
      expect { set[0.5..1] }.to raise_error(TypeError)
    end

    it "takes only an Integer indent for to_xml" do
      xml = Makiri::XML("<r><a/></r>")
      expect(xml.to_xml(indent: 2)).to include("\n  <a/>")
      expect { xml.to_xml(indent: to_int_only) }.to raise_error(TypeError)
    end
  end

  it "takes the same String twice, and leaves it changeable afterwards" do
    html = Makiri.HTML("<p>t</p>")
    s = +"dup"
    html.at_css("p")[s] = s
    expect(html.at_css("p")["dup"]).to eq("dup")
    expect(s << "!").to eq("dup!")
  end
end
