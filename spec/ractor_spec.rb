# frozen_string_literal: true

# The extension declares itself Ractor-safe (`declare_ractor_safe` at the top
# of Init_makiri), so its methods run in any Ractor, each Ractor's threads
# under a GVL of their own. What makes that sound: everything the extension
# and nupur keep for the process is atomic, a OnceLock, thread-local or a
# class, and a Document - with every node, set and context over it - never
# leaves the Ractor that made it, which the last examples pin.
#
# Not :slow: the examples are small, and Makiri's code running in parallel is
# what ASan and Valgrind have the most to say about.
RSpec.describe "Makiri in Ractors" do
  around do |example|
    previous = Warning[:experimental]
    Warning[:experimental] = false
    example.run
  ensure
    Warning[:experimental] = previous
  end

  # Ruby 4.0 renamed Ractor#take to #value.
  def result(ractor)
    ractor.respond_to?(:value) ? ractor.value : ractor.take
  end

  # Runs `code` in each of `count` Ractors at once and answers their results
  # (a Ractor's block cannot capture this scope, so the code goes in as a
  # String).
  def in_ractors(count, code)
    ractors = Array.new(count) do |i|
      Ractor.new(code, i) do |source, index|
        Thread.current.report_on_exception = false
        eval(source, binding) # rubocop:disable Security/Eval
      end
    end
    ractors.map { |r| result(r) }
  end

  it "parses, queries, edits and writes HTML outside the main Ractor" do
    answer = in_ractors(1, <<~RUBY).first
      doc = Makiri::HTML("<ul><li class=a>one</li><li>two</li></ul>")
      doc.at_css("li.a").content = "uno"
      doc.at_css("ul") << doc.create_element("li")
      ctx = Makiri::XPathContext.new(doc)
      [doc.css("li").map(&:text), doc.xpath("count(//li)"), ctx.evaluate("string(//li[2])"),
       doc.at_css("ul").to_html]
    RUBY
    expect(answer).to eq([%w[uno two] + [""], 3.0, "two",
                          "<ul><li class=\"a\">uno</li><li>two</li><li></li></ul>"])
  end

  it "parses and queries XML outside the main Ractor" do
    answer = in_ractors(1, <<~RUBY).first
      doc = Makiri::XML('<r xmlns:p="urn:p"><p:x a="1"/><y/></r>')
      [doc.at_xpath("//p:x", "p" => "urn:p")["a"], doc.css("y").length, doc.root.to_xml]
    RUBY
    expect(answer).to eq(["1", 1, '<r xmlns:p="urn:p"><p:x a="1"/><y/></r>'])
  end

  it "raises Makiri's errors there as it does here" do
    errors = in_ractors(1, <<~RUBY).first
      doc = Makiri::HTML("<p>x</p>")
      [(doc.xpath("//p[") rescue $!.class), (doc.css("p[") rescue $!.class)]
    RUBY
    expect(errors).to eq([Makiri::XPath::SyntaxError, Makiri::CSS::SyntaxError])
  end

  it "passes a block's break and throw out of NodeSet#each there" do
    answer = in_ractors(1, <<~RUBY).first
      set = Makiri::HTML("<p>a</p><p>b</p><p>c</p>").css("p")
      [set.each { |p| break p.text }, catch(:out) { set.each { |p| throw :out, p.text * 2 } }]
    RUBY
    expect(answer).to eq(%w[a aa])
  end

  it "transcodes a fragment's String there" do
    text = in_ractors(1, <<~'RUBY').first
      Makiri::HTML::DocumentFragment.parse("<b>日本</b>".encode("Shift_JIS")).children.first.text
    RUBY
    expect(text).to eq("日本")
  end

  it "keeps each Ractor's documents its own while several run at once" do
    answers = in_ractors(4, <<~RUBY)
      200.times.map do |n|
        doc = Makiri::HTML("<ul>" + Array.new(index + 2) { |i| "<li id=i\#{i}>\#{n}</li>" }.join + "</ul>")
        doc.css("li").each { |li| li.content = li.text + "!" }
        GC.start if (n % 50).zero?
        [doc.css("li").length, doc.xpath("string(//li[last()])"), doc.at_css("#i0").text]
      end.uniq.length == 200 && index
    RUBY
    expect(answers).to eq([0, 1, 2, 3])
  end

  describe "a Document never leaves its Ractor" do
    objects = {
      "an HTML Document" => -> { Makiri::HTML("<p>x</p>") },
      "an Element" => -> { Makiri::HTML("<p>x</p>").at_css("p") },
      "a NodeSet" => -> { Makiri::HTML("<p>x</p>").css("p") },
      "an XPathContext" => -> { Makiri::XPathContext.new(Makiri::HTML("<p>x</p>")) },
      "an XML Document" => -> { Makiri::XML("<r/>") }
    }

    objects.each do |name, make|
      it "is refused as shareable, a copy and a move: #{name}" do
        object = make.call
        expect { Ractor.make_shareable(object) }.to raise_error(Ractor::Error)
        [false, true].each do |move|
          ractor = Ractor.new { Ractor.receive }
          expect { ractor.send(object, move: move) }.to raise_error(StandardError)
          ractor.send(nil)
          result(ractor)
        end
        expect(Ractor.shareable?(object)).to be(false)
      end
    end
  end
end
