#ifndef MAKIRI_BRIDGE_H
#define MAKIRI_BRIDGE_H

#include <ruby.h>
#include <ruby/encoding.h>

#include "../core/mkr_core.h" /* mkr_verified_text_t, mkr_owned_bytes_t */

#ifdef __cplusplus
extern "C" {
#endif

/* Enforce Makiri's text-input contract on a programmatic-API String argument:
 * it must be valid UTF-8 (bytes validated regardless of the declared encoding)
 * and must not contain a NUL byte. Raises Makiri::Error otherwise. */
void mkr_verify_text(VALUE str, const char *what);

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
mkr_ruby_borrowed_text_t mkr_ruby_verified_text(VALUE in, const char *what);

/* Coerce +in+ to a String and return its raw bytes. No UTF-8 / NUL validation.
 * The returned `ptr` borrows the bytes of `value`; use mkr_ruby_copy_bytes when
 * the buffer must outlive the source String. */
mkr_ruby_borrowed_bytes_t mkr_ruby_bytes_view(VALUE in);

/* Copy a Ruby String's raw bytes into owned C storage (at least one byte even
 * for an empty input), suitable for use while the GVL is released. */
int mkr_ruby_copy_bytes(VALUE in, mkr_owned_bytes_t *out);

/* Return a UTF-8 Ruby String for `str`, honouring its declared encoding: UTF-8 /
 * US-ASCII / ASCII-8BIT are returned unchanged (the parser handles their bytes
 * directly); any other encoding is transcoded to UTF-8 (invalid/undef -> U+FFFD)
 * so its content is preserved rather than read as raw UTF-8. The UTF-8 common
 * case is a single encoding comparison. */
VALUE mkr_ruby_to_utf8(VALUE str);

/* STRICT decode for XML (§2.1): like mkr_ruby_to_utf8 it honours the String's
 * declared encoding (UTF-8 / US-ASCII / ASCII-8BIT pass through; any other
 * encoding is transcoded to UTF-8) - but FAIL-CLOSED, never lenient: a non-UTF-8
 * byte that can't be converted, invalid UTF-8, or an embedded NUL all raise
 * Makiri::XML::SyntaxError (no U+FFFD replacement). Returns a validated,
 * UTF-8-tagged Ruby String. (The HTML replace path mkr_ruby_to_utf8 itself is
 * NOT reused for the conversion - only its encoding-judgment rule is shared.)
 *
 * +max_bytes+ bounds the decoded UTF-8 length: an input that already exceeds the
 * parser's arena byte budget is rejected here with Makiri::XML::LimitExceeded,
 * before the validation copy and the caller's GVL-release copy (so a hostile
 * oversized document is not copied twice for a doomed parse). 0 disables the
 * check (decode-only callers that build no arena). */
VALUE mkr_xml_decode_input(VALUE str, size_t max_bytes);

/* True if `str` is *already known* to be valid UTF-8 - pure ASCII, or valid in
 * the UTF-8 encoding - from its cached coderange, WITHOUT forcing a scan. Lets
 * the parse skip mkr_utf8_sanitize's validation pass for input Ruby has already
 * classified (an unknown/broken coderange returns false: sanitize handles it). */
bool mkr_ruby_str_known_valid_utf8(VALUE str);

/* Validate a Ruby String for use as an XPath engine string: valid UTF-8,
 * no interior NUL, and at most +max_bytes+. Returns NULL on success and fills
 * +out+; otherwise returns a static reason string. +sv+ must be a String. */
const char *mkr_ruby_try_verified_text(VALUE sv, size_t max_bytes,
                                        mkr_ruby_borrowed_text_t *out);

/* Copy an exception's #message into +buf+ without letting a broken #message
 * escape. Embedded NULs are intentionally truncated by %s formatting. */
void mkr_ruby_exception_message(VALUE exc, char *buf, size_t len);

/* The sole constructor of mkr_verified_text_t. Turns an already-validated view
 * (from mkr_ruby_verified_text or mkr_ruby_try_verified_text) into the token the
 * XPath engine accepts. The returned token borrows v.ptr; the caller must keep
 * v.value alive for the duration of the engine call. */
mkr_verified_text_t mkr_verified_text_from_view(mkr_ruby_borrowed_text_t v);

/* Assemble a UTF-8 Ruby String of exactly +total+ bytes by concatenating +n+
 * borrowed slices (their lengths must sum to +total+). The output-side analogue
 * of the input helpers: the sole place an assembled result is written through
 * RSTRING_PTR. Used by Node#text's text-index fast path. */
VALUE mkr_ruby_str_from_slices(const mkr_borrowed_text_t *slices, size_t n,
                               size_t total);

/* Build a UTF-8 Ruby String by copying a single borrowed valid-text view (the
 * single-slice sibling of mkr_ruby_str_from_slices). A NULL/empty view (the
 * mkr_borrowed_text(NULL, 0) "absent" sentinel) yields "". Used by the DOM
 * text/name/attribute readers that turn a Lexbor arena slice into a String. */
VALUE mkr_ruby_str_from_borrowed(mkr_borrowed_text_t text);

#ifdef __cplusplus
}
#endif

#endif /* MAKIRI_BRIDGE_H */
