#ifndef MAKIRI_BRIDGE_H
#define MAKIRI_BRIDGE_H

#include <ruby.h>
#include <ruby/encoding.h>

#include "../core/mkr_safe.h" /* mkr_valid_text_t, mkr_owned_bytes_t */

#ifdef __cplusplus
extern "C" {
#endif

/* Enforce Makiri's text-input contract on a programmatic-API String argument:
 * it must be valid UTF-8 (bytes validated regardless of the declared encoding)
 * and must not contain a NUL byte. Raises Makiri::Error otherwise. */
void mkr_check_text(VALUE str, const char *what);

/* A validated, borrowed view of a Ruby String argument at a programmatic API
 * boundary: valid UTF-8, no NUL. `value` keeps `ptr` alive while the view stays
 * on the C stack; `ptr` is NUL-terminated, so it also works as a C string. */
typedef struct {
    VALUE       value;
    const char *ptr;
    size_t      len;
} mkr_ruby_borrowed_text_t;

/* A borrowed raw byte view of a Ruby String. This deliberately does NOT enforce
 * Makiri's strict text contract; HTML parsing consumes raw bytes and decodes
 * invalid UTF-8 leniently. */
typedef struct {
    VALUE       value;
    const char *ptr;
    size_t      len;
} mkr_ruby_borrowed_bytes_t;

/* Coerce +in+ to a String and enforce the text-input contract, raising
 * Makiri::Error (naming +what+) on a NUL byte or invalid UTF-8. */
mkr_ruby_borrowed_text_t mkr_ruby_checked_text(VALUE in, const char *what);

/* Coerce +in+ to a String and return its raw bytes. No UTF-8 / NUL validation.
 * The returned `ptr` borrows the bytes of `value`; use mkr_ruby_copy_bytes when
 * the buffer must outlive the source String. */
mkr_ruby_borrowed_bytes_t mkr_ruby_bytes_view(VALUE in);

/* Copy a Ruby String's raw bytes into owned C storage (at least one byte even
 * for an empty input), suitable for use while the GVL is released. */
int mkr_ruby_copy_bytes(VALUE in, mkr_owned_bytes_t *out);

/* Validate a Ruby String for use as an XPath engine string: valid UTF-8,
 * no interior NUL, and at most +max_bytes+. Returns NULL on success and fills
 * +out+; otherwise returns a static reason string. +sv+ must be a String. */
const char *mkr_ruby_engine_string_view(VALUE sv, size_t max_bytes,
                                        mkr_ruby_borrowed_text_t *out);

/* Copy an exception's #message into +buf+ without letting a broken #message
 * escape. Embedded NULs are intentionally truncated by %s formatting. */
void mkr_ruby_exception_message(VALUE exc, char *buf, size_t len);

/* The sole constructor of mkr_valid_text_t. Turns an already-validated view
 * (from mkr_ruby_checked_text or mkr_ruby_engine_string_view) into the token the
 * XPath engine accepts. The returned token borrows v.ptr; the caller must keep
 * v.value alive for the duration of the engine call. */
mkr_valid_text_t mkr_text_from_view(mkr_ruby_borrowed_text_t v);

/* Assemble a UTF-8 Ruby String of exactly +total+ bytes by concatenating +n+
 * borrowed slices (their lengths must sum to +total+). The output-side analogue
 * of the input helpers: the sole place an assembled result is written through
 * RSTRING_PTR. Used by Node#text's text-index fast path. */
VALUE mkr_ruby_str_from_slices(const mkr_borrowed_text_t *slices, size_t n,
                               size_t total);

#ifdef __cplusplus
}
#endif

#endif /* MAKIRI_BRIDGE_H */
