# frozen_string_literal: true

# Keep the parts of the extension that are meant to be ordinary Rust that way.
#
# The compiler holds the default: `lib.rs` denies `unsafe_code`, so a file that
# wants unsafe must say `#![allow(unsafe_code)]` and that line shows up in
# review. This script pins the result, the way it already pins
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
# Every source file the gates read. `lint_probe.rs` is left out: it is
# compiled only under `--cfg makiri_lint_probe`, by `rake dylint`, and holds
# on purpose the very uses the lints there must report.
# The binding layer is a crate of its own (`ext/makiri/rust/nupur`), read as
# if it were still `src/nupur/`: its files are keyed `nupur/<path>` below.
NUPUR = File.join(ROOT, "ext/makiri/rust/nupur/src")
RUST_FILES = (Dir.glob(File.join(RUST, "**", "*.rs")) + Dir.glob(File.join(NUPUR, "**", "*.rs"))).sort
                .reject { |p| p.end_with?("/lint_probe.rs") }.freeze

# A file's key in the tables: its path under `src/`, or `nupur/<path>` for
# the binding layer's.
def rust_rel(path)
  return "nupur/#{path.delete_prefix("#{NUPUR}/")}" if path.start_with?("#{NUPUR}/")

  path.delete_prefix("#{RUST}/")
end

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
  "falloc/mod.rs" => 1,
  "init.rs" => 1,
  "lexbor/abi.rs" => 4,
  "lexbor/adapter/arena_bytes.rs" => 14,
  "lexbor/adapter/html/attrs.rs" => 19,
  "lexbor/adapter/html/build.rs" => 14,
  "lexbor/adapter/html/mod.rs" => 66,
  "lexbor/adapter/html/serialize.rs" => 18,
  "lexbor/adapter/post_parse.rs" => 8,
  "lexbor/adapter/source_loc.rs" => 1,
  "lexbor/adapter/tree_guard.rs" => 7,
  "lexbor/chunks.rs" => 2,
  "lexbor/conformance_tests.rs" => 24,
  "lexbor/css_engine.rs" => 10,
  "lexbor/css_parser.rs" => 20,
  "lexbor/css_tokens.rs" => 10,
  "lexbor/fragment.rs" => 7,
  "lexbor/res_tests.rs" => 3,
  "lexbor/selector_port_tests.rs" => 1,
  "lexbor/selectors.rs" => 10,
  "lexbor/serialize.rs" => 1,
  "lexbor/stylesheet_diff.rs" => 5,
  "lexbor/tests.rs" => 3,
  "no_unwind.rs" => 2,
  "nupur/abi.rs" => 1,
  "nupur/describe.rs" => 4,
  "nupur/encoding.rs" => 9,
  "nupur/gvl.rs" => 3,
  "nupur/no_unwind.rs" => 2,
  "nupur/raw/mod.rs" => 75,
  "nupur/raw/protect.rs" => 14,
  "nupur/rb/class.rs" => 1,
  "nupur/rb/global.rs" => 2,
  "nupur/rb/method.rs" => 17,
  "nupur/rb/ruby.rs" => 5,
  "nupur/rb/value.rs" => 25,
  "nupur/stack.rs" => 1,
  "nupur/string.rs" => 7,
  "nupur/typed.rs" => 31,
  "utf8_copy.rs" => 1,
}.freeze

# Files whose safety is compiler-enforced. Checked by containment, so adding one
# needs no edit here, and kept SORTED (the gate fails otherwise) so `--fix` and a
# hand edit agree. `forbid` cannot be overridden by an inner `allow`, which is
# why a module root whose children need unsafe is not on this list.
#
# The subtree roots in FORBID_ROOTS make whole directories compiler-forbidden:
# `forbid` is inherited, so a new file under `glue/` or `xpath/` cannot slip in
# with an `allow`, however few its `unsafe`s.
FORBID_ROOTS = %w[bridge/mod.rs css/mod.rs glue/mod.rs css_match/mod.rs selectors/mod.rs xml/mod.rs
                  xpath/mod.rs].freeze

