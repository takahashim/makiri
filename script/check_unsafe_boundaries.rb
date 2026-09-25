# frozen_string_literal: true

# Keep the parts of the extension that are meant to be ordinary Rust that way.
#
# The compiler holds the default: `lib.rs` denies `unsafe_code`, so a file that
# wants unsafe must say `#![allow(unsafe_code)]` and that line shows up in
# review. This script pins the result, the way it already pins `rb_sys::` and
# `static mut` - what the compiler cannot do is notice that an island grew.
#
# Why the count check runs over EVERY file rather than only the islands: an
# `allow` propagates to child modules, so the one on `falloc/mod.rs` covers all
# of `falloc/`. A new file added there could use unsafe with the compiler silent
# AND carry no allow of its own, so a check that only looked at files carrying
# an allow would miss it. Counting everything and demanding an exact match does
# not.

ROOT = File.expand_path("..", __dir__)
RUST = File.join(ROOT, "ext/makiri/rust/src")

# Every file's unsafe count, exact. A new file with unsafe fails here even when
# a parent module's `allow` kept the compiler quiet; a file that loses its last
# unsafe fails until the entry goes, so a reduction lands with the gate that
# records it. The count is `unsafe` followed by `{`, `fn`, `impl`, `trait` or
# `extern`, outside comment lines - the one definition, kept here.
#
# Kept SORTED (the gate fails otherwise), so `--fix` and a hand edit agree and
# two entries for one file cannot both stand: a Ruby Hash literal takes the last
# duplicate silently. Moving a boundary DOWN to `bridge/` raises the number here
# (the unsafe did not disappear, it moved); what ratchets is the total, and the
# summary prints the `glue/` + `xpath/` subtotal that must reach 0.
UNSAFE_ISLANDS = {
  "bridge/alloc.rs" => 4,
  "bridge/doc.rs" => 5,
  "bridge/fragment.rs" => 6,
  "bridge/gvl.rs" => 4,
  "bridge/html.rs" => 30,
  "bridge/node_set.rs" => 9,
  "bridge/ruby.rs" => 23,
  "bridge/string.rs" => 32,
  "bridge/typed.rs" => 21,
  "bridge/wrapper.rs" => 17,
  "bridge/xml.rs" => 14,
  "bridge/xml_decode.rs" => 6,
  "bridge/xpath/context_object.rs" => 5,
  "bridge/xpath/handler.rs" => 8,
  "bridge/xpath/mod.rs" => 6,
  "cbuf.rs" => 15,
  "cbuf/verify.rs" => 7,
  "falloc/calloc_verify.rs" => 3,
  "falloc/cstr.rs" => 2,
  "falloc/mod.rs" => 1,
  "falloc/raw.rs" => 3,
  "gvl.rs" => 4,
  "init.rs" => 4,
  "lexbor/abi.rs" => 4,
  "lexbor/adapter/arena_bytes.rs" => 3,
  "lexbor/adapter/cross_import.rs" => 4,
  "lexbor/adapter/html/build.rs" => 18,
  "lexbor/adapter/html/mod.rs" => 57,
  "lexbor/adapter/html/mutate.rs" => 9,
  "lexbor/adapter/post_parse.rs" => 9,
  "lexbor/adapter/source_loc.rs" => 3,
  "lexbor/adapter/text_index.rs" => 1,
  "lexbor/chunks.rs" => 2,
  "lexbor/css_engine.rs" => 12,
  "lexbor/css_parser.rs" => 20,
  "lexbor/fragment.rs" => 8,
  "lexbor/selectors.rs" => 13,
  "lexbor/serialize.rs" => 2,
  "lexbor/stylesheet.rs" => 7,
  "lexbor/tests.rs" => 2,
  "lexbor/xpath.rs" => 7,
  "rust_tests.rs" => 5,
  "text.rs" => 5,
  "token.rs" => 1,
}.freeze

# Files whose safety is compiler-enforced. Checked by containment, so adding one
# needs no edit here, and kept SORTED (the gate fails otherwise) so `--fix` and a
# hand edit agree. `forbid` cannot be overridden by an inner `allow`, which is
# why a module root whose children need unsafe is not on this list.
#
# The subtree roots in FORBID_ROOTS make whole directories compiler-forbidden:
# `forbid` is inherited, so a new file under `glue/` or `xpath/` cannot slip in
# with an `allow`, however few its `unsafe`s.
FORBID_ROOTS = %w[css/mod.rs glue/mod.rs xml/mod.rs xpath/mod.rs].freeze

