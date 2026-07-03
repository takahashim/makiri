# Verification build for the XML + XPath carve-out.
#
# Fully separate from the gem's extconf build: nothing here touches ext/'s
# Makefile or object files, and no Ruby VM or Lexbor library is linked (the
# only Lexbor pieces are UNMODIFIED vendor encoding sources - the vanilla
# constraint is about not patching, not about not compiling).
#
# Targets:
#   make smoke      - build every CBMC harness as a plain executable and run it
#                     (the link check: CBMC treats a missing function body as a
#                     nondet return, so only the linker catches a wrong source list)
#   make selftest   - build + run the carve-out selftest (XML parse + XPath eval,
#                     no Ruby, no Lexbor lib)
#   make cbmc       - the proven set (alloc/utf8/utf8-chain/span/spanbuf/buf/
#                     hash; ~2 min total)
#   make cbmc-deep  - the experimental set (xpath-lex/xpath-parse/xml-parse).
#                     These do NOT converge in practical time on current
#                     bit-level solvers (measured: >10 min each even at 3-4
#                     byte inputs, with MiniSat, kissat and Bitwuzla alike);
#                     kept as tuning knobs for future solver/harness work,
#                     e.g. make cbmc-xpath-parse XPATH_EXPR_MAX=4 XPATH_UNWIND=12
#
# Native libFuzzer targets live in ext/makiri/fuzz/ (their own Makefile + the
# nightly libfuzzer.yml workflow), not here - this directory is proofs only.

ROOT := ..
EXT  := $(ROOT)/ext/makiri
LXB  := $(ROOT)/vendor/lexbor/source

CC     ?= cc
CBMC   ?= cbmc
CFLAGS ?= -O1 -g -Wall -Wextra
INC    := -I $(EXT) -I $(LXB) -I .

CORE_SRC  := $(EXT)/core/mkr_alloc.c $(EXT)/core/mkr_buf.c \
             $(EXT)/core/mkr_core.c $(EXT)/core/mkr_utf8.c
XML_SRC   := $(EXT)/xml/mkr_xml_chars.c $(EXT)/xml/mkr_xml_index.c \
             $(EXT)/xml/mkr_xml_mutate.c $(EXT)/xml/mkr_xml_node.c \
             $(EXT)/xml/mkr_xml_tree.c
# Lexer/parser/number + shared support: what mkr_parse reaches.
XPATH_FRONT_SRC := $(EXT)/xpath/mkr_xpath_lex.c $(EXT)/xpath/mkr_xpath_parse.c \
                   $(EXT)/xpath/mkr_xpath_shared.c $(EXT)/xpath/mkr_xpath_number.c
# The full engine: dispatcher + the XML monomorphization (HTML side is stub.c).
XPATH_FULL_SRC  := $(XPATH_FRONT_SRC) $(EXT)/xpath/mkr_xpath.c \
                   $(EXT)/xpath/mkr_xpath_engine_xml.c
# Unmodified vendor sources for lxb_encoding_decode_valid_utf_8_single.
LXB_ENC_SRC := $(LXB)/lexbor/encoding/decode.c $(LXB)/lexbor/encoding/multi_res.c \
               $(LXB)/lexbor/encoding/range_res.c $(LXB)/lexbor/encoding/single_res.c

ENGINE_SRC := $(CORE_SRC) $(XML_SRC) $(XPATH_FULL_SRC) $(LXB_ENC_SRC) stub.c

BUILD := build

# ---------------------------------------------------------------- smoke + selftest

HARNESSES := alloc utf8 utf8-chain span spanbuf buf hash xpath-lex xpath-parse xml-parse

smoke: $(HARNESSES:%=$(BUILD)/harness-%)
	@for h in $^; do echo "== $$h"; $$h || exit 1; done
	@echo "smoke: all harnesses link and run"

$(BUILD)/harness-alloc: harness_alloc.c $(EXT)/core/mkr_alloc.c | $(BUILD)
	$(CC) $(CFLAGS) $(INC) $^ -o $@
