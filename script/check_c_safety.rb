#!/usr/bin/env ruby
# frozen_string_literal: true

require "optparse"
require "pathname"
require "yaml"

ROOT = Pathname.new(__dir__).join("..").expand_path
ALLOWLIST_PATH = ROOT.join("script/check_c_safety_allowlist.yml")

# A rule may carry `paths` (an array of path globs): it then applies ONLY to
# matching files. Used for the parser-TU reader discipline, where the ban is
# meaningful only in TUs whose input reads must go through mkr_span_t.
Rule = Struct.new(:id, :message, :regex, :paths, keyword_init: true)
Finding = Struct.new(:path, :line, :rule, :text, keyword_init: true)

# Byte-scanning parser TUs: every input read goes through the bounded reader
# (core/mkr_span.h) - see its header comment. The rules below turn that from a
# convention into a machine-enforced invariant for these files.
PARSER_TUS = %w[
  ext/makiri/xml/mkr_xml_tree.c
  ext/makiri/xml/mkr_xml_chars.c
  ext/makiri/xml/mkr_xml_node.c
  ext/makiri/xpath/mkr_xpath_lex.c
  ext/makiri/bridge/ruby_string.c
  ext/makiri/lexbor_compat/source_loc.c
].freeze

RULES = [
  Rule.new(
    id: "string_value_cstr",
    message: "StringValueCStr bypasses explicit ptr/len handling",
    regex: /\bStringValueCStr\s*\(/
  ),
  Rule.new(
    id: "direct_alloc",
    message: "direct (x)malloc/calloc/realloc/strdup / ALLOC_N must go through safe helpers",
    regex: /\b(?:x?(?:malloc|calloc|realloc|strdup)|ALLOC_N|REALLOC_N)\s*\(/
  ),
  Rule.new(
    id: "direct_strlen",
    message: "strlen requires an explicitly no-NUL checked C string",
    regex: /\bstrlen\s*\(/
  ),
  Rule.new(
    id: "alloca",
    message: "ALLOCA_N uses stack space based on runtime input",
    regex: /\bALLOCA_N\s*\(/
  ),
  Rule.new(
    id: "ruby_string_ptr",
    message: "RSTRING_PTR/RSTRING_LEN must be isolated in checked Ruby input helpers",
    regex: /\bRSTRING_(?:PTR|LEN)\s*\(/
  ),
  Rule.new(
    id: "allocation_plus_one",
    message: "allocation sizes using + 1 need overflow-checked helpers",
    regex: /\b(?:malloc|calloc|realloc|xrealloc)\s*\([^;\n]*\+\s*1\b/
  ),
  Rule.new(
    id: "sizeof_allocation",
    message: "count * sizeof(...) must be overflow-checked before allocation/copy sizing",
    regex: /\*\s*sizeof\s*\(/
  ),
  Rule.new(
    id: "cap_times_two",
    message: "capacity doubling must use an overflow-checked grow helper",
    regex: /(?:\b\w*cap\w*\s*\*\s*2\b|\b2\s*\*\s*\w*cap\w*\b)/
  ),
  Rule.new(
    id: "while_cap_double",
    message: "looped cap *= 2 growth must use an overflow-checked grow helper",
    regex: /while\s*\([^)]*>\s*[^)]*cap[^)]*\).*?\*=\s*2/
  ),
  Rule.new(
    id: "verified_text_forge",
    message: "mkr_verified_text_t must be minted only by mkr_verified_text_from_view (the validated boundary)",
    # Both forge shapes: the compound-literal cast AND the declaration
    # initializer (`mkr_verified_text_t x = {...}`), which the cast-only regex
    # used to miss - that gap let a fuzz harness mint over a non-NUL-terminated
    # buffer unnoticed.
    regex: /\(\s*mkr_verified_text_t\s*\)\s*\{|\bmkr_verified_text_t\s+\w+\s*=\s*\{/
  ),
  # --- HTML/XML representation boundary (see docs/html_xml_boundary_hardening) ---
  # These symbols assume one DOM representation; using them outside their
  # representation-correct / kind-checked home is how shared glue (XPath, NodeSet,
  # node identity) silently treats an XML node as HTML (or vice versa) - an
  # assert-abort or memory type-confusion. Each is allowlisted only in the files
  # that legitimately own it; anywhere else trips the lint.
  Rule.new(
    id: "html_doc_unwrap_boundary",
    message: "mkr_html_doc_unwrap is HTML-only; shared/XML code must use the kind-aware mkr_node_unwrap",
    regex: /\bmkr_html_doc_unwrap\s*\(/
  ),
  Rule.new(
    id: "parsed_html_doc_boundary",
    message: "mkr_parsed_html_doc (asserts kind==HTML) may only be used in a kind-checked / HTML-only site",
    regex: /\bmkr_parsed_html_doc\s*\(/
  ),
  Rule.new(
    id: "parsed_xml_doc_boundary",
    message: "mkr_parsed_xml_doc may only be used in a kind-checked / XML-representation site",
    regex: /\bmkr_parsed_xml_doc\s*\(/
  ),
  Rule.new(
    id: "owner_document_boundary",
    message: "owner_document is an HTML-only lxb field; shared code must compare documents via mkr_node_document",
    regex: /\bowner_document\b/
  ),
  Rule.new(
    id: "node_raw_boundary",
    message: "mkr_node_raw is the kind-agnostic raw pointer (identity / kind-guaranteed only); " \
             "to dereference a node use mkr_html_node or mkr_xml_node_unwrap (kind-checked)",
    regex: /\bmkr_node_raw\s*\(/
  ),
  # --- parser-TU reader discipline (see core/mkr_span.h) ---
  # In the byte-scanning parser TUs every input read must go through the
  # bounded reader: a raw libc scan reintroduces the "forgot the bounds check"
  # class the span made structurally impossible. memcpy stays allowed (an
  # explicit-length copy, not a scan).
  Rule.new(
    id: "raw_scan_call",
    message: "parser TUs must read input through mkr_span_* / mkr_bytes_eq / mkr_utf8_* " \
             "(core), not raw libc scanning",
    regex: /\b(?:memchr|memcmp|strchr|strrchr|strstr|strn?cmp|strcspn|strspn|strpbrk|strtod|strtol|strtoull?|sscanf)\s*\(/,
    paths: PARSER_TUS
  ),
  # The span's own cursor/bound fields are private to core/mkr_span.h: touching
  # `.p` / `.end` in a parser TU is how a hand-rolled (uncovered) cursor starts.
  Rule.new(
    id: "raw_cursor_member",
    message: "parser TUs must not access a span's .p/.end (or keep a raw cursor struct); " \
             "use the mkr_span_* helpers (mark/since for slice capture)",
    regex: /(?:->|\.)\s*(?:p|end)\b/,
    paths: PARSER_TUS
  ),
].freeze

def load_config
  YAML.safe_load(ALLOWLIST_PATH.read, permitted_classes: [], aliases: false) || {}
end

# ignore_paths entries each carry a `path` glob, a `reason`, and an OPTIONAL
# `rule`. Without `rule` the whole file is exempt from every check (e.g. the
# core/ primitives layer). With `rule` only that one check is exempt in the
# matching files (e.g. ruby_string_ptr inside the bridge/ boundary), so the same
# pattern anywhere else still trips the lint.
def load_ignore_paths(raw)
  entries = raw.fetch("ignore_paths", [])
  unless entries.is_a?(Array)
    abort "invalid allowlist: top-level 'ignore_paths' must be an array"
  end

  entries.each_with_index do |entry, idx|
    %w[path reason].each do |key|
      value = entry[key]
      if value.nil? || (value.respond_to?(:empty?) && value.empty?)
        abort "invalid ignore_paths entry ##{idx + 1}: missing #{key}"
      end
    end
    rule = entry["rule"]
    if rule && RULES.none? { |r| r.id == rule }
      abort "invalid ignore_paths entry ##{idx + 1}: unknown rule '#{rule}'"
    end
  end
  entries
end

def path_matches?(pattern, path)
  File.fnmatch?(pattern, path, File::FNM_PATHNAME) ||
    File.fnmatch?(pattern, path, File::FNM_PATHNAME | File::FNM_EXTGLOB)
end

# Whole-file ignore (no `rule`): the file is not scanned at all.
def fully_ignored?(path, ignores)
  ignores.any? { |e| e["rule"].nil? && path_matches?(e["path"], path) }
end

# (path, rule) ignore: drop just that rule's findings in matching files.
def rule_ignored?(path, rule_id, ignores)
  ignores.any? { |e| e["rule"] == rule_id && path_matches?(e["path"], path) }
end

def target_files(ignores)
  Dir.glob(ROOT.join("ext/makiri/**/*.{c,h}").to_s).sort.reject do |file|
    rel = Pathname.new(file).relative_path_from(ROOT).to_s
    fully_ignored?(rel, ignores)
  end
end

def code_line?(line)
  stripped = line.strip
  return false if stripped.empty?
  return false if stripped.start_with?("//", "/*", "*")

  true
end

def scan_findings(ignores)
  target_files(ignores).flat_map do |file|
    rel = Pathname.new(file).relative_path_from(ROOT).to_s
    File.readlines(file).flat_map.with_index(1) do |line, lineno|
      next [] unless code_line?(line)

      RULES.filter_map do |rule|
        next if rule.paths && rule.paths.none? { |pat| path_matches?(pat, rel) }
        next unless line.match?(rule.regex)
        next if rule_ignored?(rel, rule.id, ignores)

        Finding.new(path: rel, line: lineno, rule: rule.id, text: line.strip)
      end
    end
  end
end

def load_allowlist(raw)
  entries = raw.fetch("allowlist", [])
  unless entries.is_a?(Array)
    abort "invalid allowlist: top-level 'allowlist' must be an array"
  end

  entries.each_with_index do |entry, idx|
    %w[path rule max reason].each do |key|
      value = entry[key]
      if value.nil? || (value.respond_to?(:empty?) && value.empty?)
        abort "invalid allowlist entry ##{idx + 1}: missing #{key}"
      end
    end
    unless entry["max"].is_a?(Integer) && entry["max"].positive?
      abort "invalid allowlist entry ##{idx + 1}: max must be a positive integer"
    end
  end
  entries
end

def allowed_counts(entries)
  entries.each_with_object(Hash.new(0)) do |entry, h|
    key = [entry["path"], entry["rule"]]
    h[key] += entry["max"]
  end
end

def finding_key(finding)
  [finding.path, finding.rule]
end

def dump_baseline(findings)
  puts "ignore_paths:"
  puts "  - path: ext/makiri/core/**"
  puts "    reason: Safe allocation and buffer helper internals intentionally contain primitive allocation patterns."
  puts ""

  puts "allowlist:"
  findings.group_by { |f| [f.path, f.rule] }.sort.each do |(path, rule), group|
    puts "  - path: #{path}"
    puts "    rule: #{rule}"
    puts "    max: #{group.length}"
    puts "    reason: Baseline existing occurrence; remove as the C safety refactor replaces it."
  end
end

options = { dump_baseline: false, no_allowlist: false }
OptionParser.new do |opts|
  opts.on("--dump-baseline", "Print an allowlist for the current tree") do
    options[:dump_baseline] = true
  end
  opts.on("--no-allowlist", "--ignore-allowlist", "Report every finding without applying the allowlist") do
    options[:no_allowlist] = true
  end
end.parse!

config = load_config
ignores = load_ignore_paths(config)
findings = scan_findings(ignores)

if options[:dump_baseline]
  dump_baseline(findings)
  exit 0
end

allow_counts = options[:no_allowlist] ? Hash.new(0) : allowed_counts(load_allowlist(config))
seen = Hash.new(0)
violations = []

findings.each do |finding|
  key = finding_key(finding)
  seen[key] += 1
  allowed = allow_counts[key]
  next if seen[key] <= allowed

  violations << finding
end

if violations.empty?
  puts "C safety lint passed (#{findings.length} checked finding(s), all allowlisted)."
  exit 0
end

if options[:no_allowlist]
  warn "C safety lint failed: #{violations.length} finding(s) with allowlist disabled"
else
  warn "C safety lint failed: #{violations.length} unallowlisted finding(s)"
end
violations.each do |finding|
  rule = RULES.find { |r| r.id == finding.rule }
  warn "#{finding.path}:#{finding.line}: #{finding.rule}: #{rule&.message}"
  warn "  #{finding.text}"
end
warn
warn "If this is intentionally safe, add a narrow entry with a reason to #{ALLOWLIST_PATH.relative_path_from(ROOT)}."
exit 1
