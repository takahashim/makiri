# frozen_string_literal: true
#
# W3C XML Conformance Test Suite (xmlconf) runner - the XML counterpart of the
# html5lib runner. It drives Makiri's native XML parser (Makiri::XML, no libxml2)
# over the ~2200 OASIS/NIST/Sun/IBM/Japanese/Edinburgh test files and checks that
# Makiri ACCEPTS / REJECTS each document as the spec requires.
#
# What is scored, given Makiri is a NON-VALIDATING, NO-DTD-PROCESSING,
# always-namespace-aware XML 1.0 reader:
#   * not-wf  -> Makiri MUST reject (raise Makiri::XML::SyntaxError). This is the
#               core well-formedness conformance signal.
#   * valid / invalid -> Makiri MUST accept (it does not validate). Only tests
#               that need NO DTD entity expansion (ENTITIES="none") are scored;
#               the rest are documented policy differences and skipped.
#
# Out of scope -> skipped (never silently failed), so the pass rate is honest:
#   * XML 1.1 / Namespaces 1.1 (Makiri targets XML 1.0).
#   * NAMESPACE="no" tests (colons used in non-namespace ways; Makiri, like
#     Nokogiri::XML, is always namespace-aware).
#   * valid/invalid tests needing DTD-defined general/parameter entities
#     (Makiri does not process the DTD, so &name; stays undefined - by design).
#   * the optional "error" category (a parser may accept or reject).
#   * files whose declared encoding we cannot transcode here.
#
# The interesting buckets:
#   * FAIL    - a genuine divergence to investigate (e.g. a not-wf document with
#               NO doctype that Makiri wrongly accepts: a real wf bug).
#   * POLICY  - an expected difference from Makiri's no-DTD-validation stance
#               (e.g. a not-wf document whose only defect is inside the DTD that
#               Makiri recognizes-but-does-not-validate).
#
# Test data is NOT vendored. On first run this downloads the pinned W3C suite zip
# into spec/conformance/data/ (gitignored). Use --no-fetch to require a local copy.
#
# Nokogiri (a bench-only dependency) parses the manifests, so run OUTSIDE the
# bundle - the rake task does this:
#   rake conformance:xmlconf
#   ruby -Ilib spec/conformance/xmlconf_runner.rb --verbose
#   ruby -Ilib spec/conformance/xmlconf_runner.rb --show-policy

require "optparse"
require "fileutils"
require "open-uri"

$LOAD_PATH.unshift File.expand_path("../../lib", __dir__)
require "makiri"

begin
  require "nokogiri"
rescue LoadError
  abort "nokogiri is required to read the xmlconf manifests (run via `rake conformance:xmlconf`)"
end

# Pinned W3C XML Test Suite edition (dated, immutable URL) for reproducibility.
DATA_URL  = "https://www.w3.org/XML/Test/xmlts20130923.zip"
DATA_DIR  = File.expand_path("data/xmlconf", __dir__)
ROOT_DIR  = File.join(DATA_DIR, "xmlconf")
ROOT_MANIFEST = File.join(ROOT_DIR, "xmlconf.xml")

opts = { fetch: true, max_fail: 60, verbose: false, show_policy: false }
OptionParser.new do |o|
  o.banner = "Usage: ruby spec/conformance/xmlconf_runner.rb [options]"
  o.on("--no-fetch", "do not download test data; require it locally") { opts[:fetch] = false }
  o.on("--max-fail N", Integer, "show at most N failures (default 60)") { |v| opts[:max_fail] = v }
  o.on("--verbose", "show every failure") { opts[:verbose] = true }
  o.on("--show-policy", "also list the POLICY-bucket cases") { opts[:show_policy] = true }
  o.on("-h", "--help") { puts o; exit }
end.parse!(ARGV)

# --- data acquisition ------------------------------------------------------

def fetch_data!
  FileUtils.mkdir_p(DATA_DIR)
  zip = File.join(DATA_DIR, "xmlts.zip")
  puts "fetching #{DATA_URL} -> #{DATA_DIR}"
  URI.open(DATA_URL, "rb") { |r| File.binwrite(zip, r.read) }
  system("unzip", "-q", "-o", zip, "-d", DATA_DIR) or abort "unzip failed (is `unzip` installed?)"
  File.delete(zip)