FORBID_FILES = %w[
  css/build.rs css/lower.rs css/mod.rs
  cutf8.rs cutf8/verify.rs falloc/verify.rs
  glue/css.rs glue/html_doc.rs glue/html_node/css.rs
  glue/html_node/mutate.rs glue/html_node/read.rs glue/html_node/serialize.rs
  glue/mod.rs glue/node.rs glue/node_set.rs
  glue/query.rs glue/xml_doc.rs glue/xml_node/css.rs
  glue/xml_node/mod.rs glue/xml_node/mutate.rs glue/xml_node/ns.rs
  glue/xml_node/read.rs glue/xml_node/serialize.rs glue/xml_node/strings.rs
  glue/xpath_context.rs lexbor/adapter/dom_index.rs lexbor/adapter/utf8_input.rs
  lexbor/contains_guard.rs limits.rs ptr_table.rs
  xml/arena.rs xml/chars/expand.rs xml/chars/mod.rs
  xml/dom_name.rs xml/encoding_sniff.rs xml/index.rs
  xml/mod.rs xml/model.rs xml/mutate/attr.rs
  xml/mutate/copy.rs xml/mutate/edit.rs xml/mutate/factory.rs
  xml/mutate/insert.rs xml/mutate/mod.rs xml/mutate/ns.rs
  xml/qname.rs xml/selftest.rs xml/serialize/bindings.rs
  xml/serialize/c14n.rs xml/serialize/mod.rs xml/serialize/out.rs
  xml/serialize/xml.rs xml/tree/cursor.rs xml/tree/decl.rs
  xml/tree/dtd.rs xml/tree/mod.rs xml/tree/scope.rs
  xml/verify.rs xml/xpath.rs xpath/abi.rs
  xpath/ast.rs xpath/ast_ops.rs xpath/attr_pred.rs
  xpath/axis.rs xpath/ctx.rs xpath/dom.rs
  xpath/eval.rs xpath/funcs/ext.rs xpath/funcs/mod.rs
  xpath/lex.rs xpath/limits.rs xpath/mod.rs
  xpath/msg.rs xpath/nodetest.rs xpath/number.rs
  xpath/order.rs xpath/parse.rs xpath/step_index.rs
  xpath/str_cache.rs xpath/tests.rs xpath/value.rs
  xpath/verify.rs
].freeze

