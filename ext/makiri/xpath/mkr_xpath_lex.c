#include "mkr_xpath_internal.h"

#include <ctype.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

/*
 * XPath 1.0 lexer. The parser drives token consumption and handles
 * context-sensitive keywords (and, or, div, mod, *, node()...) via
 * lookahead - the lexer is intentionally context-free.
 *
 * All input reads go through the bounded reader (core mkr_span_t): the input
 * is a verified text (NUL-free, NUL-terminated, valid UTF-8) of known length,
 * and the span makes an out-of-bounds read structurally impossible rather
 * than a per-site convention. One-codepoint decoding is the strict core
 * mkr_utf8_decode1 (the lexer formerly carried its own equivalent copy).
 */

/* NCName code-point classes per XPath 1.0 §3.7 -> Namespaces in XML -> the
 * XML 1.0 (5th ed.) Name production, minus ':' (NCName). NameStartChar and
 * the extra NameChar ranges; this is the definition browsers / libxml2 track. */
static int
is_ncname_start_cp(uint32_t c)
{
  return c == '_'
      || (c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z')
      || (c >= 0xC0    && c <= 0xD6)    || (c >= 0xD8    && c <= 0xF6)
      || (c >= 0xF8    && c <= 0x2FF)   || (c >= 0x370   && c <= 0x37D)
      || (c >= 0x37F   && c <= 0x1FFF)  || (c >= 0x200C  && c <= 0x200D)
      || (c >= 0x2070  && c <= 0x218F)  || (c >= 0x2C00  && c <= 0x2FEF)
      || (c >= 0x3001  && c <= 0xD7FF)  || (c >= 0xF900  && c <= 0xFDCF)
      || (c >= 0xFDF0  && c <= 0xFFFD)  || (c >= 0x10000 && c <= 0xEFFFF);
}

static int
is_ncname_cont_cp(uint32_t c)
{
  return is_ncname_start_cp(c)
      || c == '-' || c == '.' || (c >= '0' && c <= '9')
      || c == 0xB7
      || (c >= 0x0300 && c <= 0x036F) || (c >= 0x203F && c <= 0x2040);
}

/* If the span's cursor (+off bytes) begins an NCName character, return its
 * byte length (1-4); else 0 (including at end-of-input - the strict decode
 * fails closed there). +start+ selects NameStartChar (1) vs NameChar (0). */
static int
ncname_char(const mkr_span_t *in, size_t off, int start)
{
  mkr_span_t s = mkr_span_tail(in, off);
  uint32_t cp;
  int len = mkr_utf8_decode1_span(&s, &cp);
  if (len == 0) return 0;
  return (start ? is_ncname_start_cp(cp) : is_ncname_cont_cp(cp)) ? len : 0;
}

static void
skip_ws(mkr_lexer_t *L)
{
  /* XPath 1.0 §3.7 ExprWhitespace = XML S = (#x20 | #x9 | #xD | #xA)+ only.
   * Not C isspace(), which would also skip #xB (\v) and #xC (\f) - those are
   * not XPath whitespace and must surface as a syntax error. */
  for (;;) {
    int c = mkr_span_peek(&L->in);
    if (c == ' ' || c == '\t' || c == '\r' || c == '\n') mkr_span_skip(&L->in, 1);
    else break;
  }
}

static int
lex_number(mkr_lexer_t *L, mkr_token_t *t, mkr_xpath_error_t *err)
{
  const char *s = mkr_span_mark(&L->in);
  char *end = NULL;
  /* strtod scans the raw bytes, but the engine text contract makes that
   * bounded: the input is NUL-terminated and NUL-free, so the scan can never
   * leave the buffer (lint-allowlisted as the one sanctioned raw scan). */
  double v = strtod(s, &end);
  if (end == s) {
    mkr_err_setf(err, MKR_XPATH_ERR_SYNTAX, "expected number at '%.10s'", s);
    return -1;
  }
  t->kind  = MKR_TK_NUMBER;
  t->text.ptr = s;
  t->text.len   = (size_t)(end - s);
  t->num   = v;
  mkr_span_skip(&L->in, (size_t)(end - s));
  return 0;
}

static int
lex_string(mkr_lexer_t *L, mkr_token_t *t, mkr_xpath_error_t *err)
{
  int quote = mkr_span_peek(&L->in);
  mkr_span_skip(&L->in, 1);
  const char *start = mkr_span_mark(&L->in);
  size_t len;
  if (!mkr_span_find(&L->in, (char)quote, &len)) {
    mkr_err_set(err, MKR_XPATH_ERR_SYNTAX, "unterminated string literal");
    return -1;
  }
  /* Validate UTF-8 here, once, so every character-wise string function
   * (translate, substring, string-length, ...) can assume well-formed input
   * and so an invalid literal fails closed (SyntaxError) rather than producing
   * a silently wrong/truncated result. The strict core validator applies the
   * same rules (overlong/surrogate/out-of-range rejected) as the strict
   * decoder the rest of the lexer uses. */
  if (!mkr_utf8_valid((const unsigned char *)start, len)) {
    mkr_err_set(err, MKR_XPATH_ERR_SYNTAX, "invalid UTF-8 in string literal");
    return -1;
  }
  t->kind  = MKR_TK_LITERAL;
  t->text.ptr = start;
  t->text.len   = len;
  mkr_span_skip(&L->in, len + 1);   /* content + closing quote */
  return 0;
}

static int
lex_name(mkr_lexer_t *L, mkr_token_t *t)
{
  const char *s = mkr_span_mark(&L->in);
  int n;
  while ((n = ncname_char(&L->in, 0, 0)) > 0) mkr_span_skip(&L->in, (size_t)n);
  /* QName NameTest is `prefix:local` OR `prefix:*` (XPath 1.0 §2.3); the ':'
   * must not be the '::' axis separator. */
  if (mkr_span_peek(&L->in) == ':' && mkr_span_at(&L->in, 1) != ':'
      && (mkr_span_at(&L->in, 1) == '*' || ncname_char(&L->in, 1, 1) > 0)) {
    mkr_span_skip(&L->in, 1); /* eat ':' */
    if (mkr_span_peek(&L->in) == '*') {
      mkr_span_skip(&L->in, 1); /* prefix:* */
    } else {
      while ((n = ncname_char(&L->in, 0, 0)) > 0) mkr_span_skip(&L->in, (size_t)n);
    }
    t->kind = MKR_TK_QNAME;
  } else {
    t->kind = MKR_TK_NAME;
  }
  t->text.ptr = s;
  t->text.len   = mkr_span_since(&L->in, s);
  return 0;
}

static int
next_token(mkr_lexer_t *L, mkr_token_t *t, mkr_xpath_error_t *err)
{
  memset(t, 0, sizeof(*t));
  skip_ws(L);
  const char *s = mkr_span_mark(&L->in);
  int c = mkr_span_peek(&L->in);

  if (c < 0) {
    t->kind = MKR_TK_EOF;
    t->text.ptr = s;
    t->text.len = 0;
    return 0;
  }

  int c1 = mkr_span_at(&L->in, 1);
  if (c == '/' && c1 == '/') { t->kind = MKR_TK_DSLASH;     t->text.ptr = s; t->text.len = 2; mkr_span_skip(&L->in, 2); return 0; }
  if (c == '.' && c1 == '.') { t->kind = MKR_TK_DOTDOT;     t->text.ptr = s; t->text.len = 2; mkr_span_skip(&L->in, 2); return 0; }
  if (c == ':' && c1 == ':') { t->kind = MKR_TK_COLONCOLON; t->text.ptr = s; t->text.len = 2; mkr_span_skip(&L->in, 2); return 0; }
  if (c == '!' && c1 == '=') { t->kind = MKR_TK_NE;         t->text.ptr = s; t->text.len = 2; mkr_span_skip(&L->in, 2); return 0; }
  if (c == '<' && c1 == '=') { t->kind = MKR_TK_LE;         t->text.ptr = s; t->text.len = 2; mkr_span_skip(&L->in, 2); return 0; }
  if (c == '>' && c1 == '=') { t->kind = MKR_TK_GE;         t->text.ptr = s; t->text.len = 2; mkr_span_skip(&L->in, 2); return 0; }

  switch (c) {
  case '(': t->kind = MKR_TK_LPAREN;   t->text.ptr = s; t->text.len = 1; mkr_span_skip(&L->in, 1); return 0;
  case ')': t->kind = MKR_TK_RPAREN;   t->text.ptr = s; t->text.len = 1; mkr_span_skip(&L->in, 1); return 0;
  case '[': t->kind = MKR_TK_LBRACKET; t->text.ptr = s; t->text.len = 1; mkr_span_skip(&L->in, 1); return 0;
  case ']': t->kind = MKR_TK_RBRACKET; t->text.ptr = s; t->text.len = 1; mkr_span_skip(&L->in, 1); return 0;
  case '.':
    if (c1 >= 0 && isdigit(c1)) {
      return lex_number(L, t, err);
    }
    t->kind = MKR_TK_DOT; t->text.ptr = s; t->text.len = 1; mkr_span_skip(&L->in, 1); return 0;
  case '@': t->kind = MKR_TK_AT;       t->text.ptr = s; t->text.len = 1; mkr_span_skip(&L->in, 1); return 0;
  case ',': t->kind = MKR_TK_COMMA;    t->text.ptr = s; t->text.len = 1; mkr_span_skip(&L->in, 1); return 0;
  case '|': t->kind = MKR_TK_PIPE;     t->text.ptr = s; t->text.len = 1; mkr_span_skip(&L->in, 1); return 0;
  case '/': t->kind = MKR_TK_SLASH;    t->text.ptr = s; t->text.len = 1; mkr_span_skip(&L->in, 1); return 0;
  case '+': t->kind = MKR_TK_PLUS;     t->text.ptr = s; t->text.len = 1; mkr_span_skip(&L->in, 1); return 0;
  case '-': t->kind = MKR_TK_MINUS;    t->text.ptr = s; t->text.len = 1; mkr_span_skip(&L->in, 1); return 0;
  case '*': t->kind = MKR_TK_STAR;     t->text.ptr = s; t->text.len = 1; mkr_span_skip(&L->in, 1); return 0;
  case '=': t->kind = MKR_TK_EQ;       t->text.ptr = s; t->text.len = 1; mkr_span_skip(&L->in, 1); return 0;
  case '<': t->kind = MKR_TK_LT;       t->text.ptr = s; t->text.len = 1; mkr_span_skip(&L->in, 1); return 0;
  case '>': t->kind = MKR_TK_GT;       t->text.ptr = s; t->text.len = 1; mkr_span_skip(&L->in, 1); return 0;
  case '$': t->kind = MKR_TK_DOLLAR;   t->text.ptr = s; t->text.len = 1; mkr_span_skip(&L->in, 1); return 0;
  case '\'':
  case '"':
    return lex_string(L, t, err);
  default:
    break;
  }

  if (isdigit(c)) {
    return lex_number(L, t, err);
  }
  if (ncname_char(&L->in, 0, 1) > 0) {
    return lex_name(L, t);
  }

  if (c >= 0x20 && c < 0x7F) {
    mkr_err_setf(err, MKR_XPATH_ERR_SYNTAX, "unexpected character '%c'", c);
  } else {
    mkr_err_setf(err, MKR_XPATH_ERR_SYNTAX, "unexpected byte 0x%02x", (unsigned)c);
  }
  return -1;
}

void
mkr_lexer_init(mkr_lexer_t *L, const char *src, size_t len, mkr_xpath_error_t *err)
{
  L->src = src;
  L->in  = mkr_span(src, len);
  memset(&L->tok, 0, sizeof(L->tok));
  L->good = (next_token(L, &L->tok, err) == 0);
}

int
mkr_lexer_advance(mkr_lexer_t *L, mkr_xpath_error_t *err)
{
  L->good = (next_token(L, &L->tok, err) == 0);
  return L->good ? 0 : -1;
}

/* The next non-whitespace byte after the current token (or -1 at the end),
 * without consuming anything - the parser's function-call lookahead ("is this
 * NAME followed by '('?"), kept here so the raw cursor never leaves the lexer. */
int
mkr_lexer_peek_nonws(const mkr_lexer_t *L)
{
  mkr_span_t s = L->in;
  for (;;) {
    int c = mkr_span_peek(&s);
    if (c == ' ' || c == '\t' || c == '\n' || c == '\r') { mkr_span_skip(&s, 1); continue; }
    return c;
  }
}

int
mkr_tok_is_word_len(const mkr_token_t *t, const char *word, size_t word_len)
{
  if (t->kind != MKR_TK_NAME) return 0;
  return mkr_borrowed_text_eq(t->text, mkr_borrowed_text(word, word_len));
}
