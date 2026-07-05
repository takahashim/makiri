#ifndef MAKIRI_BRIDGE_INTERNAL_H
#define MAKIRI_BRIDGE_INTERNAL_H

/* Bridge-private surface shared between the string-bridge TUs (ruby_string.c and
 * xml_decode.c). Deliberately NOT part of the public glue API in bridge.h: this
 * is the allocation-free core of Makiri's strict text contract, consumed both by
 * the programmatic-API validators (ruby_string.c) and by the XML input decoder
 * (xml_decode.c). */
#include <ruby.h>
#include <stddef.h>

/* Verdict of the strict text contract (no NUL byte, valid UTF-8). Each caller
 * maps a verdict to its own error surface (Makiri::Error, XML::SyntaxError, or a
 * reason string). */
typedef enum {
    MKR_TEXT_OK = 0,
    MKR_TEXT_HAS_NUL,
    MKR_TEXT_INVALID_UTF8,
} mkr_text_verdict_t;

/* Check [ptr,len) against the strict text contract, consulting +coderange_str+'s
 * CACHED coderange (no scan, no alloc) for the fast path; +ptr+/+len+ may be a
 * suffix of +coderange_str+ (see the XML BOM-strip case). ALLOCATION-FREE BY
 * DESIGN - it runs between a caller taking a borrowed RSTRING pointer and using
 * it, so it must not be a GC point. Defined in ruby_string.c. */
mkr_text_verdict_t mkr_text_check(VALUE coderange_str, const char *ptr, size_t len);

#endif /* MAKIRI_BRIDGE_INTERNAL_H */
