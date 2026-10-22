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
  "bridge/alloc.rs" => 9,
  "bridge/doc.rs" => 1,
  "bridge/dom.rs" => 13,
  "bridge/fragment.rs" => 2,
  "bridge/gvl.rs" => 5,
  "bridge/html.rs" => 7,
  "bridge/node_set.rs" => 2,
  "bridge/node_wrap.rs" => 1,
  "bridge/raw.rs" => 44,
  "bridge/ruby.rs" => 19,
  "bridge/stack.rs" => 1,
  "bridge/string.rs" => 22,
  "bridge/typed.rs" => 23,
  "bridge/wrapper.rs" => 18,
  "bridge/xml.rs" => 11,
  "bridge/xml_decode.rs" => 7,
  "bridge/xpath/context_object.rs" => 3,
  "bridge/xpath/handler.rs" => 7,
  "bridge/xpath/mod.rs" => 7,
  "cbuf.rs" => 16,
  "cbuf/verify.rs" => 8,
  "falloc/cstr.rs" => 1,
  "falloc/cstr/verify.rs" => 1,
  "falloc/mod.rs" => 3,
  "gvl.rs" => 4,
  "lexbor/abi.rs" => 4,
  "lexbor/adapter/arena_bytes.rs" => 14,
  "lexbor/adapter/cross_import.rs" => 2,
  "lexbor/adapter/html/attrs.rs" => 25,
  "lexbor/adapter/html/build.rs" => 26,
  "lexbor/adapter/html/doc_mut.rs" => 6,
  "lexbor/adapter/html/mod.rs" => 67,
  "lexbor/adapter/html/mutate.rs" => 9,
  "lexbor/adapter/html/serialize.rs" => 18,
  "lexbor/adapter/post_parse.rs" => 9,
  "lexbor/adapter/source_loc.rs" => 1,
  "lexbor/adapter/text_index.rs" => 1,
  "lexbor/adapter/tree_guard.rs" => 7,
  "lexbor/arena_tests.rs" => 18,
  "lexbor/chunks.rs" => 2,
  "lexbor/conformance_tests.rs" => 24,
  "lexbor/css_engine.rs" => 10,
  "lexbor/css_parser.rs" => 19,
  "lexbor/css_tokens.rs" => 10,
  "lexbor/fragment.rs" => 10,
  "lexbor/res_tests.rs" => 3,
  "lexbor/selector_port_tests.rs" => 1,
  "lexbor/selectors.rs" => 9,
  "lexbor/serialize.rs" => 1,
  "lexbor/stylesheet_diff.rs" => 5,
  "lexbor/tests.rs" => 5,
  "lexbor/xpath.rs" => 9,
  "lexbor/xpath_diff.rs" => 2,
  "source_lines.rs" => 1,
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
FORBID_ROOTS = %w[css/mod.rs glue/mod.rs css_match/mod.rs selectors/mod.rs xml/mod.rs xpath/mod.rs].freeze

