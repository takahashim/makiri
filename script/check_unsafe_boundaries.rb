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
  "bridge/gvl.rs" => 3,
  "bridge/node_set.rs" => 3,
  "bridge/node_wrap.rs" => 1,
  "bridge/raw.rs" => 92,
  "bridge/rb/class.rs" => 2,
  "bridge/rb/method.rs" => 11,
  "bridge/rb/ruby.rs" => 5,
  "bridge/rb/value.rs" => 16,
  "bridge/ruby.rs" => 13,
  "bridge/stack.rs" => 1,
  "bridge/string/build.rs" => 2,
  "bridge/string/encoding.rs" => 10,
  "bridge/string/view.rs" => 6,
  "bridge/typed.rs" => 27,
  "bridge/wrapper/document.rs" => 1,
  "bridge/wrapper/node.rs" => 1,
  "bridge/xml_decode.rs" => 3,
  "bridge/xpath/context_object.rs" => 2,
  "bridge/xpath/mod.rs" => 3,
  "falloc/mod.rs" => 1,
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
  bridge/build.rs bridge/doc.rs bridge/dom.rs
  bridge/dom_error.rs bridge/fragment.rs bridge/string/source.rs
  bridge/wrapper/accounting.rs bridge/wrapper/edit.rs cbuf.rs
  child_index.rs conformance/mod.rs css/build.rs
  css/lower.rs css/mod.rs css_cache.rs
  css_match/mod.rs cutf8.rs cutf8/verify.rs
  doc_cell.rs dom_rules.rs engine_error.rs
  falloc/verify.rs glue/css.rs glue/html_doc.rs
  glue/html_node/css.rs glue/html_node/mutate.rs glue/html_node/read.rs
  glue/html_node/serialize.rs glue/mod.rs glue/node.rs
  glue/node_set.rs glue/query.rs glue/xml_doc.rs
  glue/xml_node/css.rs glue/xml_node/mod.rs glue/xml_node/mutate.rs
  glue/xml_node/ns.rs glue/xml_node/read.rs glue/xml_node/serialize.rs
  glue/xpath_context.rs gvl.rs html/dom/mod.rs
  html/encoding/mod.rs html/mod.rs html/tokenizer/mod.rs
  html/tree/mod.rs html_dom.rs lexbor/contains_guard.rs
  limits.rs msg.rs node_type.rs
  ptr_table.rs rust_tests.rs selectors/mod.rs
  selectors/stylesheet.rs selectors/text.rs source_lines.rs
  stack.rs text.rs token.rs
  utf8_input.rs xml/arena.rs xml/attr_key.rs
  xml/chars/expand.rs xml/chars/mod.rs xml/css.rs
  xml/css_tests.rs xml/dom_name.rs xml/encoding_names.rs
  xml/encoding_sniff.rs xml/index.rs xml/mod.rs
  xml/model.rs xml/mutate/attr.rs xml/mutate/copy.rs
  xml/mutate/edit.rs xml/mutate/factory.rs xml/mutate/insert.rs
  xml/mutate/mod.rs xml/mutate/ns.rs xml/ns_scope.rs
  xml/qname.rs xml/selftest.rs xml/serialize/bindings.rs
  xml/serialize/c14n.rs xml/serialize/mod.rs xml/serialize/out.rs
  xml/serialize/xml.rs xml/tree/cursor.rs xml/tree/decl.rs
  xml/tree/dtd.rs xml/tree/mod.rs xml/tree/scope.rs
  xml/verify.rs xml/xpath.rs xpath/abi.rs
  xpath/ast.rs xpath/ast_ops.rs xpath/attr_pred.rs
  xpath/axis.rs xpath/ctx.rs xpath/dom.rs
  xpath/eval.rs xpath/funcs/css_hooks.rs xpath/funcs/ext.rs
  xpath/funcs/mod.rs xpath/lex.rs xpath/limits.rs
  xpath/mod.rs xpath/nodetest.rs xpath/number.rs
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
# table is empty and any `rb_sys::` above the bridge fails. Comment lines are
# not counted.
RB_SYS_COUNTS = {}.freeze

