# frozen_string_literal: true
#
# Random HTML for spec/conformance/browser_diff.rb's --generate: a token soup
# weighted toward what the tree builder decides - tables, select, template,
# formatting elements, foreign content and its breakout tags, raw-text
# elements, misnested and stray end tags - plus the tokenizer's edges
# (references, comments, bogus markup, CR). Seeded, so a run reproduces.

module HtmlGen
  TAGS = %w[
    html head body p div span a b i u s em strong nobr font code big small tt
    table caption colgroup col thead tbody tfoot tr td th
    select option optgroup selectedcontent datalist
    template form button input image img br hr li ul ol dl dd dt h1 h2 pre listing
    textarea title style script xmp iframe noembed noframes noscript plaintext
    svg math mi mo mtext annotation-xml foreignObject desc path
    frameset frame applet object marquee ruby rb rt rp area keygen
  ].freeze
  ATTRS = %w[id class type href hidden selected disabled encoding xlink:href xmlns xmlns:xlink definitionURL].freeze
  VALUES = ["", "a", "text/html", "hidden", "HIDDEN", "application/xhtml+xml", "x y", "&amp;", "\"q\""].freeze
  TEXTS = ["x", " ", "\n", "\t", "a b", "\r\n", " ", "&amp;", "&notin", "&noti;", "&#0;", "&#x110000;", "&lt;"].freeze
  ODD = ["<!---->", "<!-- c -->", "<!-->", "<!x>", "<?pi x?>", "</>", "<", "</ x>", "<![CDATA[c]]>", "<!DOCTYPE html>", "<a<b>"].freeze
  CONTEXTS = ["body", "div", "table", "tbody", "tr", "td", "select", "template", "svg svg", "math math"].freeze

  module_function

  # `count` cases as [key, data, context], keyed "gen:<seed>:<i>".
  def cases(count, seed)
    rng = Random.new(seed)
    Array.new(count) do |i|
      ["gen:#{seed}:#{i}", soup(rng, 3 + rng.rand(14)), CONTEXTS.sample(random: rng)]
    end
  end

  def soup(rng, n)
    Array.new(n) { piece(rng) }.join
  end

  def piece(rng)
    case rng.rand(10)
    when 0..3 then start_tag(rng)
    when 4..5 then "</#{TAGS.sample(random: rng)}>"
    when 6..7 then TEXTS.sample(random: rng)
    else ODD.sample(random: rng)
    end
  end

  def start_tag(rng)
    tag = TAGS.sample(random: rng)
    tag = tag.upcase if rng.rand(8).zero?
    attrs = Array.new(rng.rand(3)) do
      %( #{ATTRS.sample(random: rng)}="#{VALUES.sample(random: rng).gsub('"', '&quot;')}")
    end
    "<#{tag}#{attrs.join}#{rng.rand(6).zero? ? '/' : ''}>"
  end
end
