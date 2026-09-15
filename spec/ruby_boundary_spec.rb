# frozen_string_literal: true

# A failure at the Ruby boundary comes back as an ordinary exception and leaves
# the receiver usable. The extension returns these failures as errors rather
# than raising from inside its own frames (ext/makiri/rust/src/glue/mod.rs), so
# nothing it holds - a context borrow, a buffer - is skipped on the way out.
# Two bugs came from breaking that rule: a context left "already in use" after
# a rejected expression, and a context leaked per call when a namespace key's
# `to_s` raised. These pin the same paths for the text checks, the argument
# coercion and the node unwraps.
RSpec.describe "Ruby boundary failure paths" do
  let(:raising) { Object.new.tap { |o| def o.to_s = raise("to_s failed") } }
  let(:html) { Makiri::HTML("<div><p id='a'>x</p></div>") }
  let(:xml) { Makiri::XML("<r><a k='v'/></r>") }

  describe "an argument whose to_s raises" do
    it "surfaces the error from an HTML attribute read and write" do
      p = html.at_css("p")
      3.times do
        expect { p[raising] }.to raise_error(RuntimeError, "to_s failed")
        expect { p[raising] = "v" }.to raise_error(RuntimeError, "to_s failed")
        expect { p["k"] = raising }.to raise_error(RuntimeError, "to_s failed")
      end
      p["k"] = "v"
      expect(p["id"]).to eq("a")
      expect(p.to_html).to eq(%(<p id="a" k="v">x</p>))
    end

    it "surfaces the error from an XML attribute read and element creation" do
      3.times do
        expect { xml.root.at_xpath("a")[raising] }.to raise_error(RuntimeError, "to_s failed")
        expect { xml.create_element(raising) }.to raise_error(RuntimeError, "to_s failed")
      end
      expect(xml.root.at_xpath("a")["k"]).to eq("v")
    end

    it "leaves an XPathContext usable" do
      ctx = Makiri::XPathContext.new(html)
      3.times do
        expect { ctx.evaluate(raising) }.to raise_error(RuntimeError, "to_s failed")
        expect { ctx.register_namespace(raising, "urn:x") }.to raise_error(RuntimeError, "to_s failed")
        expect(ctx.evaluate("count(//p)")).to eq(1.0)
      end
    end

    it "surfaces the error from a query and a fragment context" do
      expect { html.xpath(raising) }.to raise_error(RuntimeError, "to_s failed")
      expect { html.fragment("<b/>", context: raising) }.to raise_error(RuntimeError, "to_s failed")
      expect(html.xpath("//p").length).to eq(1)
    end
  end

  describe "a receiver of the wrong kind" do
    it "is a TypeError for an HTML reader, mutator or identity method on an XML node" do
      node = xml.root
      3.times do
        expect { Makiri::HTML::NodeMethods.instance_method(:name).bind(node).call }
          .to raise_error(TypeError, /expected Makiri::HTML::Node/)
        expect { Makiri::HTML::NodeMethods.instance_method(:[]).bind(node).call("k") }
          .to raise_error(TypeError)
        expect { Makiri::HTML::NodeMethods.instance_method(:add_child).bind(node).call(html.at_css("p")) }
          .to raise_error(TypeError)
        expect { Makiri::HTML::NodeMethods.instance_method(:clone_node).bind(node).call }
          .to raise_error(TypeError)
      end
      expect(node.name).to eq("r")
    end

    it "is a TypeError for an XML reader on an HTML node" do
      expect { Makiri::XML::NodeMethods.instance_method(:name).bind(html.at_css("p")).call }
        .to raise_error(TypeError, /expected Makiri::XML::Node/)
      expect(html.at_css("p").name).to eq("p")
    end

    it "is a TypeError for node identity on a non-node" do
      p = html.at_css("p")
      expect { Makiri::HTML::NodeMethods.instance_method(:==).bind(Object.new).call(p) }
        .to raise_error(TypeError)
      expect { Makiri::HTML::NodeMethods.instance_method(:pointer_id).bind(Object.new).call }
        .to raise_error(TypeError)
      expect(p == html.at_css("p")).to be(true)
    end
  end
end
