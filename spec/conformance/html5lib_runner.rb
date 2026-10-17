# frozen_string_literal: true
#
# WHATWG HTML parsing conformance: run the tree-construction suite in the
# html5lib .dat format (maintained in WPT since 2026-06) through Makiri's public
# parse API and diff the resulting DOM against each test's expected tree dump.
#
# A failure is either a parse divergence from the pinned data or a Makiri-side
# regression in what sits over the parser (the Ruby wrappers, the fragment
# context, the template contents). Divergences already checked against the
# pinned HTML Standard are listed in KNOWN_DIVERGENCES below.
#
# The test data is NOT vendored: parser_data.rb fetches the pinned WPT commit
# (html/syntax/parsing/resources) on first run. Use --no-fetch to require a
# local copy.
#
# Document and fragment tests both run (a fragment through
# DocumentFragment.parse with its context element). Scripting-on tests
# (#script-on) are counted and skipped, not run: Makiri parses with scripting
# disabled, so the pass rate is never inflated by silently dropping hard cases.
# A test whose dump exercises a DOM detail Makiri cannot represent on the Ruby
# surface is reported as "unsupported" rather than failed; that count is 0.
#
# Usage:
#   ruby -Ilib spec/conformance/html5lib_runner.rb
#   ruby -Ilib spec/conformance/html5lib_runner.rb --file tests1.dat
#   ruby -Ilib spec/conformance/html5lib_runner.rb --max-diffs 40
#   ruby -Ilib spec/conformance/html5lib_runner.rb --verbose      # show every diff
#   ruby -Ilib spec/conformance/html5lib_runner.rb --no-fetch     # fail if data absent

require "optparse"
require "fileutils"

$LOAD_PATH.unshift File.expand_path("../../lib", __dir__)
require "makiri"

require_relative "html5lib_dump"

require_relative "parser_data"

DATA_COMMIT = ParserData::WPT.commit
TC_DIR      = ParserData::WPT.data_dir

# --- options ---------------------------------------------------------------

opts = { fetch: true, max_diffs: 20, verbose: false, file: nil }
OptionParser.new do |o|
  o.banner = "Usage: ruby spec/conformance/html5lib_runner.rb [options]"
  o.on("--file NAME", "run only this .dat file (e.g. tests1.dat)") { |v| opts[:file] = v }
  o.on("--max-diffs N", Integer, "show at most N failing diffs (default 20)") { |v| opts[:max_diffs] = v }
  o.on("--verbose", "show a diff for every failure")               { opts[:verbose] = true }
  o.on("--no-fetch", "do not auto-fetch test data; require it locally") { opts[:fetch] = false }
  o.on("-h", "--help") { puts o; exit }
end.parse!(ARGV)

# --- data acquisition ------------------------------------------------------

unless ParserData::WPT.present?
  abort "no test data at #{TC_DIR} (run without --no-fetch to fetch it)" unless opts[:fetch]
end
ParserData.ensure!(ParserData::WPT) if opts[:fetch]

# --- .dat parsing ----------------------------------------------------------
#
# Each test is a sequence of "#section\n<lines...>" blocks separated by blank
# lines. We need #data (the input), #document (the expected dump), and the
# presence of #document-fragment / #script-on (which gate v1 support).

Test = Struct.new(:data, :document, :fragment_ctx, :script_on, :file, :index, keyword_init: true)

