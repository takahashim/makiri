# frozen_string_literal: true
#
# Browsers as auxiliary evidence for the HTML parser: parse the same inputs in
# Makiri and in Chromium, Firefox and WebKit (Playwright, scripting disabled)
# and report where the DOM trees differ.
#
# This is NOT a gate and not an arbiter. docs/rewrite_parser.ja.md fixes the
# pinned HTML Standard as the primary authority: two browsers agreeing does not
# make an answer right, since both can predate a spec change. A difference found
# here is classified by hand (spec change / reference not updated / Makiri bug /
# intended compatibility difference) into docs/parser_differences.ja.md, with
# the spec step that decides it. To help with that, each reported case also says
# whether Makiri matches the pinned test data and whether it is already a known
# divergence.
#
# Trees are compared whole: node kinds, namespace/prefix/local name of elements
# and attributes, attribute order, character data and its node boundaries,
# doctype ids, template contents, and quirks mode as document.compatMode can
# show it (limited-quirks reads as CSS1Compat in a browser, so it is folded).
#
# Needs Node.js and `npm install` in spec/conformance/browser (playwright-core,
# pinned to the build whose browsers `npx playwright install` caches).
#
# Usage:
#   ruby -Ilib spec/conformance/browser_diff.rb                 # the WPT .dat inputs
#   ruby -Ilib spec/conformance/browser_diff.rb --file a.html   # extra documents
#   ruby -Ilib spec/conformance/browser_diff.rb --browsers chromium,firefox --verbose
#   ruby -Ilib spec/conformance/browser_diff.rb --no-dat --generate 20000 --seed 1
#
# --generate adds random fragments (html_gen.rb), parsed in a context element
# in-page: about as fast as the browsers parse, where a document costs a frame
# load each. The test data and the known divergences do not cover them, so a
# generated case reports test-data=- and is classified by hand like any other.

require "json"
require "open3"
require "optparse"

$LOAD_PATH.unshift File.expand_path("../../lib", __dir__)
require "makiri"
require_relative "parser_data"
require_relative "html5lib_dump"
require_relative "html_gen"

opts = { browsers: "chromium,firefox,webkit", files: [], verbose: false, dat: true, generate: 0, seed: 1 }
OptionParser.new do |o|
  o.on("--browsers LIST", "comma-separated (default chromium,firefox,webkit)") { |v| opts[:browsers] = v }
  o.on("--file PATH", "also compare this HTML file as a document") { |v| opts[:files] << v }
  o.on("--no-dat", "skip the WPT tree-construction inputs") { opts[:dat] = false }
  o.on("--generate N", Integer, "also compare N random fragments (html_gen.rb)") { |v| opts[:generate] = v }
  o.on("--seed S", Integer, "the generator's seed (default 1)") { |v| opts[:seed] = v }
  o.on("--verbose", "print the differing trees") { opts[:verbose] = true }
end.parse!(ARGV)

HTML_NS = Html5libDump::HTML_NS
NS = { "svg" => Html5libDump::SVG_NS, "math" => Html5libDump::MATH_NS }.freeze

Case = Struct.new(:key, :kind, :data, :context, :expected, keyword_init: true)

