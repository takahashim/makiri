#ifndef MAKIRI_CORE_MKR_TEXT_H
#define MAKIRI_CORE_MKR_TEXT_H

/*
 * The string-type lattice: ownership- and contract-typed {ptr,len} views.
 * Ruby-free (the VALUE-anchored variants live in bridge.h). See
 * docs/string_types.ja.md for the full guide. (mkr_safe.h is a thin umbrella
 * over mkr_alloc.h + this + mkr_buf.h.)
 */

#include <stddef.h>
#include <stdlib.h> /* free() for mkr_owned_bytes_clear */

#ifdef __cplusplus
extern "C" {
#endif

/* ---------------------------------------------------------------- */
/* mkr_verified_text_t — a string proven to meet the engine text contract */
/* ---------------------------------------------------------------- */

/* A borrowed byte slice whose contents are guaranteed to satisfy Makiri's
 * engine text contract: valid UTF-8, no interior NUL, and NUL-terminated at
 * ptr[len]. The XPath engine's public string entry points accept ONLY this
 * type, so a raw `const char *` cannot be passed without a compile error. The
 * sole constructor is mkr_verified_text_from_view() at the Ruby boundary (bridge),
 * which can only be fed a view that mkr_ruby_verified_text() already validated;
 * there is deliberately no Ruby-free constructor, so unvalidated bytes have no
 * path into the engine. (ptr is non-owning: the caller keeps it alive for the
 * duration of the engine call.) */
typedef struct {
    const char *ptr;
    size_t      len;
} mkr_verified_text_t;

/* ---------------------------------------------------------------- */
/* string-type lattice                                              */
/* ---------------------------------------------------------------- */
/*
 * Makiri's string types form a small lattice over two axes plus a shape marker.
 * They look alike ({ptr,len}) but C has no subtyping, so each contract is its
 * own type — that distinctness IS the guarantee, and is why there is no single
 * "string" type.
 *
 *   axis 1  ownership : borrowed (we never free) | owned (free via *_clear)
 *   axis 2  contract  : bytes (raw, unchecked) | text (valid UTF-8, no interior
 *                       NUL, NUL-terminated at ptr[len])
 *   marker  ruby_     : also carries a Ruby VALUE as a GC liveness anchor for
 *                       ptr (bridge layer only; the engine stays Ruby-free)
 *
 *   shape \ contract        raw (bytes)               valid (text)
 *   ----------------------  ------------------------  -------------------------
 *   ruby-anchored borrowed  mkr_ruby_borrowed_bytes_t mkr_ruby_borrowed_text_t  (bridge.h)
 *   borrowed slice          (none yet — would be      mkr_borrowed_text_t /
 *                            mkr_borrowed_bytes_t)     mkr_verified_text_t (*)
 *   owned                   mkr_owned_bytes_t         mkr_owned_text_t
 *
 *   The borrowed-raw cell is intentionally empty: nothing needs an unanchored
 *   raw slice today. When something does, name it mkr_borrowed_bytes_t per the
 *   convention above; do not resurrect a placeholder before there is a user.
 *
 *   (*) mkr_verified_text_t (defined above) is the one deliberate naming exception:
 *   it is a borrowed valid text AND a capability token. Its sole constructor is
 *   mkr_verified_text_from_view() at the Ruby boundary, so an unvalidated const char*
 *   cannot reach the engine's public API. Internally the engine carries the
 *   freely-constructible mkr_borrowed_text_t instead.
 *
 * Conversions — the only sanctioned edges. The points that actually VALIDATE
 * raw bytes are the bridge's checked entry points; everything else only moves
 * already-valid text between shapes (no edge re-validates, and none turns raw
 * bytes into text without one of those checks):
 *   validate raw -> valid : the bridge's checked entry points only —
 *                           mkr_ruby_verified_text / mkr_ruby_try_verified_text
 *                           (both validate UTF-8 + no NUL); never a cast.
 *   drop the GC anchor    : mkr_verified_text_from_view (ruby_borrowed_text -> verified_text)
 *   assert valid (no copy) : mkr_borrowed_text (const char*,len -> borrowed_text)
 *                            — caller asserts the bytes already meet the contract
 *   downgrade to borrow   : mkr_borrowed_text_from_owned (owned_text -> borrowed_text)
 *                           mkr_borrowed_text_from_verified (verified_text -> borrowed_text)
 *   copy into owned       : mkr_owned_text_from_borrowed_copy /
 *                           mkr_owned_text_from_buf_steal — accept only
 *                           already-asserted-valid text; they copy, not validate.
 *   take ownership        : mkr_owned_text (char*,len -> owned_text) — caller
 *                           transfers an already-valid heap buffer it produced
 *                           (substring/concat/format output); asserts validity.
 */

/* Owned bytes (raw, unchecked). */
typedef struct { char *ptr; size_t len; } mkr_owned_bytes_t;

static inline void
mkr_owned_bytes_clear(mkr_owned_bytes_t *o)
{
    free(o->ptr);
    o->ptr = NULL;
    o->len = 0;
}

/* Owned / borrowed engine text: same layout as the raw byte types, but the
 * bytes are NUL-terminated at ptr[len] and well-formed UTF-8, so character-wise
 * string code can assume valid input. Used across the XPath engine and exposed
 * in its public value type (mkr_xpath.h). */
typedef struct {
  char  *ptr; /* owned; kept NUL-terminated at ptr[len] */
  size_t len; /* bytes, excluding the terminator */
} mkr_owned_text_t;

typedef struct {
  const char *ptr; /* borrowed; owner is value/cache/AST/Lexbor/expr buffer */
  size_t      len; /* bytes, excluding the terminator */
} mkr_borrowed_text_t;

/* Borrow a slice of an owned text (no copy). */
static inline mkr_borrowed_text_t
mkr_borrowed_text_from_owned(mkr_owned_text_t t)
{
  return (mkr_borrowed_text_t){ t.ptr, t.len };
}

/* Borrow a verified-text capability token as a plain borrowed text (no copy). */
static inline mkr_borrowed_text_t
mkr_borrowed_text_from_verified(mkr_verified_text_t t)
{
  return (mkr_borrowed_text_t){ t.ptr, t.len };
}

/* Wrap an already-valid borrowed byte range (a Lexbor name, a cache entry, an
 * XPath keyword) as a borrowed text. The caller asserts the text contract; no
 * validation, no copy. The sole way to build a borrowed_text from loose
 * pointers, replacing scattered (mkr_borrowed_text_t){p,n} casts. */
static inline mkr_borrowed_text_t
mkr_borrowed_text(const char *ptr, size_t len)
{
  return (mkr_borrowed_text_t){ ptr, len };
}

/* Borrow an already-valid string literal/static char array as text. Do not pass
 * a char* variable: sizeof(pointer) would not be the byte length. */
#define mkr_borrowed_text_lit(lit) \
  mkr_borrowed_text((lit), sizeof(lit) - 1)

/* Take ownership of an already-valid, NUL-terminated heap buffer as owned text:
 * the owned analogue of mkr_borrowed_text. The caller transfers a buffer it just
 * produced (e.g. mkr_strndup / mkr_str_alloc / mkr_buf_steal output for a
 * substring/concat/format result) and asserts it meets the text contract. The
 * single place a raw owned char* becomes an mkr_owned_text_t; freed via
 * mkr_owned_text_clear (or by the mkr_val_t that receives it). */
static inline mkr_owned_text_t
mkr_owned_text(char *ptr, size_t len)
{
  return (mkr_owned_text_t){ ptr, len };
}

#ifdef __cplusplus
}
#endif

#endif /* MAKIRI_CORE_MKR_TEXT_H */