FORBID_FILES = %w[
  bridge/doc.rs bridge/dom.rs bridge/dom_error.rs
  bridge/fragment.rs bridge/html.rs bridge/mod.rs
  bridge/node_set.rs bridge/node_wrap.rs bridge/ruby.rs
  bridge/stack.rs bridge/string/build.rs bridge/string/encoding.rs
  bridge/string/source.rs bridge/string/view.rs bridge/wrapper/accounting.rs
  bridge/wrapper/document.rs bridge/wrapper/edit.rs bridge/wrapper/node.rs
  bridge/xml.rs bridge/xml_decode.rs bridge/xpath/context_object.rs
  bridge/xpath/mod.rs cbuf.rs child_index.rs
  conformance/mod.rs css/build.rs css/lower.rs
  css/mod.rs css_cache.rs css_match/mod.rs
  cutf8.rs cutf8/verify.rs doc_cell.rs
  dom_rules.rs engine_error.rs falloc/verify.rs
  glue/css.rs glue/html_doc.rs glue/html_node/css.rs
  glue/html_node/mutate.rs glue/html_node/read.rs glue/html_node/serialize.rs
  glue/mod.rs glue/node.rs glue/node_set.rs
  glue/query.rs glue/xml_doc.rs glue/xml_node/css.rs
  glue/xml_node/mod.rs glue/xml_node/mutate.rs glue/xml_node/ns.rs
  glue/xml_node/read.rs glue/xml_node/serialize.rs glue/xpath_context.rs
  gvl.rs html/dom/mod.rs html/encoding/mod.rs
  html/mod.rs html/tokenizer/mod.rs html/tree/mod.rs
  html_dom.rs lexbor/contains_guard.rs limits.rs
  node_type.rs nupur/args.rs nupur/build.rs
  nupur/int.rs nupur/msg.rs nupur/rb/convert.rs
  nupur/rb/error.rs nupur/table.rs ptr_table.rs
  rust_tests.rs selectors/mod.rs selectors/stylesheet.rs
  selectors/text.rs source_lines.rs stack.rs
  text.rs token.rs utf8_input.rs
  xml/arena.rs xml/attr_key.rs xml/chars/expand.rs
  xml/chars/mod.rs xml/css.rs xml/css_tests.rs
  xml/dom_name.rs xml/encoding_names.rs xml/encoding_sniff.rs
  xml/index.rs xml/mod.rs xml/model.rs
  xml/mutate/attr.rs xml/mutate/copy.rs xml/mutate/edit.rs
  xml/mutate/factory.rs xml/mutate/insert.rs xml/mutate/mod.rs
  xml/mutate/ns.rs xml/ns_scope.rs xml/qname.rs
  xml/selftest.rs xml/serialize/bindings.rs xml/serialize/c14n.rs
  xml/serialize/mod.rs xml/serialize/out.rs xml/serialize/xml.rs
  xml/tree/cursor.rs xml/tree/decl.rs xml/tree/dtd.rs
  xml/tree/mod.rs xml/tree/scope.rs xml/verify.rs
  xml/xpath.rs xpath/abi.rs xpath/ast.rs
  xpath/ast_ops.rs xpath/attr_pred.rs xpath/axis.rs
  xpath/ctx.rs xpath/dom.rs xpath/eval.rs
  xpath/funcs/css_hooks.rs xpath/funcs/ext.rs xpath/funcs/mod.rs
  xpath/lex.rs xpath/limits.rs xpath/mod.rs
  xpath/nodetest.rs xpath/number.rs xpath/order.rs
  xpath/parse.rs xpath/step_index.rs xpath/str_cache.rs
  xpath/tests.rs xpath/value.rs xpath/verify.rs
].freeze