def dat_cases
  ParserData.ensure!(ParserData::WPT)
  Dir.glob(File.join(ParserData::WPT.data_dir, "*.dat")).sort.flat_map do |path|
    file = File.basename(path)
    blocks = File.read(path, encoding: "UTF-8").split(/^#data\n/).drop(1)
    blocks.each_with_index.filter_map do |chunk, i|
      sections = Hash.new { |h, k| h[k] = [] }
      section = "data"
      chunk.each_line(chomp: true) do |line|
        if (m = line.match(/\A#(data|errors|new-errors|document-fragment|script-on|script-off|document|description)\z/))
          section = m[1]
          sections[section]
          next
        end
        sections[section] << line
      end
      next if sections.key?("script-on")

      doc = sections["document"]
      doc.pop if doc.last == ""
      ctx = sections["document-fragment"].first
      Case.new(key: "#{file}##{i + 1}", kind: ctx ? "fragment" : "document",
               data: sections["data"].join("\n"), context: ctx, expected: doc.join("\n"))
    end
  end
end

def tree(node)
  case node.node_type
  when 1
    attrs = node.attribute_nodes.map { |a| [a.namespace_uri, a.prefix, a.local_name, a.value] }
    template = node.namespace_uri == HTML_NS && node.local_name == "template"
    contents = template ? (node.content_fragment&.children || []).map { |c| tree(c) } : nil
    ["el", node.namespace_uri, node.prefix, node.local_name, attrs,
     template ? [] : node.children.map { |c| tree(c) }, contents]
  when 3, 4 then ["text", node.content]
  when 7 then ["pi", node.name, node.content]
  when 8 then ["comment", node.content]
  when 10 then ["doctype", node.name.to_s, node.public_id.to_s, node.system_id.to_s]
  else ["other", node.node_type]
  end
end

def fragment_context(ctx)
  return ctx unless ctx.include?(" ")

  ns, local = ctx.split(" ", 2)
  # In a no-quirks document, as dump.mjs makes its context element (P17: the
  # context's document's mode is the fragment's).
  host = local == ns ? Makiri::HTML("<!DOCTYPE html><#{ns}></#{ns}>") : Makiri::HTML("<!DOCTYPE html><#{ns}><#{local}></#{local}></#{ns}>")
  host.at_xpath("//*[local-name()='#{local}' and namespace-uri()='#{NS.fetch(ns)}']")
end

def makiri_tree(c)
  if c.kind == "document"
    doc = Makiri::HTML(c.data)
    mode = doc.quirks_mode == 1 ? "BackCompat" : "CSS1Compat"
    { "mode" => mode, "children" => doc.children.map { |n| tree(n) } }
  else
    frag = Makiri::DocumentFragment.parse(c.data, context: fragment_context(c.context))
    { "mode" => nil, "children" => frag.children.map { |n| tree(n) } }
  end
rescue StandardError => e
  { "error" => "#{e.class}: #{e.message}" }
end

def html5lib_of(t)
  return t["error"] if t["error"]

  lines = []
  walk = lambda do |n, depth|
    pad = "| #{'  ' * depth}"
    case n[0]
    when "el"
      _, ns, _prefix, local, attrs, kids, contents = n
      tag = { Html5libDump::SVG_NS => "svg ", Html5libDump::MATH_NS => "math " }[ns].to_s + local
      lines << "#{pad}<#{tag}>"
      attrs.map { |(ans, pre, loc, v)| [ans && ans != HTML_NS && pre ? "#{pre} #{loc}" : (pre ? "#{pre}:#{loc}" : loc), v] }
           .sort.each { |(k, v)| lines << %(#{pad}  #{k}="#{v}") }
      if contents
        lines << "#{pad}  content"
        contents.each { |c| walk.call(c, depth + 2) }
      else
        kids.each { |c| walk.call(c, depth + 1) }
      end
    when "text" then lines << %(#{pad}"#{n[1]}")
    when "comment" then lines << "#{pad}<!-- #{n[1]} -->"
    when "pi" then lines << "#{pad}<?#{n[1]} #{n[2]}?>"
    when "doctype"
      lines << (n[2].empty? && n[3].empty? ? "#{pad}<!DOCTYPE #{n[1]}>" : %(#{pad}<!DOCTYPE #{n[1]} "#{n[2]}" "#{n[3]}">))
    else lines << "#{pad}#{n.inspect}"
    end
  end
  t["children"].each { |c| walk.call(c, 0) }
  (t["mode"] ? ["#mode #{t['mode']}"] : []).concat(lines).join("\n")
end

cases = []
cases.concat(dat_cases) if opts[:dat]
opts[:files].each do |f|
  cases << Case.new(key: f, kind: "document", data: File.read(f, encoding: "UTF-8"), context: nil, expected: nil)
end
HtmlGen.cases(opts[:generate], opts[:seed]).each do |key, data, context|
  cases << Case.new(key: key, kind: "fragment", data: data, context: context, expected: nil)
end
abort "no cases" if cases.empty?

browsers = opts[:browsers].split(",")
node_dir = File.expand_path("browser", __dir__)
unless Dir.exist?(File.join(node_dir, "node_modules", "playwright-core"))
  abort "run `npm install` in #{node_dir} first"
end
payload = JSON.generate(cases.map { |c| { kind: c.kind, data: c.data, context: c.context } })
out, err, status = Open3.capture3("node", File.join(node_dir, "dump.mjs"), browsers.join(","), stdin_data: payload)
abort "browser dump failed:\n#{err}" unless status.success?
results = JSON.parse(out)

known = ParserData.known_divergences("port")
stats = Hash.new(0)
reports = []
cases.each_with_index do |c, i|
  mine = makiri_tree(c)
  theirs = browsers.to_h { |b| [b, results.fetch(b)[i]] }
  agree = theirs.select { |_, t| t == mine }.keys
  if agree.size == browsers.size
    stats[:all_agree] += 1
    next
  end
  unanimous = theirs.values.uniq.size == 1
  category = if agree.empty? && unanimous then :makiri_alone
             elsif agree.empty? then :makiri_with_none
             else :browsers_split
             end
  stats[category] += 1
  spec_data = "-"
  if c.expected
    dump = html5lib_of(mine).sub(/\A#mode [^\n]*\n?/, "")
    spec_data = dump == c.expected ? "matches" : "differs"
  end
  reports << [c, category, agree, spec_data, known[c.key], mine, theirs]
end

reports.each do |c, category, agree, spec_data, known_id, mine, theirs|
  puts "#{category.to_s.ljust(16)} #{c.key}#{c.context ? " (context #{c.context})" : ''}  " \
       "agree=[#{agree.join(',')}] test-data=#{spec_data}#{known_id ? " known=#{known_id}" : ''}"
  puts "  input: #{c.data.inspect[0, 160]}"
  next unless opts[:verbose]

  puts "  --- makiri ---"
  puts html5lib_of(mine).gsub(/^/, "  ")
  theirs.group_by { |_, t| t }.each do |t, bs|
    puts "  --- #{bs.map(&:first).join(', ')} ---"
    puts html5lib_of(t).gsub(/^/, "  ")
  end
end

puts "\n#{'=' * 72}"
puts "Makiri vs browsers (#{browsers.map { |b| "#{b} #{results["#{b}_version"]}" }.join(', ')})"
puts "  cases                 : #{cases.size}"
puts "  all agree             : #{stats[:all_agree]}"
puts "  makiri alone          : #{stats[:makiri_alone]}  (every browser agrees on another tree)"
puts "  makiri with none      : #{stats[:makiri_with_none]}  (browsers split, none agrees with makiri)"
puts "  browsers split        : #{stats[:browsers_split]}  (some browser agrees with makiri)"
puts "Evidence only: classify each case against the pinned HTML Standard (docs/parser_differences.ja.md)."
