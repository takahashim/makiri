#include "mkr_xpath_internal.h"

#include <ctype.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

/*
 * XPath 1.0 lexer. The parser drives token consumption and handles
 * context-sensitive keywords (and, or, div, mod, *, node()...) via
 * lookahead — the lexer is intentionally context-free.
 */

/* Decode one UTF-8 code point at NUL-terminated +p+. Returns its byte length
 * (1-4) and writes the code point to *cp; returns 0 at the terminator or on a
 * malformed/truncated/overlong/surrogate/out-of-range sequence (fail closed).
 * Never reads past the NUL: a NUL where a continuation byte is expected fails
 * the (b & 0xC0) == 0x80 check. */
static int
utf8_decode(const char *p, uint32_t *cp)
{
  const unsigned char *u = (const unsigned char *)p;
  unsigned char b0 = u[0];
  if (b0 < 0x80) { *cp = b0; return b0 == 0 ? 0 : 1; }

  int n;
  uint32_t c, min;
  if      ((b0 & 0xE0) == 0xC0) { n = 1; c = b0 & 0x1Fu; min = 0x80; }
  else if ((b0 & 0xF0) == 0xE0) { n = 2; c = b0 & 0x0Fu; min = 0x800; }
  else if ((b0 & 0xF8) == 0xF0) { n = 3; c = b0 & 0x07u; min = 0x10000; }
  else return 0; /* continuation byte or 0xF8+ as a lead: invalid */

  for (int i = 1; i <= n; i++) {
    unsigned char b = u[i];
    if ((b & 0xC0) != 0x80) return 0; /* truncated (incl. NUL) or bad continuation */
    c = (c << 6) | (b & 0x3Fu);
  }
  if (c < min) return 0;                      /* overlong */
  if (c >= 0xD800 && c <= 0xDFFF) return 0;   /* surrogate */
  if (c > 0x10FFFF) return 0;                 /* out of Unicode range */
  *cp = c;
  return n + 1;
}

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

/* If +p+ begins an NCName character, return its byte length (1-4); else 0.
 * +start+ selects the NameStartChar (1) vs NameChar (0) class. */
static int
ncname_char(const char *p, int start)
{
  uint32_t cp;
  int len = utf8_decode(p, &cp);
  if (len == 0) return 0;
  return (start ? is_ncname_start_cp(cp) : is_ncname_cont_cp(cp)) ? len : 0;
}

static void
skip_ws(mkr_lexer_t *L)
{
  while (*L->cur && isspace((unsigned char)*L->cur)) L->cur++;
}

static int
lex_number(mkr_lexer_t *L, mkr_token_t *t, mkr_xpath_error_t *err)
{
  const char *s = L->cur;
  char *end = NULL;
  double v = strtod(s, &end);
  if (end == s) {
    mkr_err_setf(err, MKR_XPATH_ERR_SYNTAX, "expected number at '%.10s'", s);
    return -1;
  }
  t->kind  = MKR_TK_NUMBER;
  t->start = s;
  t->len   = (size_t)(end - s);
  t->num   = v;
  L->cur   = end;
  return 0;
}

static int
lex_string(mkr_lexer_t *L, mkr_token_t *t, mkr_xpath_error_t *err)
{
  char quote = *L->cur;
  const char *start = L->cur + 1;
  const char *p = start;
  while (*p && *p != quote) p++;
  if (*p != quote) {
    mkr_err_set(err, MKR_XPATH_ERR_SYNTAX, "unterminated string literal");
    return -1;
  }
  t->kind  = MKR_TK_LITERAL;
  t->start = start;
  t->len   = (size_t)(p - start);
  L->cur   = p + 1;
  return 0;
}

static int
lex_name(mkr_lexer_t *L, mkr_token_t *t)
{
  const char *s = L->cur;
  int n;
  while ((n = ncname_char(L->cur, 0)) > 0) L->cur += n;
  if (*L->cur == ':' && L->cur[1] != ':' && ncname_char(L->cur + 1, 1) > 0) {
    L->cur++; /* eat ':' */
    while ((n = ncname_char(L->cur, 0)) > 0) L->cur += n;
    t->kind = MKR_TK_QNAME;
  } else {
    t->kind = MKR_TK_NAME;
  }
  t->start = s;
  t->len   = (size_t)(L->cur - s);
  return 0;
}

