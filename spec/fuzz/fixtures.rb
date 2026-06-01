# frozen_string_literal: true
#
# HTML fixtures for the fuzzer. One is picked per query at random. Small,
# structured documents catch most bugs; `deep` and `flat` push the recursion
# and result-set sizes that toy docs miss.

module FuzzFixtures
  module_function

  def all
    [
      ["simple",  simple],
      ["nested",  nested],
      ["flat",    flat],
      ["mixed",   mixed],
      ["unicode", unicode],
      ["deep",    deep],
    ]
  end

  def simple
    '<html><body><div id="root"><a href="x">link</a></div></body></html>'
  end

  def nested
    <<~HTML
      <html><body>
        <section id="s1"><h1>A</h1><p class="lead">x</p><p>y</p></section>
        <section id="s2"><h1>B</h1><p class="lead">z</p></section>
        <section id="s3"><h2>C</h2></section>
      </body></html>
    HTML
  end

  def flat
    inner = (1..20).map { |i| %(<div class="item" data-i="#{i}">item #{i}</div>) }.join
    "<html><body>#{inner}</body></html>"
  end

  def mixed
    <<~HTML
      <html><body>
        <article><h1>Top</h1>
          <p>intro <em>strong</em> rest</p>
          <ul><li>one</li><li class="hot">two</li><li>three</li></ul>
          <a href="http://example.com">link</a>
          <!-- a comment -->
          <svg><circle id="c" r="5"/></svg>
        </article>
      </body></html>
    HTML
  end

  def unicode
    <<~HTML
      <html><body>
        <p lang="ja">こんにちは <em>世界</em></p>
        <p lang="zh">你好</p>
        <p>arrow › arrow</p>
      </body></html>
    HTML
  end

  # ~80 nested divs to exercise ancestor/descendant walks and recursion guards.
  def deep
    open  = (1..80).map { |i| %(<div class="d#{i}">) }.join
    close = "</div>" * 80
    "<html><body>#{open}<span id=\"leaf\">x</span>#{close}</body></html>"
  end
end