# The Ruby C calls that can RAISE - start a `longjmp` that skips every Rust
# destructor between the raise and the nearest `rb_protect` - are named in ONE
# file, `bridge/raw.rs`, each wrapper stating what raises it. The names below
# may appear nowhere else: outside `bridge/` (as before), nor in any other
# bridge file, which reaches them through `raw`'s wrappers and so under a
# `protect` or from a frame that owns nothing. A new raising call is a new
# wrapper there, and this count moves with it on purpose.
RAISING_API = /\b(?:rb_raise|rb_exc_raise|rb_jump_tag|rb_check_typeddata|
  rb_sym2str|rb_check_frozen|
  rb_range_beg_len|rb_thread_check_ints|rb_intern3|rb_intern|rb_utf8_str_new|
  rb_enc_interned_str|rb_str_dup|rb_str_subseq|rb_enc_associate|rb_str_encode|
  rb_to_encoding|rb_enc_find_index|rb_data_typed_object_wrap|rb_data_typed_object_zalloc|
  ruby_xmalloc|ruby_xrealloc2|
  rb_ary_new_capa|rb_ary_push|rb_hash_new|rb_hash_aset|rb_ll2inum|rb_ull2inum|rb_float_new|
  rb_id2sym|rb_str_cat|rb_enumeratorize_with_size|rb_memerror|rb_hash_foreach|rb_yield|
  rb_define_class_under|rb_define_module_under|rb_define_module|rb_const_set|rb_const_get|
  rb_include_module|rb_define_method|rb_define_private_method|rb_define_singleton_method|
  rb_define_module_function|rb_exc_new_str|rb_exc_fatal|rb_big2ll|rb_big2ull|rb_hash_lookup2|
  rb_gc_register_mark_object|rb_module_new|rb_range_values|rb_funcallv|rb_ivar_set)\b/x
RAISING_COUNTS = { "bridge/raw.rs" => 55 }.freeze # with `rb_memerror` (the raise sweep's injected raise) and `rb_jump_tag` (`jump_error`); 55 with the node table (`rb_hash_lookup2`, `rb_hash_aset`) and the hidden ivars (`rb_ivar_set`)

