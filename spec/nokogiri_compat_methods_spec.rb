# frozen_string_literal: true

# Nokogiri-compatible convenience methods layered (in pure Ruby) over Makiri's
# core API: attribute aliases, CSS class helpers, traversal/predicates, NodeSet
# query/mutation shortcuts, and bulk XPath-context registration.
RSpec.describe "Nokogiri-compatible convenience methods" do
  let(:doc) do
    Makiri::HTML(<<~HTML)
      <html><head></head><body>
        <div id="m" class="a b" data-x="1"><p>hi</p><span>  </span></div>
      </body></html>
    HTML
  end
  let(:el) { doc.at_css("#m") }

  describe "attribute aliases" do
    it "attr / get_attribute return the value; attribute returns the node" do
      expect(el.attr("id")).to eq("m")
      expect(el.get_attribute("data-x")).to eq("1")
      expect(el.attribute("class")).to be_a(Makiri::Attribute)
      expect(el.attribute("class").value).to eq("a b")
      expect(el.attribute("nope")).to be_nil
    end

    it "set_attribute / has_attribute? / remove_attribute" do
      el.set_attribute("data-y", "2")
      expect(el["data-y"]).to eq("2")
      expect(el.has_attribute?("data-y")).to be(true)
      expect(el.has_attribute?("nope")).to be(false)
      el.remove_attribute("data-y")
      expect(el["data-y"]).to be_nil
    end
  end

  describe "predicates and name aliases" do
    it "node_name / type / elem? / fragment? / blank?" do
      expect(el.node_name).to eq("div")
      expect(el.type).to eq(1) # element node type
      expect(el.elem?).to be(true)
      expect(el.fragment?).to be(false)
      expect(doc.at_css("span").child.blank?).to be(true)  # whitespace-only text
      expect(doc.at_css("p").child.blank?).to be(false)
    end

    it "node_name= renames the element" do
      el.node_name = "section"
      expect(doc.at_css("section")).to eq(el)
    end
  end

  describe "CSS class helpers" do
    it "classes lists the class names" do
      expect(el.classes).to eq(%w[a b])
      expect(doc.at_css("p").classes).to eq([])
    end

    it "add_class only adds classes not already present" do
      expect(el.add_class("a c")).to eq(el)
      expect(el.classes).to eq(%w[a b c])
    end

    it "append_class adds unconditionally (duplicates allowed)" do
      el.append_class("b")
      expect(el["class"]).to eq("a b b")
    end

    it "remove_class removes the named classes, dropping the attribute when empty" do
      el.remove_class("a")
      expect(el.classes).to eq(["b"])
      el.remove_class("b")
      expect(el.key?("class")).to be(false) # attribute gone
    end

    it "remove_class with no argument removes all classes" do
      el.remove_class
      expect(el.key?("class")).to be(false)
    end
  end

  describe "traverse" do
    it "yields every node depth-first, children before self (post-order)" do
      names = []
      el.traverse { |n| names << n.name }
      expect(names).to eq(%w[text p text span div])
    end
  end

  describe "NodeSet shortcuts" do
    let(:set) { doc.css("div, p, span") }

    it "at_css / at_xpath return the first match" do
      expect(set.at_css("p").name).to eq("p")
      expect(set.at_xpath(".//span").name).to eq("span")
    end

    it "remove_attr clears the attribute on every node" do
      doc.css("div").remove_attr("data-x")
      expect(el["data-x"]).to be_nil
    end
  end

  describe "XPathContext bulk registration" do
    it "register_namespaces / register_variables" do
      ctx = Makiri::XPathContext.new(doc)
      ctx.register_namespaces("svg" => "http://www.w3.org/2000/svg")
      ctx.register_variables("want" => "hi")
      expect(ctx.evaluate("$want")).to eq("hi")
    end
  end

  describe "Document#title=" do
    it "sets the title, creating <title> when absent" do
      doc.title = "Hello"
      expect(doc.title).to eq("Hello")
      expect(doc.at_css("head title")).not_to be_nil
    end
  end
end