UNSAFE_USE = /\bunsafe\s*(?:\{|fn\b|impl\b|trait\b|extern\b)/

# These are the remaining C/Ruby ABI globals.  Each entry is deliberately
# exact: adding another static mut must come with a dedicated boundary type or
# an explicit review of its synchronisation proof.
STATIC_MUT_COUNTS = {}.freeze

# Six text gates that stood here are `rake dylint`'s now
# (lints/makiri_lints/src/boundaries.rs), on resolved paths:
# `raw_ruby_outside_bridge` (`rb_sys::` outside bridge/, a raw `VALUE` outside
# the files that are the typed values), `raising_call_outside_protect` (whose
# `call` is private to `bridge::raw`, so it also pins where the raising calls
# are), `runs_callers_ruby`, `raw_node_crossing` and `ruby_surface_in_bridge`,
# each sanctioned site an `allow` with its reason where it is made.
# `Value::from_raw` is `pub(in crate::bridge)`, so the compiler holds that one.

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
# `bridge/` or `glue/`. `lexbor/fragment.rs` and `selectors/stylesheet.rs` once
# reached back up for a `VALUE`, the String borrow rules and `Makiri::Error` - an
# engine module holding the bridge's invariants, and the path a raise escaped
# along - so the table is empty and any such use fails.
# The crate-root modules the engine is built on sit outside those directories
# and are held to the same two rules. `html/` (the parser and the arena DOM
# an edit's closure works on), its facade and the child-index walk are
# engine too: an edit lends `&mut HtmlDocMut` to a closure that must run no
# Ruby, and this is what keeps what that closure calls Ruby-free.
ENGINE_DIRS = %w[lexbor/ xml/ xpath/ css/ selectors/ html/ html_dom.rs child_index.rs child_index/ falloc/ cbuf.rs cutf8.rs dom_rules.rs dom_rules/ limits.rs node_type.rs ptr_table.rs text.rs token.rs utf8_input.rs].freeze
RUBY_LAYER = /\bcrate::(?:bridge|glue|init)\b|\brb_sys::/
RUBY_LAYER_COUNTS = {}.freeze
# The rule for the rest of the engine, and INFALLIBLE_ALLOC's below, are
# `rake dylint`'s now (`ruby_layer_in_engine`, `infallible_allocation`):
# checked on the compiled code, by resolved path, where this read text. What
# stays here is `lexbor/` - the reference, which the lint's build (`ruby`)
# does not compile.
TEXT_CHECKED = %w[lexbor/].freeze

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
#
# It holds for the Ruby layer too (bridge/, glue/, init.rs): an exception
# message is written into a stack buffer (`crate::msg::Msg`, through
# `format_args!`) and made into the exception at once, so an error path - the
# one place a message used to be a `format!` - allocates nothing of Rust's,
# and a real OOM there is Ruby's `NoMemoryError` rather than an abort.
INFALLIBLE_ALLOC = /\.to_owned\(\)|\.to_vec\(\)|String::from\(|\bformat!\(|\.to_string\(\)|from_utf8_lossy|\.into_owned\(\)|\bvec!\[|CString::new/
# Where the gate looks: the engine, and the Ruby layer over it.
INFALLIBLE_ALLOC_DIRS = (ENGINE_DIRS + %w[bridge/ nupur/ glue/ init.rs lib.rs caught.rs css_cache.rs utf8_copy.rs
                                          compat.rs doc_cell.rs engine_error.rs gvl.rs source_lines.rs
                                          stack.rs]).freeze
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
# does NOT touch STATIC_MUT_COUNTS or LEXBOR_ABI_COUNTS: those record a
# boundary DECISION rather than a consequence of moving code, and a new
# process-wide mutable global or a new Lexbor ABI name above `lexbor/` is exactly the thing a person should have to think about. Rewriting
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

unless File.binread(File.join(RUST, "lib.rs")).include?("#![deny(unsafe_code)]")
  errors << "lib.rs: must retain #![deny(unsafe_code)] - it is what makes the rest a ratchet"
end

# The typed handles keep Lexbor pointers inside `lexbor::adapter`, so nothing
# above it - not the facades, not the glue - can read a Lexbor struct field
# through one. This is what closed the `compat_mode` leak: a field read spells
# no `lxb_*`/`Lxb*` name, so LEXBOR_ABI below cannot see it, and only the
# compiler can enforce this one.
#
# Stated as a rule rather than a list of signatures: every `fn` under
# `lexbor/adapter/` whose signature carries a `*mut Lxb*`/`*const Lxb*` or a
# `NonNull<Lxb...>` handle must be private, `pub(super)` or
# `pub(in crate::lexbor::adapter)`. A new accessor is caught without anyone
# remembering to list it. The exceptions are the doors a facade is meant to use,
# each with its reason.
# Not a Lexbor pointer, so the rule above cannot see it, but as dangerous: an
# unbounded-lifetime reinterpretation, whose one safe caller is
# `HtmlParsed::tag_bucket`. Held to the same visibility by name.
ADAPTER_ONLY_BY_NAME = %w[as_html_nodes_unchecked].freeze
RAW_POINTER_EXEMPT = {
  "as_lxb_mut" => "the one way a facade (selectors, serialize) gets the node pointer a Lexbor call takes; " \
                  "typed, so nothing casts c_void back",
}.freeze
ADAPTER_PRIVATE = /\A(?:pub\(super\)|pub\(in crate::lexbor::adapter(?:::\w+)*\))?\z/
Dir.glob(File.join(RUST, "lexbor/adapter/**/*.rs")).sort.each do |path|
  rel = rust_rel(path)
  File.binread(path).scan(/^[ \t]*((pub(?:\([^)]*\))?)?\s*(?:const\s+)?(?:unsafe\s+)?(?:extern\s+"C"\s+)?fn\s+(\w+)[^{;]*?)(?:\{|;|\bwhere\b)/m) do |sig, vis, name|
    next unless sig.match?(/\*(?:mut|const)\s+Lxb\w*|NonNull\s*<\s*Lxb\w*/) ||
                ADAPTER_ONLY_BY_NAME.include?(name)
    next if ADAPTER_PRIVATE.match?(vis.to_s)
    next if RAW_POINTER_EXEMPT.key?(name)
    errors << "#{rel}: `#{name}` hands out a Lexbor pointer or an unchecked view, so it must stay inside lexbor::adapter " \
              "(private, pub(super) or pub(in crate::lexbor::adapter)) or be listed in RAW_POINTER_EXEMPT with a reason"
  end
end

unsafe_actual = Hash.new(0)
forbidding = []
RUST_FILES.each do |path|
  relative = rust_rel(path)
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
RUST_FILES.each do |path|
  count = File.binread(path).scan(/^\s*(?:pub\s+)?static\s+mut\b/).length
  next if count.zero?

  actual[rust_rel(path)] = count
end

if actual != STATIC_MUT_COUNTS
  errors << "static mut boundary changed: #{table_diff(STATIC_MUT_COUNTS, actual)}"
end

lexbor_abi = Hash.new(0)
RUST_FILES.each do |path|
  relative = rust_rel(path)
  next if relative.start_with?("lexbor/")

  count = comments_removed(File.binread(path)).scan(LEXBOR_ABI).length
  lexbor_abi[relative] = count unless count.zero?
end
if lexbor_abi != LEXBOR_ABI_COUNTS
  errors << "Lexbor ABI names outside lexbor/ changed: #{table_diff(LEXBOR_ABI_COUNTS, lexbor_abi)}"
end

# A Lexbor function is DECLARED in one place, `lexbor/abi.rs` (bindgen's output
# plus the exports no header declares). A second `extern "C"` declaration of the
# same symbol gives it a second Rust type, which nothing checks agree, and on
# macOS a declaration that matches no symbol is a NULL call rather than a link
# error. `post_parse.rs` re-declared four generated functions that way.
LEXBOR_DECL = /\bfn\s+(?:lxb|lexbor)_[A-Za-z0-9_]*\s*\([^)]*\)[^;{]*;/m
lexbor_decls = Hash.new(0)
RUST_FILES.each do |path|
  relative = rust_rel(path)
  next if relative == "lexbor/abi.rs"

  count = comments_removed(File.binread(path)).scan(LEXBOR_DECL).length
  lexbor_decls[relative] = count unless count.zero?
end
unless lexbor_decls.empty?
  errors << "Lexbor functions declared outside lexbor/abi.rs: #{lexbor_decls.inspect}"
end

# Lexbor's CSS parsers are CALLED in one place each - the function that puts
# their input through `contains_guard` first (CLAUDE.md: the guard is not
# optional, and a new parse entry must go through it). A type alone cannot
# say so: the raw ABI is reachable crate-wide, so a second call site would
# compile and reach the parser unguarded. Pinned here instead. (Both are
# reference-only now: the shipping build parses with the `selectors` port.)
GUARDED_PARSES = {
  "lxb_css_selectors_parse" => "lexbor/css_engine.rs",   # SelectorParser::parse
  "lxb_css_stylesheet_parse" => "lexbor/stylesheet_diff.rs",  # lexbor_dump
}.freeze
guarded_calls = Hash.new { |h, k| h[k] = Hash.new(0) }
RUST_FILES.each do |path|
  relative = rust_rel(path)
  code = comments_removed(File.binread(path))
  GUARDED_PARSES.each_key do |name|
    count = code.scan(/\b#{name}\s*\(/).length
    guarded_calls[name][relative] = count unless count.zero?
  end
end
GUARDED_PARSES.each do |name, home|
  next if guarded_calls[name] == { home => 1 }

  errors << "#{name} must be called exactly once, in #{home} (behind contains_guard): " \
            "#{guarded_calls[name].inspect}"
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
RUST_FILES.each do |path|
  relative = rust_rel(path)
  next unless relative.start_with?(*TEXT_CHECKED)

  count = comments_removed(File.binread(path)).scan(RUBY_LAYER).length
  ruby_layer[relative] = count unless count.zero?
end
if ruby_layer != RUBY_LAYER_COUNTS
  errors << "Ruby-layer use inside an engine layer changed: #{table_diff(RUBY_LAYER_COUNTS, ruby_layer)}"
end

infallible = Hash.new(0)
RUST_FILES.each do |path|
  relative = rust_rel(path)
  next unless relative.start_with?(*TEXT_CHECKED)
  next if relative.end_with?("selftest.rs", "verify.rs", "tests.rs")
  # Built only for the tests and the `html_diff` / `html_xpath_diff` /
  # `selector_diff` / `stylesheet_diff` fuzz targets (`lexbor-reference`),
  # never in a build that ships - test files by another name.
  next if %w[lexbor/tree_diff.rs lexbor/xpath_diff.rs lexbor/selector_diff.rs
             lexbor/stylesheet_diff.rs].include?(relative)
  # A `#[cfg(test)]` module of its own (`pub mod conformance` is declared
  # under `#[cfg(test)]` in html/dom/mod.rs), a test file by another name.
  next if relative == "html/dom/conformance.rs"

  # An inline `#[cfg(test)] mod tests` at the end of a file is tests too.
  code = comments_removed(File.binread(path)).split(/^#\[cfg\(test\)\]\s*\n\s*mod tests\b/).first.to_s
  count = code.scan(INFALLIBLE_ALLOC).length
  infallible[relative] = count unless count.zero?
end
if infallible != INFALLIBLE_ALLOC_COUNTS
  errors << "infallible allocation in the engine or the Ruby layer changed: " \
            "#{table_diff(INFALLIBLE_ALLOC_COUNTS, infallible)}"
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
     "#{lexbor_abi.values.sum} Lexbor ABI names outside their layer and " \
     "#{lexbor_decls.values.sum} Lexbor declarations outside lexbor/abi.rs; " \
     "#{ruby_layer.values.sum} Ruby-layer uses inside lexbor/ " \
     "(the Ruby boundary's and the engine's layer rules: rake dylint)"