static int
next_token(mkr_lexer_t *L, mkr_token_t *t, mkr_xpath_error_t *err)
{
  memset(t, 0, sizeof(*t));
  skip_ws(L);
  const char *s = L->cur;
  char c = *s;

  if (c == '\0') {
    t->kind = MKR_TK_EOF;
    t->start = s;
    t->len = 0;
    return 0;
  }

  if (c == '/' && s[1] == '/') { t->kind = MKR_TK_DSLASH;     t->start = s; t->len = 2; L->cur += 2; return 0; }
  if (c == '.' && s[1] == '.') { t->kind = MKR_TK_DOTDOT;     t->start = s; t->len = 2; L->cur += 2; return 0; }
  if (c == ':' && s[1] == ':') { t->kind = MKR_TK_COLONCOLON; t->start = s; t->len = 2; L->cur += 2; return 0; }
  if (c == '!' && s[1] == '=') { t->kind = MKR_TK_NE;         t->start = s; t->len = 2; L->cur += 2; return 0; }
  if (c == '<' && s[1] == '=') { t->kind = MKR_TK_LE;         t->start = s; t->len = 2; L->cur += 2; return 0; }
  if (c == '>' && s[1] == '=') { t->kind = MKR_TK_GE;         t->start = s; t->len = 2; L->cur += 2; return 0; }

  switch (c) {
  case '(': t->kind = MKR_TK_LPAREN;   t->start = s; t->len = 1; L->cur++; return 0;
  case ')': t->kind = MKR_TK_RPAREN;   t->start = s; t->len = 1; L->cur++; return 0;
  case '[': t->kind = MKR_TK_LBRACKET; t->start = s; t->len = 1; L->cur++; return 0;
  case ']': t->kind = MKR_TK_RBRACKET; t->start = s; t->len = 1; L->cur++; return 0;
  case '.':
    if (isdigit((unsigned char)s[1])) {
      return lex_number(L, t, err);
    }
    t->kind = MKR_TK_DOT; t->start = s; t->len = 1; L->cur++; return 0;
  case '@': t->kind = MKR_TK_AT;       t->start = s; t->len = 1; L->cur++; return 0;
  case ',': t->kind = MKR_TK_COMMA;    t->start = s; t->len = 1; L->cur++; return 0;
  case '|': t->kind = MKR_TK_PIPE;     t->start = s; t->len = 1; L->cur++; return 0;
  case '/': t->kind = MKR_TK_SLASH;    t->start = s; t->len = 1; L->cur++; return 0;
  case '+': t->kind = MKR_TK_PLUS;     t->start = s; t->len = 1; L->cur++; return 0;
  case '-': t->kind = MKR_TK_MINUS;    t->start = s; t->len = 1; L->cur++; return 0;
  case '*': t->kind = MKR_TK_STAR;     t->start = s; t->len = 1; L->cur++; return 0;
  case '=': t->kind = MKR_TK_EQ;       t->start = s; t->len = 1; L->cur++; return 0;
  case '<': t->kind = MKR_TK_LT;       t->start = s; t->len = 1; L->cur++; return 0;
  case '>': t->kind = MKR_TK_GT;       t->start = s; t->len = 1; L->cur++; return 0;
  case '$': t->kind = MKR_TK_DOLLAR;   t->start = s; t->len = 1; L->cur++; return 0;
  case '\'':
  case '"':
    return lex_string(L, t, err);
  default:
    break;
  }

  if (isdigit((unsigned char)c)) {
    return lex_number(L, t, err);
  }
  if (ncname_char(s, 1) > 0) {
    return lex_name(L, t);
  }

  unsigned char uc = (unsigned char)c;
  if (uc >= 0x20 && uc < 0x7F) {
    mkr_err_setf(err, MKR_XPATH_ERR_SYNTAX, "unexpected character '%c'", c);
  } else {
    mkr_err_setf(err, MKR_XPATH_ERR_SYNTAX, "unexpected byte 0x%02x", uc);
  }
  return -1;
}

void
mkr_lexer_init(mkr_lexer_t *L, const char *src, mkr_xpath_error_t *err)
{
  L->src = src;
  L->cur = src;
  memset(&L->tok, 0, sizeof(L->tok));
  L->good = (next_token(L, &L->tok, err) == 0);
}

int
mkr_lexer_advance(mkr_lexer_t *L, mkr_xpath_error_t *err)
{
  L->good = (next_token(L, &L->tok, err) == 0);
  return L->good ? 0 : -1;
}

int
mkr_tok_is_word(const mkr_token_t *t, const char *word)
{
  if (t->kind != MKR_TK_NAME) return 0;
  size_t n = strlen(word);
  return t->len == n && memcmp(t->start, word, n) == 0;
}
