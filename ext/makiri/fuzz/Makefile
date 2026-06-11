# ext/makiri/fuzz/Makefile - libFuzzer harness build
#
# Usage:
#   make xml_fuzz      # build the XML parser harness
#   make xpath_fuzz    # build the XPath compile+eval harness
#   make clean         # remove build artifacts
#
# Requires: clang with libFuzzer support (usually part of the clang distribution)
#   and the vendored Lexbor static library already built (via `rake compile`).

CLANG ?= clang
CXX   := $(if $(filter default,$(origin CXX)),clang++,$(CXX))

# Paths relative to ext/makiri/fuzz/
EXT_DIR    = ..
LEXBOR_SRC = ../../../vendor/lexbor
LEXBOR_DST = $(LEXBOR_SRC)/dist

# CFLAGS mirror the security flags from extconf.rb, minus Ruby-specific flags.
# Sanitizer instrumentation is added at compile time so every TU is covered.
SANITIZE ?= address,undefined
CFLAGS = -O2 -g -Wall -Wextra \
  -fstack-protector-strong \
  -Wformat -Wformat-security \
  -fvisibility=hidden \
  -fno-common \
  -fsanitize=$(SANITIZE) \
  -I$(LEXBOR_DST)/include \
  -I$(EXT_DIR) \
  -I$(EXT_DIR)/core \
  -I$(EXT_DIR)/xml \
  -I$(EXT_DIR)/xpath \
  -I$(EXT_DIR)/lexbor_compat

# Linker flags: libFuzzer driver + sanitizers.
LDFLAGS  = -fsanitize=fuzzer,$(SANITIZE)

BUILD_DIR = build

# Ruby-free C sources that the harnesses link against.
CORE_SRCS = \
  $(EXT_DIR)/core/mkr_alloc.c \
  $(EXT_DIR)/core/mkr_utf8.c \
  $(EXT_DIR)/core/mkr_buf.c \
  $(EXT_DIR)/core/mkr_core.c

XML_SRCS = \
  $(EXT_DIR)/xml/mkr_xml_tree.c \
  $(EXT_DIR)/xml/mkr_xml_node.c \
  $(EXT_DIR)/xml/mkr_xml_chars.c \
  $(EXT_DIR)/xml/mkr_xml_index.c \
  $(EXT_DIR)/xml/mkr_xml_mutate.c

XPATH_SRCS = \
  $(EXT_DIR)/xpath/mkr_xpath.c \
  $(EXT_DIR)/xpath/mkr_xpath_lex.c \
  $(EXT_DIR)/xpath/mkr_xpath_parse.c \
  $(EXT_DIR)/xpath/mkr_xpath_shared.c \
  $(EXT_DIR)/xpath/mkr_xpath_engine_xml.c \
  $(EXT_DIR)/xpath/mkr_xpath_engine_html.c

CORE_OBJS  = $(patsubst $(EXT_DIR)/%.c,$(BUILD_DIR)/%.o,$(CORE_SRCS))
XML_OBJS   = $(patsubst $(EXT_DIR)/%.c,$(BUILD_DIR)/%.o,$(XML_SRCS))
XPATH_OBJS = $(patsubst $(EXT_DIR)/%.c,$(BUILD_DIR)/%.o,$(XPATH_SRCS))

.PHONY: all clean check-libfuzzer xml_fuzz xpath_fuzz

all: check-libfuzzer xml_fuzz xpath_fuzz

check-libfuzzer:
	@mkdir -p $(BUILD_DIR)
	@$(CXX) $(LDFLAGS) -o $(BUILD_DIR)/check_fuzzer check_fuzzer.cc >/dev/null 2>&1 || \
	  (echo "libFuzzer runtime not available for $(CXX). Install LLVM clang and run with CLANG=/path/to/clang CXX=/path/to/clang++." >&2; exit 1)

xml_fuzz: $(BUILD_DIR)/xml_fuzz.o $(CORE_OBJS) $(XML_OBJS)
	$(CXX) $(LDFLAGS) -o $@ $^

xpath_fuzz: $(BUILD_DIR)/xpath_fuzz.o $(CORE_OBJS) $(XML_OBJS) $(XPATH_OBJS)
	$(CXX) $(LDFLAGS) -o $@ $^ $(LEXBOR_DST)/lib/liblexbor_static.a

$(BUILD_DIR)/%.o: $(EXT_DIR)/%.c
	@mkdir -p $(dir $@)
	$(CLANG) $(CFLAGS) -c -o $@ $<

$(BUILD_DIR)/xml_fuzz.o: xml_fuzz.c
	@mkdir -p $(dir $@)
	$(CLANG) $(CFLAGS) -c -o $@ $<

$(BUILD_DIR)/xpath_fuzz.o: xpath_fuzz.c
	@mkdir -p $(dir $@)
	$(CLANG) $(CFLAGS) -c -o $@ $<

clean:
	rm -rf $(BUILD_DIR) xml_fuzz xpath_fuzz