UNSAFE_USE = /\bunsafe\s*(?:\{|fn\b|impl\b|trait\b|extern\b)/

# These are the remaining C/Ruby ABI globals.  Each entry is deliberately
# exact: adding another static mut must come with a dedicated boundary type or
# an explicit review of its synchronisation proof.
STATIC_MUT_COUNTS = {}.freeze

# Ruby's C API outside `bridge/` is forbidden: the bridge is the only owner of
# raw VALUEs, typed data and the C calls that raise (glue/mod.rs), so a raise
# there becomes an `Err` before it can longjmp past a Rust destructor. This was
# a pinned count while the glue still reached across; the port is done, so the
# table is empty and any `rb_sys::` above the bridge fails. `magnus::rb_sys` is
# magnus's own module (a safe wrapper) and is not counted, and neither are
# comment lines.
RB_SYS_COUNTS = {}.freeze

RAISING_API = /\b(?:rb_raise|rb_exc_raise|rb_jump_tag|rb_check_typeddata)\b/
RAISING_COUNTS = {}.freeze

# `Value::from_raw` outside `bridge/` is the other half of the raw Ruby
# boundary: a `VALUE` the glue already holds, turned back into a `Value`. The
# bridge's `value()` accessor owns that conversion now, so this is zero and any
# direct `Value::from_raw` above the bridge fails.
VALUE_FROM_RAW = /\bValue::from_raw\b/
VALUE_FROM_RAW_COUNTS = {}.freeze

# Lexbor ABI names outside `lexbor/` are forbidden. `lexbor` is the sole owner
# of the vendored C ABI (notes/rust_third_architecture.ja.md): the bindgen types
# (`Lxb*`), the `lxb_*` functions and constants, and `crate::lexbor::abi` must
# not appear above it. The port is done, so the pinned table is empty and any
# name found outside fails; a file that needs one goes through a `lexbor`
# facade. `lexbor/abi.rs` is inside the layer, so the `lexbor/` skip covers it.
LEXBOR_ABI = /crate::lexbor::abi\b|\blxb_[A-Za-z0-9_]+|\bLxb[A-Z][A-Za-z0-9_]*/
LEXBOR_ABI_COUNTS = {}.freeze

# The engine layers are Ruby-free: `lexbor/`, `xml/`, `xpath/` and `css/` take
# bytes and return their own error types, and the Ruby half of a feature lives in
# `bridge/` or `glue/`. `lexbor/fragment.rs` and `lexbor/stylesheet.rs` once
# reached back up for a `VALUE`, the String borrow rules and `Makiri::Error` - an
# engine module holding the bridge's invariants, and the path a raise escaped
# along - so the table is empty and any such use fails.
# The crate-root modules the engine is built on sit outside those directories
# and are held to the same two rules.
ENGINE_DIRS = %w[lexbor/ xml/ xpath/ css/ falloc/ cbuf.rs cutf8.rs limits.rs ptr_table.rs text.rs token.rs].freeze
RUBY_LAYER = /crate::(?:bridge|glue|init)|magnus::/
RUBY_LAYER_COUNTS = {}.freeze

# The engine allocates only through `falloc`, so `rake oom` can fail every site
# and an OOM raises instead of aborting the host. `clippy.toml` bans the
# infallible `Box::new` / `Vec::with_capacity` / `reserve` / `to_vec` crate-wide,
# but it CANNOT ban `ToOwned::to_owned`: on a `&str` that is the String of a Ruby
# exception message, which the glue is right to allocate on a path that is
# already raising. The rule is per-LAYER, so the per-layer half is here.
#
# It is here because three `.to_owned()` calls reached `xml/serialize` unnoticed -
# the engine's only infallible allocations - and the crate-wide lint that would
# have caught them fired on five legitimate glue sites too.
INFALLIBLE_ALLOC = /\.to_owned\(\)|\.to_vec\(\)|String::from\(|\bformat!\(/
INFALLIBLE_ALLOC_COUNTS = {}.freeze

def rust_code(path)
  File.binread(path).lines.reject { |line| line.match?(%r{\A\s*//}) }.join
end

# Comments out entirely, for the Lexbor-name check: a name mentioned in a
# `/* ... */` block is documentation, not a reference, and `rust_code` above
# does not strip those (it drops only whole `//` lines).
def comments_removed(src)
  src.gsub(%r{/\*.*?\*/}m, "").lines.reject { |line| line.match?(%r{\A\s*//}) }.join
end

# `--fix` transcribes the two tables that move whenever code moves, so a hand
# count cannot disagree with the one this script does, and it writes them SORTED
# (the shape the gate also demands), so running it is idempotent. It deliberately
# does NOT touch RB_SYS_COUNTS, STATIC_MUT_COUNTS, RAISING_COUNTS or
# LEXBOR_ABI_COUNTS: those record a boundary DECISION rather than a consequence
# of moving code, and a new direct `rb_sys::` call, a new process-wide mutable
# global, a new raising C call outside `bridge/` or a new Lexbor ABI name above
# `lexbor/` is exactly the thing a person should have to think about. Rewriting
# them automatically would spend the ratchet it exists to hold.
FIX = ARGV.delete("--fix")
abort "usage: check_unsafe_boundaries.rb [--fix]" unless ARGV.empty?

# Only the entries that moved: a pinned table has a dozen rows, and printing
# both copies of it buries the one line that changed.
def table_diff(recorded, actual)
  keys = (recorded.keys | actual.keys).select { |k| recorded[k] != actual[k] }
  keys.sort.map { |k| "#{k} #{recorded[k] || 0} -> #{actual[k] || 0}" }.join(", ")
end

# Replace `NAME = <open> ... <close>` in this script's own source.
def rewrite_table!(source, name, open_tok, close_tok, body)
  head = "#{name} = #{open_tok}\n"
  from = source.index(head) or abort "unsafe-boundaries: cannot find #{name} to rewrite"
  to = source.index("#{close_tok}\n", from) or abort "unsafe-boundaries: #{name} is unterminated"
  source[0...from] + head + body + source[to..]
end

errors = []

# `lexbor::adapter` owns the typed DOM facade.  Keeping this explicit avoids a
# second, gradually diverging compatibility boundary under the old name - and a
# file that still names the old module does not build (the fuzz crate quietly
# did not, because nothing else here looks outside `src/`). So this scans the
# WHOLE crate, build.rs and fuzz/ included, with comments removed so a
# doc-comment mention of the old path is not read as a reference.
CRATE_ROOT = File.join(ROOT, "ext/makiri/rust")
legacy_adapter_refs = Dir.glob(File.join(CRATE_ROOT, "**", "*.rs"))
  .reject { |path| path.include?("/target/") }
  .select { |path| comments_removed(File.binread(path)).match?(/\bdom_adapter\b/) }
unless legacy_adapter_refs.empty?
  paths = legacy_adapter_refs.map { |p| p.delete_prefix("#{CRATE_ROOT}/") }.sort
  errors << "legacy dom_adapter references: #{paths.inspect}"
end

unless File.binread(File.join(RUST, "lib.rs")).include?("#![deny(unsafe_code)]")
  errors << "lib.rs: must retain #![deny(unsafe_code)] - it is what makes the rest a ratchet"
end

# The typed document handle keeps its raw pointer `lexbor`-private, so nothing
# above the layer can read a Lexbor struct field through it. This is what closed
# the `compat_mode` leak: a field read spells no `lxb_*`/`Lxb*` name, so
# LEXBOR_ABI below cannot see it, and only the compiler can enforce this one.
unless File.binread(File.join(RUST, "lexbor/adapter/html/mod.rs"))
    .include?("pub(in crate::lexbor) fn as_raw(self) -> *mut LxbDoc")
  errors << "lexbor/adapter/html/mod.rs: HtmlDoc::as_raw must stay `pub(in crate::lexbor)`"
end

unsafe_actual = Hash.new(0)
forbidding = []
Dir.glob(File.join(RUST, "**", "*.rs")).sort.each do |path|
  relative = path.delete_prefix("#{RUST}/")
  source = File.binread(path)
  forbidding << relative if source.include?("#![forbid(unsafe_code)]")
  count = rust_code(path).scan(UNSAFE_USE).length
  unsafe_actual[relative] = count unless count.zero?
end

if FIX
  src = File.binread(__FILE__)
  before = src.dup
  src = rewrite_table!(src, "UNSAFE_ISLANDS", "{", "}.freeze",
                       unsafe_actual.sort.map { |f, n| %(  "#{f}" => #{n},\n) }.join)
  src = rewrite_table!(src, "FORBID_FILES", "%w[", "].freeze",
                       forbidding.sort.each_slice(3).map { |r| "  #{r.join(' ')}\n" }.join)
  if src == before
    # Nothing to say: the exit status and the summary below already report it.
  else
    File.binwrite(__FILE__, src)
    gained = unsafe_actual.reject { |k, v| UNSAFE_ISLANDS[k] == v }
    lost = UNSAFE_ISLANDS.reject { |k, v| unsafe_actual[k] == v }
    puts "unsafe-boundaries --fix: islands now #{gained.inspect} (were #{lost.inspect}); " \
         "#{forbidding.length} forbid files"
    puts "unsafe-boundaries --fix: review this file's diff - the numbers moved, and why is the commit"
  end
else
  if unsafe_actual != UNSAFE_ISLANDS
    gained = unsafe_actual.reject { |k, v| UNSAFE_ISLANDS[k] == v }
    lost = UNSAFE_ISLANDS.reject { |k, v| unsafe_actual[k] == v }
    errors << "unsafe islands changed: got #{gained.inspect}, recorded #{lost.inspect} " \
              "(`rake unsafe:fix` transcribes it)"
  end

  missing = FORBID_FILES - forbidding
  errors << "lost #![forbid(unsafe_code)]: #{missing.inspect}" unless missing.empty?
  # Two-way, like the islands: a new forbid file is recorded, so a later loss of
  # it is caught above rather than never having been known.
  unrecorded = forbidding - FORBID_FILES
  errors << "unrecorded #![forbid(unsafe_code)] (`rake unsafe:fix`): #{unrecorded.inspect}" \
    unless unrecorded.empty?

  # A whole directory is safe by one inherited attribute, so a stray `allow`
  # under it does not build; the gate only has to keep the root attribute there.
  missing_roots = FORBID_ROOTS.reject { |f| forbidding.include?(f) }
  errors << "subtree roots must keep #![forbid(unsafe_code)]: #{missing_roots.inspect}" \
    unless missing_roots.empty?

  # Both tables are kept sorted so `--fix` (which sorts) and a hand edit agree
  # and a duplicate key cannot hide: Ruby's Hash literal keeps the last silently.
  errors << "UNSAFE_ISLANDS must stay sorted (`rake unsafe:fix`)" \
    if UNSAFE_ISLANDS.keys != UNSAFE_ISLANDS.keys.sort
  errors << "FORBID_FILES must stay sorted (`rake unsafe:fix`)" \
    if FORBID_FILES != FORBID_FILES.sort
end

actual = Hash.new(0)
Dir.glob(File.join(RUST, "**", "*.rs")).sort.each do |path|
  count = File.binread(path).scan(/^\s*(?:pub\s+)?static\s+mut\b/).length
  next if count.zero?

  actual[path.delete_prefix("#{RUST}/")] = count
end

if actual != STATIC_MUT_COUNTS
  errors << "static mut boundary changed: #{table_diff(STATIC_MUT_COUNTS, actual)}"
end

rb_sys = Hash.new(0)
raising = Hash.new(0)
Dir.glob(File.join(RUST, "**", "*.rs")).sort.each do |path|
  relative = path.delete_prefix("#{RUST}/")
  next if relative.start_with?("bridge/")

  code = rust_code(path)
  count = code.scan(/(?<!magnus::)\brb_sys::/).length
  rb_sys[relative] = count unless count.zero?
  count = code.scan(RAISING_API).length
  raising[relative] = count unless count.zero?
end

if rb_sys != RB_SYS_COUNTS
  errors << "rb_sys:: outside bridge/ changed: #{table_diff(RB_SYS_COUNTS, rb_sys)}"
end
if raising != RAISING_COUNTS
  errors << "raising C API outside bridge/ changed: #{table_diff(RAISING_COUNTS, raising)}"
end

value_from_raw = Hash.new(0)
Dir.glob(File.join(RUST, "**", "*.rs")).sort.each do |path|
  relative = path.delete_prefix("#{RUST}/")
  next if relative.start_with?("bridge/")

  count = comments_removed(File.binread(path)).scan(VALUE_FROM_RAW).length
  value_from_raw[relative] = count unless count.zero?
end
if value_from_raw != VALUE_FROM_RAW_COUNTS
  errors << "Value::from_raw outside bridge/ changed: #{table_diff(VALUE_FROM_RAW_COUNTS, value_from_raw)}"
end

lexbor_abi = Hash.new(0)
Dir.glob(File.join(RUST, "**", "*.rs")).sort.each do |path|
  relative = path.delete_prefix("#{RUST}/")
  next if relative.start_with?("lexbor/")

  count = comments_removed(File.binread(path)).scan(LEXBOR_ABI).length
  lexbor_abi[relative] = count unless count.zero?
end
if lexbor_abi != LEXBOR_ABI_COUNTS
  errors << "Lexbor ABI names outside lexbor/ changed: #{table_diff(LEXBOR_ABI_COUNTS, lexbor_abi)}"
end

# A Lexbor function is DECLARED in one place, `lexbor/abi.rs` (bindgen's output
# plus the three exports no header declares). A second `extern "C"` declaration of the
# same symbol gives it a second Rust type, which nothing checks agree, and on
# macOS a declaration that matches no symbol is a NULL call rather than a link
# error. `post_parse.rs` re-declared four generated functions that way.
LEXBOR_DECL = /\bfn\s+(?:lxb|lexbor)_[A-Za-z0-9_]*\s*\([^)]*\)[^;{]*;/m
lexbor_decls = Hash.new(0)
Dir.glob(File.join(RUST, "**", "*.rs")).sort.each do |path|
  relative = path.delete_prefix("#{RUST}/")
  next if relative == "lexbor/abi.rs"

  count = comments_removed(File.binread(path)).scan(LEXBOR_DECL).length
  lexbor_decls[relative] = count unless count.zero?
end
unless lexbor_decls.empty?
  errors << "Lexbor functions declared outside lexbor/abi.rs: #{lexbor_decls.inspect}"
end

# Inside it, only the header-less exports are written by hand - everything else
# is generated, so its signature is Lexbor's. A new hand declaration of a
# header-declared function belongs in build.rs's allowlist instead; one with no
# header belongs in build.rs's UNDECLARED_EXPORTS as well as here.
LEXBOR_HAND_DECLS = 4
abi_decls = comments_removed(File.binread(File.join(RUST, "lexbor/abi.rs"))).scan(LEXBOR_DECL).length
if abi_decls != LEXBOR_HAND_DECLS
  errors << "lexbor/abi.rs hand-declares #{abi_decls} Lexbor functions (expected " \
            "#{LEXBOR_HAND_DECLS}): generate header-declared ones in build.rs; pin a " \
            "header-less one in build.rs's UNDECLARED_EXPORTS"
end

ruby_layer = Hash.new(0)
Dir.glob(File.join(RUST, "**", "*.rs")).sort.each do |path|
  relative = path.delete_prefix("#{RUST}/")
  next unless relative.start_with?(*ENGINE_DIRS)

  count = comments_removed(File.binread(path)).scan(RUBY_LAYER).length
  ruby_layer[relative] = count unless count.zero?
end
if ruby_layer != RUBY_LAYER_COUNTS
  errors << "Ruby-layer use inside an engine layer changed: #{table_diff(RUBY_LAYER_COUNTS, ruby_layer)}"
end

infallible = Hash.new(0)
Dir.glob(File.join(RUST, "**", "*.rs")).sort.each do |path|
  relative = path.delete_prefix("#{RUST}/")
  next unless relative.start_with?(*ENGINE_DIRS)
  next if relative.end_with?("selftest.rs", "verify.rs", "tests.rs")

  count = comments_removed(File.binread(path)).scan(INFALLIBLE_ALLOC).length
  infallible[relative] = count unless count.zero?
end
if infallible != INFALLIBLE_ALLOC_COUNTS
  errors << "infallible allocation inside an engine layer changed: " \
            "#{table_diff(INFALLIBLE_ALLOC_COUNTS, infallible)}"
end

# The bridge hands the glue primitives; the Ruby surface - defining a method,
# scanning its argument list - is the glue's. A method defined in bridge/ is
# how a feature's Ruby half ends up split across the two layers by where it
# happened to need an `unsafe`, rather than by what it does.
RUBY_SURFACE = /\b(?:define_(?:singleton_|private_|module_)?(?:method|function)|scan_args)\b/
ruby_surface = Hash.new(0)
Dir.glob(File.join(RUST, "bridge", "**", "*.rs")).sort.each do |path|
  relative = path.delete_prefix("#{RUST}/")
  count = comments_removed(File.binread(path)).scan(RUBY_SURFACE).length
  ruby_surface[relative] = count unless count.zero?
end
# Every Ruby method the glue registers runs its body inside
# `bridge::ruby::entry`, so a panic below it raises Makiri::InternalError rather
# than `fatal`. Wrapping by hand, method by method, left readers like
# `children`, `[]` and `NodeSet#each` out; so the gate finds EVERY `method!` /
# `function!` in the glue - wherever it is written, a registration table
# included - resolves it to its one definition, and fails unless that body is
# nothing but a `crate::bridge::ruby::entry(...)` call. What it cannot read, it
# refuses rather than skips. The exemptions are the hooks that exist to test the
# panic and allocation paths themselves.
ENTRY_EXEMPT = %w[
  init.rs:panic_probe init.rs:alloc_inject_p init.rs:alloc_inject init.rs:alloc_inject_calls
].freeze
ENTRY_FN = "crate::bridge::ruby::entry"

def module_dir(rel)
  base = File.basename(rel, ".rs")
  dir = File.dirname(rel)
  %w[mod lib].include?(base) || rel == "init.rs" ? (dir == "." ? "" : dir) : File.join(dir, base).sub(%r{\A\./}, "")
end

def module_file(rust, modpath)
  ["#{modpath}.rs", "#{modpath}/mod.rs"].find { |c| File.exist?(File.join(rust, c)) }
end

def fn_def_re(name) = /^\s*(?:pub(?:\([^)]*\))?\s+)?fn\s+#{Regexp.escape(name)}\b/

# The file a registered path names: `read::name` from `glue/html_node/mod.rs`,
# `crate::glue::node::f`, or a bare name defined here or brought in by `use`.
def resolve_fn(rust, rel, path, src)
  parts = path.split("::")
  name = parts.pop
  if parts.empty?
    imported = src.match?(/^\s*(?:pub\s+)?use\s+[\w:]+::(?:\{[^}]*\b#{Regexp.escape(name)}\b[^}]*\}|#{Regexp.escape(name)}\s*;)/m)
    local = src.match?(/\bfn\s+#{Regexp.escape(name)}\b/)
    # Both: which one the macro reaches is Rust's to decide, not this script's.
    return nil if imported && local
    return [rel, name] if local

    src.scan(/^\s*(?:pub\s+)?use\s+([\w:]+)::\{([^}]*)\}|^\s*(?:pub\s+)?use\s+([\w:]+)::(\w+)\s*;/m) do |p1, list, p2, single|
      if p1 && list.split(",").map(&:strip).include?(name)
        return resolve_fn(rust, rel, "#{p1}::#{name}", "")
      elsif p2 && single == name
        return resolve_fn(rust, rel, "#{p2}::#{name}", "")
      end
    end
    return nil
  end
  modpath = if parts.first == "crate"
              parts.drop(1).join("/")
            elsif parts.first == "super"
              File.join(File.dirname(module_dir(rel)), *parts.drop(1))
            else
              File.join(module_dir(rel), *parts).sub(%r{\A/}, "")
            end
  file = module_file(rust, modpath) or return nil
  [file, name]
end

# `src` with every comment, and the inside of every string and character
# literal, turned to spaces - line breaks kept, so offsets and line numbers
# still match. What is left is code only: a `method!(` written in a string or a
# comment is not seen, and a `"/*"` or `'('` in the code cannot throw a scan
# off. Handles nested block comments, raw strings (`r#"..."#`, `br"..."`) and
# tells a char literal from a lifetime.
def code_only(src)
  out = src.dup
  blank = ->(from, to) { (from...to).each { |k| out[k] = " " unless out[k] == "\n" } }
  i = 0
  n = src.length
  while i < n
    if src[i, 2] == "//"
      j = src.index("\n", i) || n
      blank.(i, j)
      i = j
    elsif src[i, 2] == "/*"
      depth = 0
      j = i
      while j < n
        if src[j, 2] == "/*"
          depth += 1
          j += 2
        elsif src[j, 2] == "*/"
          depth -= 1
          j += 2
          break if depth.zero?
        else
          j += 1
        end
      end
      blank.(i, j)
      i = j
    elsif (m = src[i..].match(/\A(?<![\w])b?r(#*)"/)) && (i.zero? || src[i - 1] !~ /\w/)
      close = "\"" + m[1]
      j = src.index(close, i + m[0].length) || n
      blank.(i + m[0].length, j)
      i = j + close.length
    elsif src[i] == '"'
      j = i + 1
      j += (src[j] == "\\" ? 2 : 1) while j < n && src[j] != '"'
      blank.(i + 1, j)
      i = j + 1
    elsif src[i] == "'" && (m = src[i..].match(/\A'(?:\\u\{[0-9a-fA-F]+\}|\\.|[^\\'\n])'/))
      blank.(i + 1, i + m[0].length - 1)
      i += m[0].length
    else
      i += 1
    end
  end
  out
end

# The index of the bracket closing the one at `open` in CODE (see
# `code_only`); nil when unbalanced.
def matching_close(code, open)
  depth = 0
  (open...code.length).each do |i|
    c = code[i]
    if "([{".include?(c)
      depth += 1
    elsif ")]}".include?(c)
      depth -= 1
      return i if depth.zero?
    end
  end
  nil
end

# Why the definition of `name` in `src` is not a method body wrapped in entry,
# or nil when it is: exactly one definition, whose whole body is one
# `crate::bridge::ruby::entry(...)` call.
def entry_violation(src, name)
  code = code_only(src)
  # Anywhere, not just at a line start, so a same-named fn tucked into a
  # one-line `mod` still counts as a second definition.
  defs = code.to_enum(:scan, /\bfn\s+#{Regexp.escape(name)}\b/).map { Regexp.last_match }
  return "no definition" if defs.empty?
  return "#{defs.length} definitions" if defs.length > 1

  open = code.index("{", defs.first.end(0)) or return "no body"
  close = matching_close(code, open) or return "unbalanced body"
  body = code[(open + 1)...close].strip
  # The call, with a closure or a bare function as its argument - both run
  # inside the catch. `entry(make(x))` would build its value before it.
  m = body.match(/\A#{Regexp.escape(ENTRY_FN)}\s*\(\s*(?:(?:move\s*)?\||[\w:]+\s*\))/)
  return "body is not #{ENTRY_FN}(|| ...) or #{ENTRY_FN}(a_fn)" unless m

  call_close = matching_close(body, body.index("(", ENTRY_FN.length))
  return nil if call_close && body[(call_close + 1)..].strip.empty?

  "code outside the #{ENTRY_FN}(...) call"
end

REGISTRATION = /\b(?:[\w:]*::)?(?:method|function)!\s*\(/

# The first argument of the macro call whose `(` is at `open` in `code`: the
# text up to its first top-level comma. nil when the call is not closed.
def first_argument(code, open)
  close = matching_close(code, open) or return nil
  depth = 0
  ((open + 1)...close).each do |i|
    c = code[i]
    depth += 1 if "([{".include?(c)
    depth -= 1 if ")]}".include?(c)
    return code[(open + 1)...i].strip if c == "," && depth.zero?
  end
  code[(open + 1)...close].strip
end

def unwrapped_entries(rust)
  missing = []
  seen = []
  files = Dir.glob(File.join(rust, "glue", "**", "*.rs")) + [File.join(rust, "init.rs")]
  files.sort.each do |path|
    rel = path.delete_prefix("#{rust}/")
    src = File.binread(path)
    code = code_only(src)
    # A renamed macro (`use magnus::method as m`) would register unseen.
    if code.match?(/\b(?:method|function)\s+as\s+\w+/)
      missing << "#{rel}: magnus's method!/function! imported under another name"
    end
    code.to_enum(:scan, REGISTRATION).each do
      m = Regexp.last_match
      fpath = first_argument(code, m.end(0) - 1)
      unless fpath&.match?(/\A[\w:]+\z/)
        missing << "#{rel}: registration of `#{fpath}` is not a function path"
        next
      end
      file, name = resolve_fn(rust, rel, fpath, code)
      if file.nil?
        missing << "#{rel}: cannot resolve #{fpath}"
        next
      end
      key = "#{file}:#{name}"
      seen << key
      next if ENTRY_EXEMPT.include?(key)

      why = entry_violation(File.binread(File.join(rust, file)), name)
      missing << "#{key} (#{why})" if why
    end
  end
  stale = ENTRY_EXEMPT - seen
  missing << "ENTRY_EXEMPT names what nothing registers: #{stale.inspect}" unless stale.empty?
  missing.uniq
end

unentered = unwrapped_entries(RUST)
unless unentered.empty?
  errors << "Ruby methods whose body is not wrapped in bridge::ruby::entry " \
            "(wrap it, or add it to ENTRY_EXEMPT with the reason): #{unentered.inspect}"
end

unless ruby_surface.empty?
  errors << "Ruby methods defined or argument lists scanned in bridge/ (they are the glue's): " \
            "#{ruby_surface.inspect}"
end

if FIX && !errors.empty?
  puts "unsafe-boundaries --fix: NOT rewritten, these record a decision rather than a count:"
  errors.each { |e| puts "  #{e}" }
end
abort "unsafe-boundaries: #{errors.join("\nunsafe-boundaries: ")}" unless errors.empty?

glue_xpath_unsafe = unsafe_actual
  .select { |f, _| f.start_with?("glue/", "xpath/") }
  .values.sum
puts "unsafe-boundaries: #{forbidding.length} forbid files; " \
     "#{unsafe_actual.values.sum} unsafe uses in #{unsafe_actual.length} islands " \
     "(glue+xpath #{glue_xpath_unsafe}); " \
     "#{actual.values.sum} reviewed static mut declarations; " \
     "#{rb_sys.values.sum} rb_sys:: and #{raising.values.sum} raising C calls outside bridge/; " \
     "#{value_from_raw.values.sum} Value::from_raw and " \
     "#{lexbor_abi.values.sum} Lexbor ABI names outside their layer and " \
     "#{lexbor_decls.values.sum} Lexbor declarations outside lexbor/abi.rs; " \
     "#{ruby_layer.values.sum} Ruby-layer uses inside the engine; " \
     "#{ruby_surface.values.sum} Ruby methods in bridge/; " \
     "every registered method but #{ENTRY_EXEMPT.length} exempt runs under entry"