FORBID_FILES = %w[
  bridge/dom_error.rs child_index.rs conformance/mod.rs
  css/build.rs css/lower.rs css/mod.rs
  css_match/cache.rs css_match/mod.rs cutf8.rs
  cutf8/verify.rs dom_rules.rs engine_error.rs
  falloc/verify.rs glue/css.rs glue/html_doc.rs
  glue/html_node/css.rs glue/html_node/mutate.rs glue/html_node/read.rs
  glue/html_node/serialize.rs glue/mod.rs glue/node.rs
  glue/node_set.rs glue/query.rs glue/xml_doc.rs
  glue/xml_node/css.rs glue/xml_node/mod.rs glue/xml_node/mutate.rs
  glue/xml_node/ns.rs glue/xml_node/read.rs glue/xml_node/serialize.rs
  glue/xpath_context.rs html/dom/mod.rs html/encoding.rs
  html/mod.rs html/tokenizer/mod.rs html/tree/mod.rs
  html_dom.rs lexbor/adapter/dom_index.rs lexbor/contains_guard.rs
  limits.rs node_type.rs ptr_table.rs
  rust_tests.rs selectors/mod.rs selectors/stylesheet.rs
  selectors/text.rs stack.rs text.rs
  utf8_input.rs xml/arena.rs xml/attr_key.rs
  xml/chars/expand.rs xml/chars/mod.rs xml/css.rs
  xml/css_tests.rs xml/dom_name.rs xml/encoding_sniff.rs
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

# Ruby's C API outside `bridge/` is forbidden: the bridge is the only owner of
# raw VALUEs, typed data and the C calls that raise (glue/mod.rs), so a raise
# there becomes an `Err` before it can longjmp past a Rust destructor. This was
# a pinned count while the glue still reached across; the port is done, so the
# table is empty and any `rb_sys::` above the bridge fails. `magnus::rb_sys` is
# magnus's own module (a safe wrapper) and is not counted, and neither are
# comment lines.
RB_SYS_COUNTS = {}.freeze

# The Ruby C calls that can RAISE - start a `longjmp` that skips every Rust
# destructor between the raise and the nearest `rb_protect` - are named in ONE
# file, `bridge/raw.rs`, each wrapper stating what raises it. The names below
# may appear nowhere else: outside `bridge/` (as before), nor in any other
# bridge file, which reaches them through `raw`'s wrappers and so under a
# `protect` or from a frame that owns nothing. A new raising call is a new
# wrapper there, and this count moves with it on purpose.
RAISING_API = /\b(?:rb_raise|rb_exc_raise|rb_jump_tag|rb_check_typeddata|
  rb_String|rb_funcallv|rb_funcall|rb_obj_as_string|rb_respond_to|rb_check_frozen|
  rb_range_beg_len|rb_thread_check_ints|rb_intern3|rb_intern|rb_utf8_str_new|
  rb_enc_interned_str|rb_str_dup|rb_str_subseq|rb_enc_associate|rb_str_encode|
  rb_to_encoding|rb_enc_find|rb_data_typed_object_wrap|ruby_xmalloc|ruby_xrealloc2)\b/x
RAISING_COUNTS = { "bridge/raw.rs" => 22 }.freeze

# `Value::from_raw` outside `bridge/` is the other half of the raw Ruby
# boundary: a `VALUE` the glue already holds, turned back into a `Value`. The
# bridge's `value()` accessor owns that conversion now, so this is zero and any
# direct `Value::from_raw` above the bridge fails.
VALUE_FROM_RAW = /\bValue::from_raw\b/
VALUE_FROM_RAW_COUNTS = {}.freeze

# Raw HTML node crossings in `bridge/`: `RawNode::from(<typed>)` turns a
# borrowed handle into a lifetime-erased pointer, `RawNode::from_ptr` turns an
# arbitrary pointer into one, and `HtmlNodeKey::raw_node()` takes the raw handle
# back out of a long-lived key. They are how a raw node pointer crosses or
# escapes the bridge, and the `unsafe` count table cannot see a SAFE one - a
# `RawNode::from(node)` returned out of a closure, or a new place that reads a
# key, is exactly what the handle API exists to keep countable. Pinned per file,
# exact: a new crossing is a deliberate edit, and the checked entry
# (`with_html_node` / `HtmlParsed::mint_key` / `HtmlParsed::resolve`) is what to
# reach for instead.
RAW_NODE_CROSSING =
  /\bRawNode::from\s*\(|\bRawNode::from_ptr\b|\.raw_node\(\)|\bHtmlNodeKey::raw_node\b/
RAW_NODE_CROSSING_COUNTS = {
  "bridge/doc.rs" => 1,
  # `node_ref`: a checked key as a reference INTO its document, for an edit
  # that holds that document by `&mut` (`HtmlDocMut`) and reads the node
  # through it - the one exit, so an edit never holds a second handle. The
  # fragment staging that used to cross three times now passes references
  # inside the edit and crosses none.
  # 8th: an Attr's owner, read as the wrapper cache's identity key for the
  # owner-frozen check (`check_attr_owner_frozen`); it does not escape.
  # 9th/10th: `TokenTree for HtmlTree` (`child_count` / `child_at`), a node's pointer as
  # the child-position memo's token and back. `child_index::ChildPositionMemo`
  # answers only for the `tree_version` it was filled under, so a token read
  # back is still that child; the node is never handed out raw, only wrapped.
  # `bridge/dom.rs`: the two steps whose spelling differs between the HTML
  # DOMs (`crate::html_dom`) - a stored node word back into a node
  # (`node_of_word`, Lexbor's `from_ptr`), and a Document's own node
  # (`document_node`). Both were spelled twice, in `html.rs` and
  # `wrapper.rs`, which each lost two.
  "bridge/dom.rs" => 1,
  "bridge/html.rs" => 7,
  "bridge/wrapper.rs" => 1,
}.freeze

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
#
# Empty: the `selectors`-crate exploration that once held an
# accepted exception here was rejected in favour of porting Lexbor's own
# `lxb_selectors_*` state machine instead - `css_match/`, which
# allocates only through `falloc` like the rest of the engine (and `rake oom`
# sweeps it).
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
  rel = path.delete_prefix("#{RUST}/")
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
  code = rust_code(path)
  # A name in a block comment is documentation, as for the Lexbor names.
  count = comments_removed(File.binread(path)).scan(RAISING_API).length
  raising[relative] = count unless count.zero?
  next if relative.start_with?("bridge/")

  count = code.scan(/(?<!magnus::)\brb_sys::/).length
  rb_sys[relative] = count unless count.zero?
end

if rb_sys != RB_SYS_COUNTS
  errors << "rb_sys:: outside bridge/ changed: #{table_diff(RB_SYS_COUNTS, rb_sys)}"
end
if raising != RAISING_COUNTS
  errors << "raising C API outside bridge/raw.rs changed: #{table_diff(RAISING_COUNTS, raising)}"
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

raw_node_crossing = Hash.new(0)
Dir.glob(File.join(RUST, "bridge", "**", "*.rs")).sort.each do |path|
  relative = path.delete_prefix("#{RUST}/")
  count = comments_removed(File.binread(path)).scan(RAW_NODE_CROSSING).length
  raw_node_crossing[relative] = count unless count.zero?
end
if raw_node_crossing != RAW_NODE_CROSSING_COUNTS
  errors << "raw-node crossings in bridge/ changed: " \
            "#{table_diff(RAW_NODE_CROSSING_COUNTS, raw_node_crossing)} " \
            "(reach for with_html_node / HtmlParsed::mint_key / HtmlParsed::resolve, or record the crossing here)"
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
# plus the exports no header declares). A second `extern "C"` declaration of the
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
Dir.glob(File.join(RUST, "**", "*.rs")).sort.each do |path|
  relative = path.delete_prefix("#{RUST}/")
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

# magnus's `RHash::foreach` runs its closure under magnus's own `protect`, below
# `rb_hash_foreach`'s C frames, so a panic in it aborts the process instead of
# raising. `glue::hash::hash_foreach` latches it; every Hash walk goes there.
HASH_FOREACH = /\.foreach\(/
hash_walks = Hash.new(0)
Dir.glob(File.join(RUST, "**", "*.rs")).sort.each do |path|
  relative = path.delete_prefix("#{RUST}/")
  next if relative == "glue/hash.rs"

  count = comments_removed(File.binread(path)).scan(HASH_FOREACH).length
  hash_walks[relative] = count unless count.zero?
end
unless hash_walks.empty?
  errors << "RHash#foreach outside glue::hash::hash_foreach (a panic there aborts): #{hash_walks.inspect}"
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
      # A generic function registered per representation
      # (`child_count::<HtmlSelf>`) is one definition: resolve it without its
      # type arguments.
      fpath = fpath&.sub(/::<[\w:, ]+>\z/, "")
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

# A frame in `glue/` or `bridge/` that owns a copy of an argument (a checked
# view, the HTML source, an owned buffer) must not reach a call that can raise
# while the copy is live: a raise is a longjmp past the frame, its `Drop` never runs, and the
# copy leaks - the shape fixed one site at a time in build_fragment, css, the
# factories and `[]` before this check existed. Drop the copy first, or make
# the call under `bridge::ruby::protected`, where a raise comes back as `Err`.
#
# What raises is worked out, not listed: `bridge/raw.rs`'s wrappers of the
# RAISING_API calls and magnus's allocating constructors are the seeds, and a
# function in `bridge/` or `glue/` that calls one outside a protect raises
# too, to a fixed point. A call is matched by name, a method call (`.f(`) to
# methods and a plain or path call to free functions, and a name defined more
# than once in the crate is not followed (it would make every `new` and `get` raise); the
# wrappers below are followed by hand for that reason. A copy is a `let`
# whose right side calls a function returning one of COPY_TYPES (or a closure
# that does), or a parameter that takes one by value; it is live to the end
# of its block, to its `drop(name)`, or to the call it is handed to by value -
# that callee owns it then, and is checked through its own parameter.
COPY_TYPES = /\b(?:RubyText|RubyData|RubyStr|HtmlSource|OwnedBuf)\b/
MAGNUS_ALLOCATING = /\.(?:str_new|enc_str_new|str_from_slice|str_buf_new|str_with_capacity|
  ary_new|ary_new_capa|ary_new_from_values|ary_from_iter|ary_from_vec|hash_new|hash_new_capa|
  enumeratorize|obj_alloc|float_from_f64|integer_from_i64|integer_from_u64|sym_new)\s*\(/x
PROTECTING = /\b(?:protected|protect_value|protect)\s*\(/
# Raise only when their contract is broken: `check_typeddata` on a VALUE the
# caller built as that type.
CANNOT_RAISE = %w[typed_data_known typed_data_unprotected].freeze
# Raising, but defined under a name used more than once (`TypedType::wrap`).
RAISING_BY_HAND = [["wrap_cached", false]].freeze
# Methods of ours named like a std method (`NodeSet::filter`): a `.filter(`
# is far more often an Option's or an iterator's, so these are not followed.
STD_METHOD_NAMES = %w[at concat copy filter get push reserve slice split_off take union value].freeze

# Every `fn` in `files`: [rel, name, method?, body code, body offset, file code].
def fn_bodies(rust, files)
  files.flat_map do |path|
    rel = path.delete_prefix("#{rust}/")
    code = code_only(File.binread(path))
    code.to_enum(:scan, /\bfn\s+(\w+)/).filter_map do
      m = Regexp.last_match
      open = code.index(/[{;]/, m.end(0))
      next if open.nil? || code[open] == ";"

      close = matching_close(code, open) or next
      sig = code[m.end(0)...open]
      params = sig[/\((.*)/m, 1].to_s
      [rel, m[1], params.match?(/\A\s*&?\s*(?:'\w+\s+)?(?:mut\s+)?self\b/), code[(open + 1)...close],
       open + 1, code, sig]
    end
  end
end

def protected_spans(body)
  body.to_enum(:scan, PROTECTING).filter_map do
    open = Regexp.last_match.end(0) - 1
    close = matching_close(body, open)
    close && (open..close)
  end
end

# Every call site in `body` outside a protect, as [offset, key, label]: key is
# the [name, method?] it calls, or nil for a magnus allocating constructor,
# which raises whatever `raising` holds. Worked out once per body, so the
# fixed point below only filters.
def call_sites(body)
  calls = body.to_enum(:scan, MAGNUS_ALLOCATING).map do
    [Regexp.last_match.begin(0), nil, Regexp.last_match[0].strip]
  end
  body.to_enum(:scan, /(\.\s*)?(\b[A-Z]\w*\s*(?:<[^>]*>)?\s*::\s*)?\b(\w+)\s*(?:::<[^>]*>)?\s*\(/).each do
    m = Regexp.last_match
    # `Type::f(` names an associated function, not a free one of that name.
    next if m[2]

    calls << [m.begin(3), [m[3], !m[1].nil?], m[3]]
  end
  # A free function handed over as a value, `.map(utf8_str)`, is called too.
  body.to_enum(:scan, /(?<![.\w])((?:\w+::)*)(\w+)(?=\s*[),])/).each do
    m = Regexp.last_match
    next if m[1].match?(/(?:\A|::)[A-Z]\w*::\z/) # `Val::string`, not `string`

    calls << [m.begin(2), [m[2], false], m[2]]
  end
  spans = protected_spans(body)
  calls.reject { |at, _, _| spans.any? { |s| s.cover?(at) } }
end

# The calls in `sites` (from `call_sites`) to a [name, method?] in `raising`,
# as [offset, label].
def raising_in(sites, raising)
  sites.filter_map { |at, key, label| [at, label] if key.nil? || raising.include?(key) }
end

# The calls in `body` to a [name, method?] in `raising`, outside a protect.
def raising_calls(body, raising)
  raising_in(call_sites(body), raising)
end

def copies_across_raises(rust)
  fns = fn_bodies(rust, Dir.glob(File.join(rust, "{bridge,glue}", "**", "*.rs")))
  # Counted over the whole crate: a bridge method named like an engine one
  # (`Bridge::resolve`, `HtmlParsed::resolve`) is not the one a call reaches.
  defined = Dir.glob(File.join(rust, "**", "*.rs")).flat_map do |path|
    File.binread(path).scan(/\bfn\s+(\w+)\s*(?:<[^>{]*>)?\s*\((\s*&?\s*(?:'\w+\s+)?(?:mut\s+)?self\b)?/)
        .map { |n, recv| [n, !recv.nil?] }
  end.tally
  raising = fns.select { |rel, _, _, body| rel == "bridge/raw.rs" && body.match?(RAISING_API) }
               .to_set { |f| [f[1], false] }
  raising.merge(RAISING_BY_HAND)
  # `use path::f as g`: a call to `g` is a call to `f`.
  aliases = Dir.glob(File.join(rust, "{bridge,glue}", "**", "*.rs")).flat_map do |path|
    code_only(File.binread(path)).scan(/\b(\w+)\s+as\s+(\w+)\b/)
  end
  sites = fns.map { |f| call_sites(f[3]) }
  loop do
    aliases.each { |from, to| raising << [to, false] if raising.include?([from, false]) }
    followed = raising.select { |k| defined[k].to_i <= 1 && !(k[1] && STD_METHOD_NAMES.include?(k[0])) }.to_set
    grown = fns.zip(sites).filter_map do |(rel, name, method), body_sites|
      k = [name, method]
      next if rel == "bridge/raw.rs" || raising.include?(k) || CANNOT_RAISE.include?(name)

      k if raising_in(body_sites, followed).any?
    end
    break if grown.empty?

    raising.merge(grown)
  end
  aliases.each { |from, to| raising << [to, false] if raising.include?([from, false]) }
  followed = raising.select { |k| defined[k].to_i <= 1 && !(k[1] && STD_METHOD_NAMES.include?(k[0])) }.to_set
  copy_fns = fns.select { |f| f[6][/->(.*)\z/m, 1]&.match?(COPY_TYPES) }.to_set { |f| f[1] }

  found = []
  fns.select { |f| f[0].start_with?("glue/", "bridge/") && f[0] != "bridge/raw.rs" }.each do |rel, name, _, body, base, code, sig|
    # `var`, a copy from `from`, live to `block_end` unless dropped or moved.
    check = lambda do |var, from, block_end|
      dropped = body.index(/\bdrop\(\s*(?:\([\w\s,]*)?\b#{var}\b[\w\s,]*\)?\s*\)/, from)
      # Handed to a call by value (`f(x)`, `f(a, x)`): the callee owns it.
      moved = body.index(/[(,]\s*#{var}\s*[,)]/, from)
      live_end = [block_end, dropped || block_end, moved || block_end].min
      seg = body[from...live_end]
      # The call it is handed to by value owns it: not live across that one.
      into = moved && (open = body.rindex("(", moved)) && (open - from)
      raising_calls(seg, followed).each do |at, callee|
        next if into && at < into && seg.index("(", at) == into
        line = code[0...(base + from + at)].count("\n") + 1
        found << "#{rel}:#{line} #{name}: `#{var}` (a copy) is live across #{callee}"
      end
    end
    # A copy taken by value is this frame's from the start: the caller that
    # handed it over is not checked past that call.
    sig[/\((.*)\)/m, 1].to_s.scan(/(?:\A|,)\s*(?:mut\s+)?(\w+)\s*:\s*(?:Option<\s*)?(?:\w+::)*(?:#{COPY_TYPES.source})/)
                       .each { |(var)| check.(var, 0, body.length) }
    local_copy_fns = copy_fns.dup
    body.to_enum(:scan, /\blet\s+(?:mut\s+)?(?:Some\(\s*)?(?:mut\s+)?(\(?[\w\s,]+\)?)\s*(?::[^=;]*)?=(?!=)/).each do
      m = Regexp.last_match
      depth = 0
      stmt_end = (m.end(0)...body.length).find do |i|
        c = body[i]
        depth += 1 if "([{".include?(c)
        depth -= 1 if ")]}".include?(c)
        c == ";" && depth.zero?
      end or next
      rhs = body[m.end(0)...stmt_end]
      # A block's value is not the copy made inside it; its own lets are seen.
      next if rhs.lstrip.start_with?("{")
      next unless local_copy_fns.any? { |f| rhs.match?(/\b#{f}\s*\(/) }

      names = m[1].scan(/\w+/) - %w[mut]
      if rhs.lstrip.match?(/\A(?:move\s*)?\|/)
        local_copy_fns.merge(names)
        next
      end
      # The innermost block holding the `let`.
      opens = (0...m.begin(0)).select { |i| body[i] == "{" }.reverse
      block_end = opens.lazy.map { |o| matching_close(body, o) }.find { |c| c && c > m.begin(0) } || body.length
      names.each { |var| check.(var, stmt_end, block_end) }
    end
  end
  found.uniq
end

copy_leaks = copies_across_raises(RUST)
unless copy_leaks.empty?
  errors << "a frame holds a copy across a call that can raise (drop it first, or " \
            "make the call under bridge::ruby::protected): #{copy_leaks.inspect}"
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
     "#{rb_sys.values.sum} rb_sys:: outside bridge/ and #{raising.values.sum} raising C calls in bridge/raw.rs; " \
     "#{value_from_raw.values.sum} Value::from_raw and " \
     "#{lexbor_abi.values.sum} Lexbor ABI names outside their layer and " \
     "#{lexbor_decls.values.sum} Lexbor declarations outside lexbor/abi.rs; " \
     "#{raw_node_crossing.values.sum} raw-node crossings in bridge/; " \
     "#{ruby_layer.values.sum} Ruby-layer uses inside the engine; " \
     "#{ruby_surface.values.sum} Ruby methods in bridge/; " \
     "every registered method but #{ENTRY_EXEMPT.length} exempt runs under entry; " \
     "no copy is live across a raise in glue/ or bridge/"