$(BUILD)/harness-utf8: harness_utf8.c $(EXT)/core/mkr_utf8.c | $(BUILD)
	$(CC) $(CFLAGS) $(INC) $^ -o $@
$(BUILD)/harness-utf8-chain: harness_utf8_chain.c $(EXT)/core/mkr_utf8.c | $(BUILD)
	$(CC) $(CFLAGS) $(INC) $^ -o $@
$(BUILD)/harness-span: harness_span.c | $(BUILD)
	$(CC) $(CFLAGS) $(INC) $^ -o $@
$(BUILD)/harness-spanbuf: harness_spanbuf.c | $(BUILD)
	$(CC) $(CFLAGS) $(INC) $^ -o $@
$(BUILD)/harness-buf: harness_buf.c $(EXT)/core/mkr_buf.c $(EXT)/core/mkr_alloc.c | $(BUILD)
	$(CC) $(CFLAGS) $(INC) $^ -o $@
$(BUILD)/harness-hash: harness_hash.c | $(BUILD)
	$(CC) $(CFLAGS) $(INC) $^ -o $@
# mkr_parse's limit checks + limits_init_defaults live in mkr_xpath.c (the
# dispatcher), which references both engine instances - so the smoke link
# takes the full engine (stub.c supplies the HTML side).
$(BUILD)/harness-xpath-parse: harness_xpath_parse.c $(ENGINE_SRC) | $(BUILD)
	$(CC) $(CFLAGS) $(INC) $^ -o $@
$(BUILD)/harness-xpath-lex: harness_xpath_lex.c $(ENGINE_SRC) | $(BUILD)
	$(CC) $(CFLAGS) $(INC) $^ -o $@
$(BUILD)/harness-xml-parse: harness_xml_parse.c $(XML_SRC) $(CORE_SRC) | $(BUILD)
	$(CC) $(CFLAGS) $(INC) $^ -o $@

selftest: $(BUILD)/selftest
	$(BUILD)/selftest

$(BUILD)/selftest: selftest_main.c $(ENGINE_SRC) $(EXT)/xpath/mkr_xpath_xml_selftest.c | $(BUILD)
	$(CC) $(CFLAGS) $(INC) $^ -o $@

$(BUILD):
	mkdir -p $(BUILD)

# ---------------------------------------------------------------- CBMC proofs
#
# Every proof gets --bounds-check --pointer-check (memory safety) and
# --signed-overflow-check (UB); --unsigned-overflow-check is added ONLY where
# wrapping would be a bug (the size arithmetic) - the UTF-8 validator's
# word-at-a-time scan and hash-style code wrap on purpose, and unsigned wrap
# is defined C. --malloc-may-fail --malloc-fail-null verifies the fail-closed
# OOM branches. Unwind bounds are per-harness knobs; --unwinding-assertions
# turns "too small a bound" into a failure instead of a silent partial proof.

CBMC_MEM := --bounds-check --pointer-check --signed-overflow-check \
            --div-by-zero-check --unwinding-assertions --object-bits 12
CBMC_OOM := --malloc-may-fail --malloc-fail-null
# libc bodies CBMC lacks on this platform (see the file's comment).
CBMC_MODELS := cbmc_models.c

# mkr_grow_capacity doubles from 8, so <= ~60 iterations before the cap check.
ALLOC_UNWIND ?= 64
cbmc-alloc:
	$(CBMC) harness_alloc.c $(CBMC_MODELS) $(EXT)/core/mkr_alloc.c $(INC) \
	  $(CBMC_MEM) $(CBMC_OOM) --unsigned-overflow-check --conversion-check \
	  --unwind $(ALLOC_UNWIND)

UTF8_UNWIND ?= 12
cbmc-utf8:
	$(CBMC) harness_utf8.c $(CBMC_MODELS) $(EXT)/core/mkr_utf8.c $(INC) \
	  $(CBMC_MEM) --unwind $(UTF8_UNWIND)

