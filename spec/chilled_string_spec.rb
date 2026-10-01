# frozen_string_literal: true

# A string literal in a file without the frozen_string_literal comment is
# "chilled" on Ruby 3.4+: not frozen yet, and changing it warns. Holding an
# argument with rb_str_locktmp counted as a change, so every call given such a
# literal warned "literal string will be frozen in the future" at the
# caller's line.
RSpec.describe "a chilled String argument",
               skip: (RUBY_VERSION < "3.4" && "no chilled strings before Ruby 3.4") do
  # This file's literals are frozen; eval'd source without the comment is not.
  def chilled(str) = eval(str.dump) # rubocop:disable Security/Eval

  def warnings
    seen = []
    deprecated = Warning[:deprecated]
    Warning[:deprecated] = true
    hook = Module.new { define_method(:warn) { |msg, **| seen << msg } }
    Warning.singleton_class.prepend(hook)
    yield
    seen
  ensure
    Warning[:deprecated] = deprecated
    hook&.send(:remove_method, :warn)
  end

  it "is read without a warning, and stays chilled" do
    doc = Makiri::HTML("<p id=a>x</p>")
    xml = Makiri::XML("<r/>")
    p_el = doc.at_css("p")
    sel = chilled("p")
    seen = warnings do
      expect(doc.css(sel).size).to eq(1)
      expect(doc.xpath(chilled("//p")).size).to eq(1)
      p_el[chilled("k")] = chilled("v")
      expect(p_el[chilled("k")]).to eq("v")
      p_el.content = chilled("t")
      doc.create_element(chilled("div"))
      doc.fragment(chilled("<b>b</b>"))
      Makiri::HTML(chilled("<i>i</i>"))
      xml.root[chilled("a")] = chilled("b")
      xml.root.set_attribute_ns(chilled("urn:a"), chilled("a:k"), chilled("v"))
      ctx = Makiri::XPathContext.new(doc)
      ctx.register_variable(chilled("v"), chilled("a"))
      expect(ctx.evaluate(chilled("//p[@id=$v]")).size).to eq(1)
    end
    expect(seen).to be_empty
    expect(warnings { sel << "x" }.join).to include("will be frozen")
  end
end
