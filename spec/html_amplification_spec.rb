# frozen_string_literal: true

require "spec_helper"

# The HTML parse guard's amplification limit (html/tree/guard.rs, which says
# why): a document may build at most `max_amplification` times its input,
# never less than 64 MiB. "Reconstruct the active formatting elements"
# re-creates every unclosed formatting element before each character, so a
# few hundred <b>s with distinct attributes, then `<div>x</div>` repeated,
# built a document thousands of times the input (62 KB -> 344 MB).
RSpec.describe "HTML amplification limit" do
  let(:message) { /\Adocument size limit exceeded \(256 times the input; see max_amplification:\)\z/ }

  # ~390 formatting elements the stack loses at `</div>`, then `reps`
  # characters that each have them all re-created. Past the 64 MiB floor at a
  # few thousand reps.
  def reconstruct(reps)
    fmt = (1..390).map { |i| "<b x=#{i}>" }.join
    "<div>#{fmt}</div>" + "<div>x</div>" * reps
  end

  def elapsed
    t = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    yield
    Process.clock_gettime(Process::CLOCK_MONOTONIC) - t
  end

  it "refuses a document that builds far past its input, quickly, with a Makiri::Error" do
    input = reconstruct(5000)
    expect(input.bytesize).to be < 70_000
    t = elapsed { expect { Makiri::HTML(input) }.to raise_error(Makiri::Error, message) }
    expect(t).to be < 5
  end

  it "refuses the same through inner_html=, leaving the element as it was" do
    doc = Makiri::HTML("<p id=p>old</p>")
    p = doc.at_css("#p")
    expect { p.inner_html = reconstruct(5000) }.to raise_error(Makiri::Error, message)
    expect(p.inner_html).to eq("old")
  end

  it "accepts an ordinary document, and the same input under the 64 MiB floor" do
    expect(Makiri::HTML("<ul>#{"<li>x" * 20_000}</ul>").css("li").length).to eq(20_000)
    expect(Makiri::HTML(reconstruct(100)).xpath("count(//b)")).to be > 30_000
  end

  describe "max_amplification:" do
    let(:small) { "<p>x</p>" }

    it "takes nil (the default), any Integer, and a negative one for no limit" do
      [nil, 0, 1, 1000, 2**100, -1, -(2**100)].each do |r|
        expect(Makiri::HTML(small, max_amplification: r).at_css("p").text).to eq("x")
      end
    end

    it "is a TypeError for anything but an Integer" do
      ["256", 2.5, :x].each do |r|
        expect { Makiri::HTML(small, max_amplification: r) }
          .to raise_error(TypeError, "max_amplification must be an Integer")
      end
    end

    it "is taken by the fragment parsers too" do
      doc = Makiri::HTML(small)
      expect(doc.fragment("<b>x</b>", max_amplification: -1).children.length).to eq(1)
      expect(Makiri::HTML::DocumentFragment.parse("<b>x</b>", max_amplification: 10).children.length).to eq(1)
      expect { doc.fragment("<b>x</b>", max_amplification: "1") }.to raise_error(TypeError)
    end
  end
end
