# Differential probe for dom_adapter/source_loc.c -> dom_adapter::source_loc.
#
# Node#line for every node of every fixture. This subsystem is the one where a
# wrong answer is quiet: a line is a plausible integer whatever it says, so the
# only useful check is that the two builds agree node-for-node, AND that the
# answers are actually mostly non-nil (a build that lost the recorder would
# answer nil everywhere and "agree" with nothing).
# The probes are run from spec/differential/run.rb, which puts lib/ on the
# load path; this makes them work when run directly too. The original line
# here expanded "lib" against THIS directory, which does not exist - it had
# been doing nothing, and the -Ilib on the command line was carrying it.
$LOAD_PATH.unshift File.expand_path("../../../lib", __dir__)
require "makiri"

COUNTS = Hash.new(0)
def note(k) = COUNTS[k] += 1

FIXTURES = {
  # Line 1 only.
  "oneline" => "<html><body><p>a</p><p>b</p></body></html>",

  # Ordinary multi-line, the common case.
  "multiline" => <<~H,
    <!doctype html>
    <html>
      <head><title>t</title></head>
      <body>
        <div id="a">
          <p>one</p>
          <p>two</p>
        </div>
      </body>
    </html>
  H

  # CRLF: the tokenizer normalises CR/CRLF to LF, but the OFFSETS are into the
  # pre-normalisation bytes, so this is where a line table can drift.
  "crlf" => "<html>\r\n<body>\r\n<p>a</p>\r\n<p>b</p>\r\n</body>\r\n</html>",

  # A lone CR, which is a line break in HTML but not a newline byte.
  "lone-cr" => "<html>\r<body>\r<p>a</p>\r</body>\r</html>",

  # No trailing newline, and a trailing newline: the last-line boundary.
  "no-trailing-nl" => "<p>a</p>\n<p>b</p>",
  "trailing-nl" => "<p>a</p>\n<p>b</p>\n",

  # Consecutive newlines: empty lines must still advance the count.
  "blank-lines" => "<p>a</p>\n\n\n<p>b</p>\n\n<p>c</p>",

  # Implicit html/head/body, which the parser inserts and the tracker cannot
  # place - these must stay nil rather than borrow a neighbour's line.
  "implicit" => "\n\n<p>only</p>",

  # Foster parenting: the tree builder MOVES these out of the table, so the
  # DOM order and the token order diverge. The bounded lookahead is what keeps
  # a mismatch unstamped instead of wrong.
  "foster" => "<table>\n<b>fostered</b>\n<tr><td>cell</td></tr>\n</table>",

  # The adoption agency, the other reordering the comment names.
  "adoption" => "<p>1<b>2<i>3</b>4</i>5</p>",

  # Deeply nested, so the pre-order walk and the cursor stay in step over depth.
  "deep" => "<a>\n<b>\n<c>\n<d>\n<e>x</e>\n</d>\n</c>\n</b>\n</a>",

  # Many repeats of one tag: the lookahead window matters here.
  "repeats" => (1..200).map { |i| "<p>#{i}</p>" }.join("\n"),

  # Mixed content and comments between elements.
  "mixed" => "<div>\ntext\n<!-- c -->\n<span>s</span>\ntail\n</div>",

  # Self-closing / void tags, which keep the CLOSE bit clear and ARE recorded.
  "void" => "<div>\n<br/>\n<img src='x'>\n<hr>\n<input>\n</div>",

  # SVG, where the tag ids come from a different namespace.
  "svg" => "<div>\n<svg>\n<path d='M0 0'/>\n</svg>\n</div>",

  # A custom element, whose tag id is a POINTER value rather than a small id.
  "custom" => "<div>\n<my-el>x</my-el>\n<my-el>y</my-el>\n</div>",

  # Multibyte UTF-8 ahead of the elements: offsets are BYTES, lines are lines.
  "multibyte" => "<p>日本語</p>\n<p>café</p>\n<p>x</p>",

  # Invalid UTF-8, so the sanitiser rewrites the buffer and the offsets are
  # into the SANITISED bytes.
  "invalid-utf8" => "<p>a\xC3(b</p>\n<p>c</p>",
}.freeze

out = []
FIXTURES.each do |name, html|
  doc = Makiri::HTML(html.dup.force_encoding("BINARY"))
  i = 0
  stack = doc.children.to_a
  until stack.empty?
    n = stack.shift
    stack = n.children.to_a + stack
    line = begin
      n.line
    rescue => e
      "RAISE #{e.class}"
    end
    note(line.nil? ? :nil_line : :placed)
    out << "#{name}[#{i}] #{n.class.name.split("::").last}/#{begin n.name rescue "?" end} line=#{line.inspect}"
    i += 1
  end
end

# A document large enough to exercise geometric growth of the recorder.
big = (1..5000).map { |i| "<p id='p#{i}'>#{i}</p>" }.join("\n")
doc = Makiri::HTML("<html><body>#{big}</body></html>")
lines = doc.css("p").map(&:line)
out << "big.count=#{lines.size} first=#{lines.first.inspect} last=#{lines.last.inspect}"
out << "big.monotonic=#{lines.compact.each_cons(2).all? { |a, b| b >= a }}"
out << "big.nils=#{lines.count(nil)}"
note(:big)

puts out
puts "---- branch counts ----"
COUNTS.sort.each { |k, v| puts "#{k}=#{v}" }
