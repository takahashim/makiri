# frozen_string_literal: true

# The port configuration has to agree with itself, and nothing was checking that.
#
# Four files describe one thing between them - which C sources the Rust replaces
# and how to turn each replacement on - and every one of them can drift from the
# others without any build failing:
#
#   ext/makiri/rust_ports.rb      the table: flag, cargo feature, C sources
#   ext/makiri/rust/Cargo.toml    the features those names must exist as
#   .github/workflows/ci.yml      one job per row, or the row is never built
#   ext/makiri/rust/src/lib.rs    the module each feature gates
#
# This session found four separate instances of that drift by hand - a flag list
# two entries behind, a CI leg lost across three commits, a typo'd source path
# that left the Rust dead, a feature whose proof silently did not run. Each was
# found late and by luck. So: a check, run from CI.
#
#   bundle exec ruby script/check_port_table.rb

require_relative "../ext/makiri/rust_ports"

EXT = File.expand_path("../ext/makiri", __dir__)
CARGO = File.join(EXT, "rust", "Cargo.toml")
CI = File.expand_path("../.github/workflows/ci.yml", __dir__)

problems = []

def note(problems, msg) = problems << msg

# ---- 1. every row's sources exist -------------------------------------------
# A path that does not exist does NOT fail the build: the file is simply not
# dropped, both languages define the symbol, and the linker keeps one. Verified
# experimentally - a typo'd row compiled, linked and passed the whole suite with
# the Rust half dead.
begin
  RustPorts.check_paths!(EXT)
rescue RuntimeError => e
  note(problems, e.message)
end

# ---- 2. every row's cargo feature is declared -------------------------------
cargo = File.read(CARGO)
features = cargo[/^\[features\](.*?)^\[/m, 1].to_s
declared = features.scan(/^([a-z0-9-]+)\s*=\s*\[/).flatten.to_set

RustPorts::ALL.each do |row|
  next if declared.include?(row[:feature])

  note(problems, "#{row[:env]}: cargo feature #{row[:feature].inspect} is not declared " \
                 "in Cargo.toml - the build would fail, but only for someone who sets the flag")
end

# ---- 3. every row has a CI leg ----------------------------------------------
# Without one the row is only ever built by the `everything` job, where a
# failure is attributed to whichever other flag is suspected first.
ci = File.read(CI)
RustPorts::ALL.each do |row|
  next if ci.include?("#{row[:env]}=1")

  note(problems, "#{row[:env]}: no CI leg. Add one to the `rust` matrix, or the row is " \
                 "only exercised together with every other flag")
end

# ---- 4. every declared replacement feature is in the table ------------------
# The reverse direction: a feature that gates a replacement but has no row can
# never be turned on by MAKIRI_RUST=all, so it ships untested.
table_features = RustPorts::ALL.map { |r| r[:feature] }.to_set
# Features that deliberately gate something other than a C-file replacement.
NOT_REPLACEMENTS = %w[
  default lexbor-abi alloc-inject glue xpath-engine dom-adapter
].to_set
(declared - table_features - NOT_REPLACEMENTS).sort.each do |f|
  note(problems, "cargo feature #{f.inspect} is declared but has no RustPorts row: " \
                 "MAKIRI_RUST=all cannot turn it on, so it is never exercised")
end

# ---- 5. MAKIRI_RUST=all really is every row ---------------------------------
all = RustPorts.enabled({ "MAKIRI_RUST" => "all" })
missing = RustPorts::ALL.map { |r| r[:env] } - all
note(problems, "MAKIRI_RUST=all omits: #{missing.join(", ")}") unless missing.empty?

# ---- 6. the implications are consistent -------------------------------------
# Each IMPLIES key must itself be a row, or turning it on does nothing.
RustPorts::IMPLIES.each_key do |flag|
  next if RustPorts::ALL.any? { |r| r[:env] == flag }

  note(problems, "IMPLIES names #{flag}, which is not a row - the implication is a no-op")
end

# ---- 7. no C source is claimed by two rows ----------------------------------
seen = {}
RustPorts::ALL.each do |row|
  next if row[:srcs] == :xml_dir

  row[:srcs].each do |src|
    if seen[src]
      note(problems, "#{src} is claimed by both #{seen[src]} and #{row[:env]}: " \
                     "turning on one drops a file the other still needs")
    end
    seen[src] = row[:env]
  end
end

if problems.empty?
  puts "check_port_table: OK - #{RustPorts::ALL.size} rows, each with a feature, a CI leg " \
       "and sources that exist"
  exit 0
end

warn "check_port_table: FAILED"
problems.each { |p| warn "  - #{p}" }
exit 1
