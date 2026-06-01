#include "mkr_xpath_internal.h"

#include <ctype.h>
#include <stdlib.h>
#include <string.h>

/*
 * XPath 1.0 lexer. The parser drives token consumption and handles
 * context-sensitive keywords (and, or, div, mod, *, node()...) via
 * lookahead — the lexer is intentionally context-free.
 */

static int
is_ncname_start(int c)
{
  return c == '_' || isalpha(c);
}

static int
is_ncname_cont(int c)
{
  return c == '_' || c == '-' || c == '.' || isalnum(c);
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
  while (is_ncname_cont((unsigned char)*L->cur)) L->cur++;
  if (*L->cur == ':' && L->cur[1] != ':' && is_ncname_start((unsigned char)L->cur[1])) {
    L->cur++; /* eat ':' */
    while (is_ncname_cont((unsigned char)*L->cur)) L->cur++;
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
  if (is_ncname_start((unsigned char)c)) {
    return lex_name(L, t);
  }

  mkr_err_setf(err, MKR_XPATH_ERR_SYNTAX, "unexpected character '%c'", c);
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