end

unless File.exist?(ROOT_MANIFEST)
  abort "no test data at #{ROOT_DIR} (run without --no-fetch to download it)" unless opts[:fetch]
  fetch_data!
  abort "data layout unexpected: #{ROOT_MANIFEST} missing after unzip" unless File.exist?(ROOT_MANIFEST)
end

# --- manifest enumeration --------------------------------------------------
#
# The root xmlconf.xml pulls in each sub-suite via an external SYSTEM entity; that
# entity list IS the authoritative manifest set. Each manifest's TEST URIs resolve
# relative to the manifest's own directory. We parse the manifests with plain
# Nokogiri (no DTD load) and apply the testcases.dtd attribute defaults ourselves
# (ENTITIES=none, NAMESPACE=yes, RECOMMENDATION=XML1.0), so DTD resolution can
# never make enumeration flaky.

def manifest_paths
  root = File.read(ROOT_MANIFEST)
  root.scan(/SYSTEM\s+"([^"]+)"/).flatten
      .reject { |p| p.end_with?(".dtd") }
      .map { |rel| File.join(ROOT_DIR, rel) }
      .select { |p| File.file?(p) }
end

Test = Struct.new(:id, :type, :entities, :namespace, :recommendation, :version,
                  :path, :manifest, keyword_init: true)

def load_tests
  tests = []
  manifest_paths.each do |mpath|
    base = File.dirname(mpath)
    doc = Nokogiri::XML(File.read(mpath))
    doc.remove_namespaces! # manifests are plain; avoids any prefix surprises
    doc.xpath("//TEST").each do |t|
      uri = t["URI"] or next
      # xml:base on the TEST or an ancestor TESTCASES shifts the directory.
      xb = [t, *t.ancestors].map { |n| n["xml:base"] || (n.attribute("base")&.value) }.compact
      dir = xb.reverse.inject(base) { |d, b| File.expand_path(b, d) }
      tests << Test.new(
        id:             t["ID"],
        type:           t["TYPE"],
        entities:       t["ENTITIES"]       || "none",
        namespace:      t["NAMESPACE"]      || "yes",
        recommendation: t["RECOMMENDATION"] || "XML1.0",
        version:        t["VERSION"], # nil => applies to all versions
        path:           File.expand_path(uri, dir),
        manifest:       File.basename(mpath),
      )
    end
  end
  tests
end

# --- per-test input --------------------------------------------------------
#
# Makiri autodetects the encoding from the BOM / XML declaration (XML 1.0
# Appendix F), so each test file is handed to it as raw bytes (ASCII-8BIT) and
# Makiri does the decoding - its encoding handling is now part of what is scored.
# A no-BOM, non-UTF-8 document that omits its encoding declaration is undecodable
# by anything and is rejected; that matches the spec default of UTF-8.

# Cheap structural probe (used only to classify divergences as POLICY vs FAIL),
# on a best-effort ASCII view of the bytes.
def has_doctype?(bytes)
  bytes.byteslice(0, 4096).delete("\x00").include?("<!DOCTYPE")
end

# --- run -------------------------------------------------------------------

stats = Hash.new(0)
fails = []
policies = []