cbmc-utf8-chain:
	$(CBMC) harness_utf8_chain.c $(CBMC_MODELS) $(EXT)/core/mkr_utf8.c $(INC) \
	  $(CBMC_MEM) --unwind $(UTF8_UNWIND)

SPAN_UNWIND ?= 12
cbmc-span:
	$(CBMC) harness_span.c $(CBMC_MODELS) $(INC) \
	  $(CBMC_MEM) --unwind $(SPAN_UNWIND)

cbmc-spanbuf:
	$(CBMC) harness_spanbuf.c $(CBMC_MODELS) $(INC) \
	  $(CBMC_MEM) --unwind $(SPAN_UNWIND)

cbmc-buf:
	$(CBMC) harness_buf.c $(CBMC_MODELS) $(EXT)/core/mkr_buf.c $(EXT)/core/mkr_alloc.c $(INC) \
	  $(CBMC_MEM) $(CBMC_OOM) --unwind 16

# The sizer only shifts, so unlike the mul/div guards this closes at full
# width; the loop runs at most one step per size_t bit.
HASH_UNWIND ?= 66
cbmc-hash:
	$(CBMC) harness_hash.c $(CBMC_MODELS) $(INC) \
	  $(CBMC_MEM) --unwind $(HASH_UNWIND)

cbmc: cbmc-alloc cbmc-utf8 cbmc-utf8-chain cbmc-span cbmc-spanbuf cbmc-buf cbmc-hash
	@echo "cbmc: all proofs passed"

# ------------------------------------------------- experimental (cbmc-deep)
#
# The whole-entry parser/lexer proofs below are formulated and smoke-checked,
# but no current solver closes them in practical time (see the header). CBMC
# gets mkr_xpath.c for the limit checks but NOT the engine instances or the
# vendor encoding tables (nothing there is reachable from mkr_parse; the eval
# entries mkr_xpath.c references stay bodyless - unreachable from these
# harnesses). Expression/input length and unwind grow together.

XPATH_EXPR_MAX ?= 6
XPATH_UNWIND   ?= 16
cbmc-xpath-lex:
	$(CBMC) harness_xpath_lex.c $(CBMC_MODELS) $(XPATH_FRONT_SRC) $(EXT)/xpath/mkr_xpath.c $(CORE_SRC) $(INC) \
	  -DVERIFY_XPATH_EXPR_MAX=$(XPATH_EXPR_MAX) \
	  $(CBMC_MEM) --unwind $(XPATH_UNWIND)

cbmc-xpath-parse:
	$(CBMC) harness_xpath_parse.c $(CBMC_MODELS) $(XPATH_FRONT_SRC) $(EXT)/xpath/mkr_xpath.c $(CORE_SRC) $(INC) \
	  -DVERIFY_XPATH_EXPR_MAX=$(XPATH_EXPR_MAX) \
	  $(CBMC_MEM) $(CBMC_OOM) --unwind $(XPATH_UNWIND)

XML_INPUT_MAX ?= 6
XML_UNWIND    ?= 16
cbmc-xml-parse:
	$(CBMC) harness_xml_parse.c $(CBMC_MODELS) $(XML_SRC) $(CORE_SRC) $(INC) \
	  -DVERIFY_XML_INPUT_MAX=$(XML_INPUT_MAX) \
	  $(CBMC_MEM) $(CBMC_OOM) --unwind $(XML_UNWIND)

cbmc-deep: cbmc-xpath-lex cbmc-xpath-parse cbmc-xml-parse

clean:
	rm -rf $(BUILD)

.PHONY: smoke selftest cbmc cbmc-alloc cbmc-utf8 cbmc-utf8-chain cbmc-span \
        cbmc-spanbuf cbmc-buf cbmc-hash \
        cbmc-deep cbmc-xpath-lex cbmc-xpath-parse cbmc-xml-parse clean