def parse_dat(path)
  raw = File.read(path, encoding: "UTF-8")
  tests = []
  # Blocks begin at "#data" at column 0. Split on the literal "#data\n" markers.
  raw.split(/^#data\n/).each_with_index do |chunk, i|
    next if i.zero? && !chunk.start_with?("#data") # leading empty split

    section = "data"
    buf = { "data" => [] }
    fragment = nil
    script_on = false
    chunk.each_line do |line|
      line = line.chomp("\n")
      if line.start_with?("#") && (m = line.match(/\A#(data|errors|new-errors|document-fragment|script-on|script-off|document|description)\z/))
        section = m[1]
        case section
        when "script-on" then script_on = true
        when "document-fragment" then fragment = :pending
        end
        buf[section] ||= []
        next
      end
      buf[section] << line
      fragment = line if fragment == :pending && section == "document-fragment"
    end
    # The trailing blank line that separated tests becomes a stray "" in the
    # last section; #document lines are exact, so strip one trailing blank only.
    doc = buf["document"] || []
    doc.pop if doc.last == ""
    data = (buf["data"] || []).join("\n")
    tests << Test.new(data: data, document: doc.join("\n"), fragment_ctx: fragment,
                      script_on: script_on, file: File.basename(path), index: tests.length + 1)
  end
  tests
end

# --- run -------------------------------------------------------------------

files =
  if opts[:file]
    [File.join(TC_DIR, opts[:file])].tap { |f| abort "no such file #{f.first}" unless File.exist?(f.first) }
  else
    Dir.glob(File.join(TC_DIR, "*.dat")).sort
  end

stats = Hash.new(0)
diffs = []
known_hits = []
fixed = []

# A fragment test's context is either a bare HTML tag ("td") or a namespaced
# foreign element ("svg desc", "math mi"). The HTML case goes straight through
# the string `context:`; the foreign case needs a Node context (Nokogiri's only
# way to reach a foreign non-root element), which we obtain by parsing a tiny
# host document and grabbing the element of the right tag + namespace.
def fragment_context(ctx)
  return ctx unless ctx.include?(" ")

  ns, local = ctx.split(" ", 2)
  if local == ns # "svg svg" / "math math" - the foreign root element itself
    Makiri::HTML("<#{ns}></#{ns}>").at_css(ns)
  else
    host = Makiri::HTML("<#{ns}><#{local}></#{local}></#{ns}>")
    host.at_xpath("//*[local-name()='#{local}']")
  end
end

# Tests where Makiri's current parser (vendored Lexbor) disagrees with the
# pinned data, each one checked against the pinned HTML Standard and recorded in
# docs/parser_differences.ja.md under the id given in
# spec/conformance/parser_known_divergences.txt (shared with the crate's runner). They are reported, not
# failed: the run still fails on any OTHER difference, and it fails when a
# listed test starts passing, so the table cannot outlive the divergence.
#
KNOWN_DIVERGENCES = ParserData.known_divergences.freeze

files.each do |path|
  parse_dat(path).each do |t|
    stats[:total] += 1

    if t.script_on
      stats[:skip_script] += 1
      next
    end

    actual =
      begin
        if t.fragment_ctx
          frag = Makiri::DocumentFragment.parse(t.data, context: fragment_context(t.fragment_ctx))
          Html5libDump.dump_fragment(frag)
        else
          Html5libDump.dump_document(Makiri::HTML(t.data))
        end
      rescue Html5libDump::Unsupported
        stats[:unsupported] += 1
        next
      rescue => e
        stats[:error] += 1
        diffs << [t, "Makiri raised: #{e.class}: #{e.message}", nil]
        next
      end

    known = KNOWN_DIVERGENCES["#{t.file}##{t.index}"]
    if actual == t.document
      stats[:pass] += 1
      if known
        stats[:fixed] += 1
        fixed << [t, known]
      end
    elsif known
      stats[:known] += 1
      known_hits << [t, known]
    else
      stats[:fail] += 1
      diffs << [t, t.document, actual]
    end
  end
end

# --- report ----------------------------------------------------------------

shown = 0
diffs.each do |t, expected, actual|
  break if shown >= opts[:max_diffs] && !opts[:verbose]

  shown += 1
  puts "\n#{'=' * 72}"
  puts "FAIL #{t.file} ##{t.index}#{t.fragment_ctx ? " (fragment context: #{t.fragment_ctx})" : ''}"
  puts "--- input ---"
  puts t.data
  if actual.nil?
    puts "--- #{expected} ---"
  else
    puts "--- expected ---"
    puts expected
    puts "--- actual (Makiri) ---"
    puts actual
  end
end

if !opts[:verbose] && diffs.length > shown
  puts "\n... #{diffs.length - shown} more failure(s) not shown (use --verbose or --max-diffs)"
end

run = stats[:pass] + stats[:fail] + stats[:error] + stats[:known]
puts "\n#{'=' * 72}"
puts "HTML tree construction (WPT html/syntax/parsing @#{DATA_COMMIT[0, 12]})"
puts "  total tests     : #{stats[:total]}"
puts "  skipped script  : #{stats[:skip_script]}"
puts "  unsupported     : #{stats[:unsupported]}"
puts "  ran             : #{run}"
puts "  pass            : #{stats[:pass]}"
puts "  known divergence: #{stats[:known]}  (docs/parser_differences.ja.md)"
known_hits.each { |t, id| puts "      #{id}  #{t.file} ##{t.index}" }
puts "  fail            : #{stats[:fail]}"
puts "  error           : #{stats[:error]}"
unless fixed.empty?
  puts "  NOW PASSING     : #{fixed.size}  (remove from KNOWN_DIVERGENCES and the table)"
  fixed.each { |t, id| puts "      #{id}  #{t.file} ##{t.index}" }
end
if run.positive?
  puts format("  pass rate       : %.2f%% of ran", 100.0 * stats[:pass] / run)
end

exit(stats[:fail].zero? && stats[:error].zero? && fixed.empty? ? 0 : 1)
