#!/usr/bin/env ruby
# frozen_string_literal: true

require "optparse"
require "pathname"
require "yaml"

ROOT = Pathname.new(__dir__).join("..").expand_path
ALLOWLIST_PATH = ROOT.join("script/check_c_safety_allowlist.yml")

Rule = Struct.new(:id, :message, :regex, keyword_init: true)
Finding = Struct.new(:path, :line, :rule, :text, keyword_init: true)

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
    id: "valid_text_forge",
    message: "mkr_valid_text_t must be minted only by mkr_text_from_view (the validated boundary)",
    regex: /\(\s*mkr_valid_text_t\s*\)\s*\{/
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
