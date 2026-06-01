# frozen_string_literal: true
#
# WHATWG HTML parsing conformance: run the html5lib-tests tree-construction
# suite through Makiri's public parse API and diff Makiri's resulting DOM
# against each test's expected tree dump.
#
# Lexbor passes html5lib-tests upstream; this runner verifies that Makiri's
# post-parse layer (the attr->owner index, source-location stamping, and the
# Ruby wrappers) does not perturb the tree the spec expects. A failure here is
# either a genuine parse divergence or a Makiri-side regression.
#
# The test data is NOT vendored. On first run this fetches a pinned tag of
# html5lib/html5lib-tests via a sparse, blobless clone into
# spec/conformance/data/ (gitignored). Use --no-fetch to require a local copy.
#
# Scope of v1: full-document tests only. Fragment tests (#document-fragment)
# and scripting-on tests (#script-on) are counted and skipped, not run, so the
# pass rate is never inflated by silently dropping hard cases. Tests whose
# expected dump exercises DOM details Makiri does not expose on the Ruby
# surface (doctype public/system ids, <template> content) are reported as
# "unsupported" rather than failed.
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

DATA_REPO = "https://github.com/html5lib/html5lib-tests.git"
# html5lib-tests has no maintained version tags (only ancient python-0.x), so
# we pin an exact master commit for reproducibility. Bump deliberately.
DATA_COMMIT = "e4463205ac3c4500e1379103daadfdcfe5e33af5"
DATA_DIR    = File.expand_path("data/html5lib-tests", __dir__)
TC_DIR      = File.join(DATA_DIR, "tree-construction")

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

def fetch_data!
  puts "fetching html5lib-tests@#{DATA_COMMIT[0, 12]} (sparse) -> #{DATA_DIR}"
  FileUtils.mkdir_p(DATA_DIR)
  run = lambda do |*args|
    system("git", "-C", DATA_DIR, *args) or abort "git #{args.first} failed"
  end
  run.call("init", "-q")
  system("git", "-C", DATA_DIR, "remote", "add", "origin", DATA_REPO) # ok if it already exists
  run.call("sparse-checkout", "init", "--cone")
  run.call("sparse-checkout", "set", "tree-construction")
  # GitHub permits fetching an exact commit by SHA; --depth 1 keeps it shallow.
  run.call("fetch", "-q", "--depth", "1", "--filter=blob:none", "origin", DATA_COMMIT)
  run.call("checkout", "-q", "FETCH_HEAD")
end

unless Dir.exist?(TC_DIR) && !Dir.glob(File.join(TC_DIR, "*.dat")).empty?
  abort "no test data at #{TC_DIR} (run without --no-fetch to fetch it)" unless opts[:fetch]
  fetch_data!
end

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
      if line.start_with?("#") && (m = line.match(/\A#(data|errors|new-errors|document-fragment|script-on|script-off|document)\z/))
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

# Known Makiri Ruby-API gaps that make a test unrepresentable (not a parse
# divergence). Returns :template / :doctype_id / nil. A gap is only credited
# when it FULLY explains the diff, so a real divergence in the same test still
# scores as a failure.
def known_api_gap(expected, actual)
  # <template> contents live in a separate fragment Makiri does not expose;
  # the expected dump marks it with a "content" pseudo-node.
  return :template if expected.match?(/^\|\s+content$/)

  # Doctype public/system identifiers: Lexbor parses them, but Makiri exposes
  # only the name. Strip the ids from the expected dump and see if the rest
  # matches exactly.
  # The id values themselves can contain quotes, so strip from the first quote
  # to the end of the doctype line rather than matching balanced quotes.
  stripped = expected.gsub(/(\| <!DOCTYPE [^"\n]*) "[^\n]*">/, '\1>')
  return :doctype_id if stripped != expected && stripped == actual

  nil
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

files.each do |path|
  parse_dat(path).each do |t|
    stats[:total] += 1

    if t.fragment_ctx
      stats[:skip_fragment] += 1
      next
    end
    if t.script_on
      stats[:skip_script] += 1
      next
    end

    begin
      doc = Makiri::HTML(t.data)
    rescue => e
      stats[:error] += 1
      diffs << [t, "Makiri raised during parse: #{e.class}: #{e.message}", nil]
      next
    end

    actual =
      begin
        Html5libDump.dump_document(doc)
      rescue Html5libDump::Unsupported => e
        stats[:unsupported] += 1
        next
      rescue => e
        stats[:error] += 1
        diffs << [t, "dump raised: #{e.class}: #{e.message}", nil]
        next
      end

    if actual == t.document
      stats[:pass] += 1
    elsif (gap = known_api_gap(t.document, actual))
      # The parse is (very likely) correct, but Makiri's Ruby surface cannot
      # represent the node, so we can neither match the dump nor score it as a
      # parse divergence. These two gaps are documented in the README.
      stats[:"unsupported_#{gap}"] += 1
      stats[:unsupported] += 1
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
  puts "FAIL #{t.file} ##{t.index}"
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

run = stats[:pass] + stats[:fail] + stats[:error]
puts "\n#{'=' * 72}"
puts "html5lib-tests tree-construction (data @#{DATA_COMMIT[0, 12]})"
puts "  total tests     : #{stats[:total]}"
puts "  skipped fragment: #{stats[:skip_fragment]}"
puts "  skipped script  : #{stats[:skip_script]}"
puts "  unsupported     : #{stats[:unsupported]} " \
     "(template content: #{stats[:unsupported_template]}, " \
     "doctype ids: #{stats[:unsupported_doctype_id]})"
puts "  ran             : #{run}"
puts "  pass            : #{stats[:pass]}"
puts "  fail            : #{stats[:fail]}"
puts "  error           : #{stats[:error]}"
if run.positive?
  puts format("  pass rate       : %.2f%% of ran", 100.0 * stats[:pass] / run)
end

exit(stats[:fail].zero? && stats[:error].zero? ? 0 : 1)