# Makiri runs none of its caller's Ruby code: no argument is converted with its
# `to_s`/`to_str`, no method of a caller's object is called, and XPath has no
# custom-function handler. The C calls and methods that would run such code are
# counted here, across the crate, and two are allowed: `NodeSet#each`'s
# `yield` (the block is the point of the method, and no guard or copy is held
# across it) and `XML::Namespace.new` (Makiri's own Data class, called with no
# borrow of the arena held).
USER_CODE_API = /\b(?:rb_String|rb_funcall\w*|rb_obj_as_string|rb_respond_to|rb_yield\w*|
  rb_block_call|rb_call_super|rb_eval_string\w*|rb_str_to_str|rb_string_value\w*|
  rb_check_string_type|rb_convert_type|rb_check_convert_type)\b|
  \.(?:funcall\w*|respond_to|to_r_string|block_call|yield_\w+|yield_splat|call_method)\(|
  raw::yield_value\(/x
# `rb_yield` and `rb_funcallv` are `bridge/raw.rs`'s; NodeSet#each yields, and
# `XML::Namespace.new` is `Module::call_method`'s one use.
USER_CODE_COUNTS = { "bridge/raw.rs" => 2, "glue/node_set.rs" => 1, "glue/xml_node/ns.rs" => 1 }.freeze

# A raw `VALUE` (`rb_sys::VALUE`, an integer to Rust) is a Ruby value no type
# keeps honest: no lint can tell it from a `u64` in a `Vec` or a struct field,
# where the GC would not see it. So it is spelled only where it must be:
# `bridge/raw.rs` (the C calls), `bridge/typed.rs` (TypedData, `Held`, the GC
# callbacks) and the `bridge::rb` files that ARE the typed values (`value.rs`,
# `ruby.rs`, `method.rs`'s trampoline pointer). Everything else holds those
# typed values, which `clippy.toml` and the dylint lint can see. The reference-only `lexbor/` is
# not counted. Pinned per file, empty: a new one is a decision.
VALUE_ALLOWED = %w[bridge/raw.rs bridge/typed.rs bridge/rb/value.rs bridge/rb/ruby.rs bridge/rb/method.rs].freeze
VALUE_COUNTS = {}.freeze

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
  # 11th: a Document's own node, minted as its key (`html_node_key`); and in
  # `wrapper/node.rs` the same node as its word (`node_raw`). `bridge/dom.rs`
  # spelled it once (`document_node`) until the document became a guard's,
  # read where it is used.
  # 12th: `HtmlView::set`, the nodes a read lists as the words a NodeSet holds,
  # pushed under `protect` while the read is held; the glue's `set_of` did it
  # before the read was a guard.
  # 13th: `HtmlView::wrap`, the node a read finds - with its type, so the
  # wrap after the read need not read again.
  "bridge/html.rs" => 10,
  "bridge/wrapper/node.rs" => 2,
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
RUBY_LAYER = /\bcrate::(?:bridge|glue|init)\b|\brb_sys::/
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
#
# It holds for the Ruby layer too (bridge/, glue/, init.rs): an exception
# message is written into a stack buffer (`crate::msg::Msg`, through
# `format_args!`) and made into the exception at once, so an error path - the
# one place a message used to be a `format!` - allocates nothing of Rust's,
# and a real OOM there is Ruby's `NoMemoryError` rather than an abort.
INFALLIBLE_ALLOC = /\.to_owned\(\)|\.to_vec\(\)|String::from\(|\bformat!\(|\.to_string\(\)|from_utf8_lossy|\.into_owned\(\)|\bvec!\[|CString::new/
# Where the gate looks: the engine, and the Ruby layer over it.
INFALLIBLE_ALLOC_DIRS = (ENGINE_DIRS + %w[bridge/ glue/ init.rs lib.rs msg.rs caught.rs css_cache.rs
                                          compat.rs doc_cell.rs engine_error.rs gvl.rs source_lines.rs
                                          stack.rs]).freeze
#
# The one site is a test hook's: `Makiri.__panic(1)`'s out-of-bounds index.
INFALLIBLE_ALLOC_COUNTS = { "init.rs" => 1 }.freeze

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

  count = code.scan(/\brb_sys::/).length
  rb_sys[relative] = count unless count.zero?
end

if rb_sys != RB_SYS_COUNTS
  errors << "rb_sys:: outside bridge/ changed: #{table_diff(RB_SYS_COUNTS, rb_sys)}"
end
if raising != RAISING_COUNTS
  errors << "raising C API outside bridge/raw.rs changed: #{table_diff(RAISING_COUNTS, raising)}"
end

user_code = Hash.new(0)
Dir.glob(File.join(RUST, "**", "*.rs")).sort.each do |path|
  relative = path.delete_prefix("#{RUST}/")
  next if relative.start_with?("lexbor/")

  count = rust_code(path).scan(USER_CODE_API).length
  user_code[relative] = count unless count.zero?
end
if user_code != USER_CODE_COUNTS
  errors << "calls that run the caller's Ruby code changed: #{table_diff(USER_CODE_COUNTS, user_code)}"
end

raw_values = Hash.new(0)
Dir.glob(File.join(RUST, "**", "*.rs")).sort.each do |path|
  relative = path.delete_prefix("#{RUST}/")
  next if relative.start_with?("lexbor/") || VALUE_ALLOWED.include?(relative)

  count = comments_removed(File.binread(path)).scan(/\bVALUE\b/).length
  raw_values[relative] = count unless count.zero?
end
if raw_values != VALUE_COUNTS
  errors << "raw VALUE outside bridge/raw.rs and bridge/typed.rs changed: #{table_diff(VALUE_COUNTS, raw_values)}"
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

# A Hash is walked by `rb_hash_foreach` calling back into Rust, below its C
# frames, so a panic there would abort the process. `bridge::raw::hash_foreach`
# latches it, and every Hash walk goes through `glue::hash::hash_foreach` over
# it (magnus's `RHash::foreach`, which had no latch, was the reason for this
# check).
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
  next unless relative.start_with?(*INFALLIBLE_ALLOC_DIRS)
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

# The bridge hands the glue primitives; the Ruby surface - defining a method,
# scanning its argument list - is the glue's. A method defined in bridge/ is
# how a feature's Ruby half ends up split across the two layers by where it
# happened to need an `unsafe`, rather than by what it does.
RUBY_SURFACE = /\b(?:define_(?:singleton_|private_|module_)?(?:method|function)|scan_args)\b/
# The layer that defines methods at all: `bridge::rb::class` is the definition
# functions, `bridge::raw` the C calls under them.
RUBY_SURFACE_HOME = %w[bridge/rb/class.rs bridge/raw.rs].freeze
ruby_surface = Hash.new(0)
Dir.glob(File.join(RUST, "bridge", "**", "*.rs")).sort.each do |path|
  relative = path.delete_prefix("#{RUST}/")
  next if RUBY_SURFACE_HOME.include?(relative)
  count = comments_removed(File.binread(path)).scan(RUBY_SURFACE).length
  ruby_surface[relative] = count unless count.zero?
end
# Every Ruby method runs its body inside its trampoline's catch
# (`bridge::rb::method::finish`, which `method!` / `function!` expand to), so a
# panic below it raises Makiri::InternalError: a property of the macros, not of
# each method, so there is nothing per method to check.

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

# Every raising C call in `bridge/raw.rs` is made inside `call(|| ...)`, the
# thunk-style `rb_protect` that hands the raise back as `Err` - so a caller can
# own a copy or hold a document guard across any of them, and nothing needs
# tracking above `raw.rs`. The functions that sit outside a `call` on
# purpose: `injected_raise` (it runs INSIDE every wrapper's closure, so its
# raise is that call's), and `jump_tag`, `exc_raise` and `exc_fatal`, which
# ARE the raise - `Error::raise` and `run_init` call them last, from a
# method's trampoline, with nothing owned on the way.
RAW_OUTSIDE_CALL = %w[injected_raise jump_tag exc_raise exc_fatal].freeze

def raw_unprotected(path)
  code = comments_removed(File.binread(path))
  skip = RAW_OUTSIDE_CALL.filter_map do |name|
    m = code.match(/\bfn\s+#{name}\b[^{]*\{/) or next
    open = m.end(0) - 1
    close = matching_close(code, open)
    close && (open..close)
  end
  spans = code.to_enum(:scan, /\bcall\s*\(/).filter_map do
    open = Regexp.last_match.end(0) - 1
    close = matching_close(code, open)
    close && (open..close)
  end
  # The `call` helper itself, whose body names `rb_protect`, not a raiser.
  code.to_enum(:scan, RAISING_API).filter_map do
    at = Regexp.last_match.begin(0)
    next if skip.any? { |r| r.cover?(at) } || spans.any? { |r| r.cover?(at) }

    "#{code[0...at].count("\n") + 1}: #{Regexp.last_match[0]}"
  end
end

unprotected = raw_unprotected(File.join(RUST, "bridge", "raw.rs"))
unless unprotected.empty?
  errors << "a raising C call in bridge/raw.rs outside `call(|| ...)`: #{unprotected.inspect}"
end

# What a GVL-released body may call. `bridge::gvl::without_gvl`'s `Send`
# bound keeps a Ruby handle or value from being carried into the body, but
# not one from being MADE there - `bridge::ruby::gvl_ruby` makes one from
# nothing, and every bridge helper reaches it. The engine reaches none of it
# (`RUBY_LAYER` above), so the body may call only engine functions, by their
# `crate::` path: a method call, a bare function or a macro could be
# anything, and is hoisted out of the closure instead (`src.input()` is
# evaluated before the call, not inside it). What this text cannot see is a
# function pointer the engine calls (`crate::stack`'s probe, which is Ruby's C);
# no GVL-released body reaches one today.
ENGINE_MODULES = ENGINE_DIRS.map { |d| d.sub(%r{/\z}, "").sub(/\.rs\z/, "") }.uniq.freeze
# A capitalised name is a constructor (`Some(..)`, `Ok(..)`), not a call.
GVL_RELEASED_CALL = /(?<dot>\.\s*)?(?<name>\b[A-Za-z_]\w*(?:\s*::\s*[A-Za-z_]\w*)*)\s*(?<bang>!)?\s*\(/
WITHOUT_GVL_EXEMPT = {
  "init.rs" => "the __panic(4) hook: a panic below the GVL, on purpose"
}.freeze

def gvl_released_violations(rust)
  Dir.glob(File.join(rust, "**", "*.rs")).sort.flat_map do |path|
    rel = path.delete_prefix("#{rust}/")
    next [] if WITHOUT_GVL_EXEMPT.key?(rel)

    code = code_only(File.binread(path))
    code.to_enum(:scan, /\bwithout_gvl\s*\(/).map { Regexp.last_match.end(0) - 1 }.flat_map do |open|
      close = matching_close(code, open) or next ["#{rel}: unbalanced without_gvl(...)"]
      line = code[0...open].count("\n") + 1
      found = []
      code[(open + 1)...close].scan(GVL_RELEASED_CALL) do
        m = Regexp.last_match
        name = m[:name].gsub(/\s+/, "")
        next if m[:dot].nil? && m[:bang].nil? && name.match?(/\A[A-Z]\w*\z/)
        next if m[:dot].nil? && m[:bang].nil? &&
                ENGINE_MODULES.any? { |mod| name.start_with?("crate::#{mod}::") }

        kind = m[:dot] ? "method call .#{name}" : m[:bang] ? "macro #{name}!" : name
        found << "#{rel}:#{line}: #{kind}"
      end
      found
    end
  end
end

gvl_released = gvl_released_violations(RUST)
unless gvl_released.empty?
  errors << "a GVL-released body (without_gvl) calls something other than an engine " \
            "function by its crate:: path (hoist it out of the closure): #{gvl_released.inspect}"
end

# The places that must not unwind (`src/no_unwind.rs`, proven by `rake
# no_unwind`). An unwind out of an `extern "C"` function aborts the process,
# and so does a panic in a `Drop` that runs while another unwinds or from the
# GC's free callback. That task proves each LISTED site; this makes sure none
# goes unlisted: every `extern "C"` function the shipping crate defines, and
# every `Drop` it implements, runs its body through `must_not_unwind` (or a
# helper that does: a method trampoline's `finish`, `Init_makiri`'s
# `run_init`).
NO_UNWIND_GUARD = /\bmust_not_unwind\b|\bfinish(?:_cary)?\s*\(|\brun_init\s*\(/
NO_UNWIND_SKIP = %r{\A(?:lexbor/|lint_probe\.rs\z|rust_tests\.rs\z)|(?:tests|selftest|verify)\.rs\z}
# `Drop`s no build that ships compiles (each module's own `cfg` says so).
DROP_EXEMPT = {
  "compat.rs" => "Reset: `as_lexbor`, under cfg(test) or the reference features",
  "css/mod.rs" => "Nesting: the CSS-to-XPath lowering, a test reference only"
}.freeze

def unguarded_no_unwind(rust)
  Dir.glob(File.join(rust, "**", "*.rs")).sort.flat_map do |path|
    rel = path.delete_prefix("#{rust}/")
    next [] if rel.match?(NO_UNWIND_SKIP)

    code = code_only(File.binread(path))
    code = code.split(/^#\[cfg\(test\)\]\s*\n\s*mod tests\b/).first.to_s
    found = []
    # `code_only` blanks a string's inside, so `extern "C"` reads `extern " "`.
    { /\bextern\s+" *"\s+fn\s+(\w+)[^{;]*\{/ => "extern \"C\" fn",
      /\bimpl\b[^{;]*?\bDrop\s+for\s+([\w:]+)[^{;]*\{/ => "Drop for" }.each do |re, kind|
      code.to_enum(:scan, re).each do
        m = Regexp.last_match
        next if kind == "Drop for" && DROP_EXEMPT.key?(rel)

        open = m.end(0) - 1
        close = matching_close(code, open) or next found << "#{rel}: unbalanced #{kind} #{m[1]}"
        next if code[open..close].match?(NO_UNWIND_GUARD)

        found << "#{rel}:#{code[0...open].count("\n") + 1}: #{kind} #{m[1]}"
      end
    end
    found
  end
end

unguarded = unguarded_no_unwind(RUST)
unless unguarded.empty?
  errors << "an extern \"C\" fn or a Drop whose body does not run under must_not_unwind " \
            "(add a site to src/no_unwind.rs): #{unguarded.inspect}"
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
     "#{user_code.values.sum} calls into the caller's Ruby (each allowed); " \
     "#{raw_values.values.sum} raw VALUEs outside raw.rs and typed.rs; " \
     "#{value_from_raw.values.sum} Value::from_raw and " \
     "#{lexbor_abi.values.sum} Lexbor ABI names outside their layer and " \
     "#{lexbor_decls.values.sum} Lexbor declarations outside lexbor/abi.rs; " \
     "#{raw_node_crossing.values.sum} raw-node crossings in bridge/; " \
     "#{ruby_layer.values.sum} Ruby-layer uses inside the engine; " \
     "#{ruby_surface.values.sum} Ruby methods in bridge/; " \
     "every raising call in bridge/raw.rs is inside `call`; " \
     "every GVL-released body calls only the engine; " \
     "every extern \"C\" fn and Drop runs under must_not_unwind"
