#ifndef MAKIRI_H
#define MAKIRI_H

#include <ruby.h>
#include <ruby/encoding.h>

#include <lexbor/html/html.h>
#include <lexbor/dom/dom.h>

#include "core/mkr_safe.h" /* mkr_valid_text_t */

#ifdef __cplusplus
extern "C" {
#endif

/* Ruby module + class refs, populated by Init_makiri. */
extern VALUE mkr_mMakiri;
extern VALUE mkr_cNode;
extern VALUE mkr_cDocument;
extern VALUE mkr_cElement;
extern VALUE mkr_cAttribute;
extern VALUE mkr_cText;
extern VALUE mkr_cComment;
extern VALUE mkr_cCData;
extern VALUE mkr_cProcessingInstruction;
extern VALUE mkr_cDocumentType;
extern VALUE mkr_cDocumentFragment;
extern VALUE mkr_cNodeSet;
extern VALUE mkr_cXPathContext;
extern VALUE mkr_mXPath;
extern VALUE mkr_mCSS;
extern VALUE mkr_eError;
extern VALUE mkr_eXPathSyntaxError;
extern VALUE mkr_eXPathLimitExceeded;
extern VALUE mkr_eCSSSyntaxError;

/* Forward-declared sub-init entry points. Each sub-component owns its
 * own Init_* and registers methods on the class refs above. */
void mkr_init_document(void);
void mkr_init_node(void);
void mkr_init_node_set(void);
void mkr_init_xpath(void);
void mkr_init_css(void);
void mkr_init_serialize(void);
void mkr_init_mutate(void);

/* Enforce Makiri's text-input contract on a programmatic-API String argument:
 * it must be valid UTF-8 (bytes validated regardless of the declared encoding)
 * and must not contain a NUL byte. Raises Makiri::Error otherwise (never
 * truncates/repairs). +str+ must already be a String; +what+ names the input
 * for the message (e.g. "CSS selector"). Used at the XPath/CSS/DOM-mutation
 * boundaries; HTML parsing instead decodes leniently (see mkr_utf8_sanitize). */
void mkr_check_text(VALUE str, const char *what);

/* A validated, borrowed view of a Ruby String argument at a programmatic API
 * boundary: valid UTF-8, no NUL. `value` (the coerced String) keeps `ptr` alive
 * while the view stays on the C stack; `ptr` is NUL-terminated, so it also works
 * as a C string. Use this instead of StringValueCStr + a separate mkr_check_text. */
typedef struct {
    VALUE       value;
    const char *ptr;
    size_t      len;
} mkr_ruby_text_view_t;

/* A borrowed raw byte view of a Ruby String. This deliberately does NOT enforce
 * Makiri's strict text contract; HTML parsing consumes raw bytes and decodes
 * invalid UTF-8 leniently. */
typedef struct {
    VALUE       value;
    const char *ptr;
    size_t      len;
} mkr_ruby_bytes_view_t;

/* Coerce +in+ to a String and enforce the text-input contract, raising
 * Makiri::Error (naming +what+) on a NUL byte or invalid UTF-8. */
mkr_ruby_text_view_t mkr_ruby_checked_text(VALUE in, const char *what);

/* Coerce +in+ to a String and return its raw bytes. No UTF-8 / NUL validation.
 * The returned `ptr` BORROWS the bytes of `value` (the coerced String): it is
 * valid only while `value` is reachable. The view holds `value` so keeping the
 * view on the C stack suffices, but if a caller copies `ptr`/`value` out of the
 * view and lets the view die (e.g. returns `ptr` while only the view referenced
 * a freshly coerced String), the borrow dangles. For a buffer that must outlive
 * the source String, use mkr_ruby_copy_bytes instead. */
mkr_ruby_bytes_view_t mkr_ruby_bytes_view(VALUE in);

/* Copy a Ruby String's raw bytes into owned C storage (at least one byte even
 * for an empty input), suitable for use while the GVL is released. */
int mkr_ruby_copy_bytes(VALUE in, mkr_owned_bytes_t *out);

/* Validate a Ruby String for use as an XPath engine string: valid UTF-8,
 * no interior NUL, and at most +max_bytes+. Returns NULL on success and fills
 * +out+; otherwise returns a static reason string. +sv+ must be a String. */
const char *mkr_ruby_engine_string_view(VALUE sv, size_t max_bytes,
                                        mkr_ruby_text_view_t *out);

/* Copy an exception's #message into +buf+ without letting a broken #message
 * escape. Embedded NULs are intentionally truncated by %s formatting. */
void mkr_ruby_exception_message(VALUE exc, char *buf, size_t len);

/* The sole constructor of mkr_valid_text_t. Turns an already-validated view
 * (from mkr_ruby_checked_text or mkr_ruby_engine_string_view) into the token the
 * XPath engine accepts. Keeping this the only mint means unvalidated bytes
 * cannot reach the engine. The returned token borrows v.ptr; the caller must
 * keep v.value alive (RB_GC_GUARD) for the duration of the engine call. */
mkr_valid_text_t mkr_text_from_view(mkr_ruby_text_view_t v);

#ifdef __cplusplus
}
#endif

#endif /* MAKIRI_H */