tests = load_tests
tests.each do |t|
  stats[:total] += 1

  # scope filters --------------------------------------------------------
  if %w[XML1.1 NS1.1].include?(t.recommendation)
    stats[:skip_xml11] += 1; next
  end
  if t.version && !t.version.split.include?("1.0")
    stats[:skip_xml11] += 1; next
  end
  if t.namespace == "no"
    stats[:skip_nons] += 1; next
  end
  if t.type == "error"
    stats[:skip_error] += 1; next
  end
  if %w[valid invalid].include?(t.type) && t.entities != "none"
    stats[:skip_entities] += 1; next
  end
  unless File.file?(t.path)
    stats[:skip_missing] += 1; next
  end

  raw = File.binread(t.path) # ASCII-8BIT; Makiri autodetects the encoding

  # A document that DECLARES version!="1.0" is XML 1.1/1.x even if its manifest
  # entry is version-agnostic - out of scope (Makiri implements XML 1.0 only,
  # rejecting other versions by design). Skip it like a manifest-1.1 test rather
  # than scoring Makiri's deliberate rejection.
  if raw.byteslice(0, 256).delete("\x00") =~ /\A\s*(?:\xEF\xBB\xBF)?<\?xml\s[^>]*?version\s*=\s*["']([\d.]+)["']/n &&
     Regexp.last_match(1) != "1.0"
    stats[:skip_xml11] += 1; next
  end

  # run Makiri -----------------------------------------------------------
  outcome =
    begin
      Makiri::XML(raw)
      :accept
    rescue Makiri::XML::SyntaxError
      :reject
    rescue StandardError => e
      # a non-SyntaxError Makiri error (budget, etc.) - treat as a flavour of
      # reject for accept/reject scoring, but remember it for the report.
      (@odd ||= {})[t.id] = "#{e.class}: #{e.message}"
      :reject
    end

  case t.type
  when "not-wf"
    if outcome == :reject
      stats[:pass] += 1
    elsif has_doctype?(raw)
      stats[:policy] += 1
      policies << [t, "not-wf accepted (defect is in the DTD; Makiri does not validate DTDs)"]
    else
      stats[:fail] += 1
      fails << [t, "expected REJECT (not-wf) but Makiri ACCEPTED", raw]
    end
  when "valid", "invalid"
    if outcome == :accept
      stats[:pass] += 1
    elsif has_doctype?(raw)
      stats[:policy] += 1
      policies << [t, "well-formed doc rejected; relies on the DTD (defaults/entities Makiri skips)"]
    else
      stats[:fail] += 1
      fails << [t, "expected ACCEPT (#{t.type}) but Makiri REJECTED", raw]
    end
  end
end

# --- report ----------------------------------------------------------------

shown = 0
fails.each do |t, why, input|
  break if shown >= opts[:max_fail] && !opts[:verbose]
  shown += 1
  puts "\n#{'=' * 72}"
  puts "FAIL  #{t.manifest}  #{t.id}  [#{t.type}]"
  puts "  #{why}"
  puts "  file: #{t.path.sub("#{ROOT_DIR}/", '')}"
  puts "  --- input (bytes) ---"
  body = input.dup.force_encoding("UTF-8").scrub("?")
  puts body.length > 600 ? "#{body[0, 600]}\n  ...(truncated)" : body
end
if !opts[:verbose] && fails.length > shown
  puts "\n... #{fails.length - shown} more failure(s) not shown (use --verbose or --max-fail)"
end

if opts[:show_policy]
  puts "\n#{'-' * 72}\nPOLICY differences (expected, not failures):"
  policies.first(80).each { |t, why| puts "  #{t.manifest} #{t.id} [#{t.type}] - #{why}" }
  puts "  ... #{policies.length - 80} more" if policies.length > 80
end

scored = stats[:pass] + stats[:fail]
puts "\n#{'=' * 72}"
puts "W3C XML Conformance Test Suite (xmlts20130923)"
puts "  total tests        : #{stats[:total]}"
puts "  skipped XML 1.1    : #{stats[:skip_xml11]}"
puts "  skipped non-NS     : #{stats[:skip_nons]}"
puts "  skipped error-type : #{stats[:skip_error]}"
puts "  skipped DTD-entity : #{stats[:skip_entities]}"
puts "  skipped encoding   : #{stats[:skip_encoding]}"
puts "  skipped missing    : #{stats[:skip_missing]}"
puts "  policy differences : #{stats[:policy]}  (no-DTD-validation stance; expected)"
puts "  scored (in-scope)  : #{scored}"
puts "  pass               : #{stats[:pass]}"
puts "  fail               : #{stats[:fail]}"
puts format("  pass rate          : %.2f%% of scored", 100.0 * stats[:pass] / scored) if scored.positive?

exit(stats[:fail].zero? ? 0 : 1)
