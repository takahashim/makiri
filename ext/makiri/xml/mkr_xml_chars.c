/* mkr_xml_chars.c - XML Char validation + character/entity reference expansion.
 *
 * Ruby-free. §9.1: only the 5 predefined entities (lt gt amp apos quot) and
 * numeric character references (&#nn; / &#xhh;) are expanded; any other &name;
 * is an undefined entity (SyntaxError) - there is no DTD, so no entity can be
 * defined, which makes billion-laughs structurally impossible. §9.2: every
 * resulting codepoint (literal or from a numeric reference) must be an XML 1.0
 * Char; control characters / surrogates / out-of-range are rejected. The input
 * is already valid UTF-8 (§2.1).
 *
 * All input reads go through the bounded reader (core mkr_span_t) and the
 * strict core decoder (mkr_utf8_decode1) - an out-of-bounds read is
 * structurally impossible, not a per-site convention; all output goes through
 * the bounded writer (mkr_spanbuf_t). The lint (raw_scan_call /
 * raw_cursor_member) keeps it that way.
 */
#include "mkr_xml.h"
#include "mkr_xml_node.h"
#include "../core/mkr_core.h"   /* mkr_span_t + mkr_utf8_decode1 */

#include <string.h>

int
mkr_xml_is_char(uint32_t c)
{
    /* XML 1.0 §2.2 Char: #x9 | #xA | #xD | [#x20-#xD7FF] |
     *                     [#xE000-#xFFFD] | [#x10000-#x10FFFF] */
    return c == 0x9u || c == 0xAu || c == 0xDu
        || (c >= 0x20u    && c <= 0xD7FFu)
        || (c >= 0xE000u  && c <= 0xFFFDu)
        || (c >= 0x10000u && c <= 0x10FFFFu);
}

/* Validate that [src, src+len) is entirely XML 1.0 Char, with NO entity/reference
 * recognition (for comment/CDATA/PI content, where '&' and '<' are literal). 0 if
 * all valid, -1 on the first malformed UTF-8 or non-Char (caller raises SYNTAX).
 * The strict decode (truncation / bad continuation / overlong / surrogate /
 * out-of-range -> 0) is the core mkr_utf8_decode1 - self-contained, not trusting
 * the caller, even if some future caller feeds unvalidated bytes. */
int
mkr_xml_validate_chars(const char *src, uint32_t len)
{
    mkr_span_t s = mkr_span(src, len);
    while (mkr_span_left(&s) > 0) {
        uint32_t cp;
        int bl = mkr_utf8_decode1_span(&s, &cp);
        if (bl == 0 || !mkr_xml_is_char(cp)) return -1;
        mkr_span_skip(&s, (size_t)bl);
    }
    return 0;
}

/* XML 1.0 §2.3 NameStartChar / NameChar (the full Unicode sets, not just ASCII).
 * Element/attribute QNames and PI targets are validated against these so an
 * ill-formed name never reaches the DOM (§9.2b). */
int
mkr_xml_is_name_start(uint32_t c)
{
    return c == ':' || (c >= 'A' && c <= 'Z') || c == '_' || (c >= 'a' && c <= 'z')
        || (c >= 0xC0u    && c <= 0xD6u)    || (c >= 0xD8u   && c <= 0xF6u)
        || (c >= 0xF8u    && c <= 0x2FFu)   || (c >= 0x370u  && c <= 0x37Du)
        || (c >= 0x37Fu   && c <= 0x1FFFu)  || (c >= 0x200Cu && c <= 0x200Du)
        || (c >= 0x2070u  && c <= 0x218Fu)  || (c >= 0x2C00u && c <= 0x2FEFu)
        || (c >= 0x3001u  && c <= 0xD7FFu)  || (c >= 0xF900u && c <= 0xFDCFu)
        || (c >= 0xFDF0u  && c <= 0xFFFDu)  || (c >= 0x10000u && c <= 0xEFFFFu);
}

int
mkr_xml_is_name_char(uint32_t c)
{
    return mkr_xml_is_name_start(c) || c == '-' || c == '.' || (c >= '0' && c <= '9')
        || c == 0xB7u || (c >= 0x300u && c <= 0x36Fu) || (c >= 0x203Fu && c <= 0x2040u);
}

/* Validate that [src, src+len) is a well-formed XML 1.0 Name (§2.3): a
 * NameStartChar followed by NameChar*. Unlike an NCName a Name MAY contain a
 * colon (':' is a NameStartChar), so this is the right check for a PITarget,
 * which is a Name (§2.6), not an NCName. 0 if a Name, -1 otherwise (empty or
 * malformed UTF-8 / non-Name codepoint). */
int
mkr_xml_validate_name(const char *src, uint32_t len)
{
    if (len == 0) return -1;
    mkr_span_t s = mkr_span(src, len);
    uint32_t cp;
    int bl = mkr_utf8_decode1_span(&s, &cp);
    if (bl == 0 || !mkr_xml_is_name_start(cp)) return -1;
    mkr_span_skip(&s, (size_t)bl);
    while (mkr_span_left(&s) > 0) {
        bl = mkr_utf8_decode1_span(&s, &cp);
        if (bl == 0 || !mkr_xml_is_name_char(cp)) return -1;
        mkr_span_skip(&s, (size_t)bl);
    }
    return 0;
}

int
mkr_xml_is_reserved_pi_target(const char *s, uint32_t len)
{
    if (len != 3) return 0;
    mkr_span_t v = mkr_span(s, len);
    return (mkr_span_at(&v, 0) | 0x20) == 'x'
        && (mkr_span_at(&v, 1) | 0x20) == 'm'
        && (mkr_span_at(&v, 2) | 0x20) == 'l';
}

static int
utf8_encode(uint32_t cp, char *out)
{
    if (cp < 0x80u)    { out[0] = (char)cp; return 1; }
    if (cp < 0x800u)   { out[0] = (char)(0xC0u | (cp >> 6));
                         out[1] = (char)(0x80u | (cp & 0x3Fu)); return 2; }
    if (cp < 0x10000u) { out[0] = (char)(0xE0u | (cp >> 12));
                         out[1] = (char)(0x80u | ((cp >> 6) & 0x3Fu));
                         out[2] = (char)(0x80u | (cp & 0x3Fu)); return 3; }
    out[0] = (char)(0xF0u | (cp >> 18));
    out[1] = (char)(0x80u | ((cp >> 12) & 0x3Fu));
    out[2] = (char)(0x80u | ((cp >> 6) & 0x3Fu));
    out[3] = (char)(0x80u | (cp & 0x3Fu));
    return 4;
}

/* Expand references in [src, src+len) and validate XML Char. Output is never
 * longer than the input (every &...; reference is >= 4 chars and yields <= 4
 * bytes), so a buffer of `len` bytes suffices - and the bounded arena writer
 * (mkr_xml_arena_spanbuf + core mkr_spanbuf) enforces that bound rather than
 * trusting it (a write past it fails closed instead of overrunning the arena).
 * Returns the arena slice and sets *out_len;
 * returns NULL on an undefined entity / bad reference / non-XML-Char (sets
 * *status = MKR_XML_ERR_SYNTAX) or arena OOM/LIMIT. */
const char *
mkr_xml_expand(mkr_xml_doc_t *doc, const char *src, uint32_t len,
               mkr_xml_expand_mode_t mode, uint32_t *out_len, mkr_xml_status_t *status)
{
    /* Fail-closed contract guards: this is a public primitive (mkr_xml.h), so it
     * must not dereference a NULL output or document even though the in-tree
     * callers always pass real ones. out_len/status are required outputs (a NULL
     * leaves no channel to report through -> return NULL); a len>0 expansion
     * needs a real doc + source. All are programming errors, never recoverable
     * input. */
    if (out_len == NULL || status == NULL) return NULL;
    if (len == 0) { *out_len = 0; return ""; }
    if (doc == NULL || src == NULL) { *status = MKR_XML_ERR_INTERNAL; return NULL; }

    /* All output goes through the bounded writer, so no code below can overrun
     * the buffer - a write that would exceed `len` is refused and latches ok=0. */
    mkr_spanbuf_t b = mkr_xml_arena_spanbuf(doc, len);
    if (!b.ok) { *status = doc->oom; return NULL; } /* backing alloc failed */
    mkr_span_t s = mkr_span(src, len);

    while (mkr_span_left(&s) > 0) {
        if (mkr_span_peek(&s) != '&') {
            uint32_t cp;
            int bl = mkr_utf8_decode1_span(&s, &cp);
            if (bl == 0 || !mkr_xml_is_char(cp)) { *status = MKR_XML_ERR_SYNTAX; return NULL; }
            /* §3.3.3: in an attribute value a *literal* TAB/LF/CR normalizes to a
             * space (reference-derived whitespace is preserved - see below). */
            if (mode == MKR_XML_EXPAND_ATTR
                && (cp == 0x9u || cp == 0xAu || cp == 0xDu)) {
                mkr_spanbuf_putc(&b, ' ');
                mkr_span_skip(&s, (size_t)bl);
                continue;
            }
            mkr_spanbuf_write(&b, mkr_span_mark(&s), (size_t)bl);
            mkr_span_skip(&s, (size_t)bl);
            continue;
        }

        /* a reference: '&' ... ';' */
        mkr_span_skip(&s, 1); /* past '&' */
        if (mkr_span_peek(&s) == '#') {             /* numeric character reference */
            mkr_span_skip(&s, 1);
            int hex = 0;
            /* §4.1: the hex marker is a lowercase 'x' only - "&#X58;" is not-wf
             * (an uppercase 'X' is not a decimal digit either, so it is rejected
             * as "no digits" below). */
            if (mkr_span_peek(&s) == 'x') { hex = 1; mkr_span_skip(&s, 1); }
            uint32_t base = hex ? 16u : 10u;
            uint32_t cp = 0;
            int ndigits = 0;
            for (;;) {
                int d = mkr_span_peek(&s);
                if (d < 0 || d == ';') break;
                uint32_t dig;
                if (d >= '0' && d <= '9')              dig = (uint32_t)(d - '0');
                else if (hex && d >= 'a' && d <= 'f')  dig = (uint32_t)(d - 'a' + 10);
                else if (hex && d >= 'A' && d <= 'F')  dig = (uint32_t)(d - 'A' + 10);
                else { *status = MKR_XML_ERR_SYNTAX; return NULL; }
                /* check BEFORE the multiply-add so a giant &#999...; can never wrap
                 * uint32_t into the valid range and be falsely accepted. */
                if (cp > (0x10FFFFu - dig) / base) { *status = MKR_XML_ERR_SYNTAX; return NULL; }
                cp = cp * base + dig;
                ndigits++;
                mkr_span_skip(&s, 1);
            }
            if (mkr_span_peek(&s) != ';' || ndigits == 0) { *status = MKR_XML_ERR_SYNTAX; return NULL; } /* no ';' / no digits */
            mkr_span_skip(&s, 1); /* past ';' */
            if (!mkr_xml_is_char(cp)) { *status = MKR_XML_ERR_SYNTAX; return NULL; }
            /* encode into a safe 4-byte local, then hand the exact length to the
             * bounded writer (the encode itself can never overrun `enc`). */
            char enc[4];
            mkr_spanbuf_write(&b, enc, (size_t)utf8_encode(cp, enc));
        } else {                                    /* named entity */
            size_t nlen;
            if (!mkr_span_find(&s, ';', &nlen)) { *status = MKR_XML_ERR_SYNTAX; return NULL; } /* unterminated */
            const char *ns = mkr_span_mark(&s);
            mkr_span_skip(&s, nlen + 1); /* name + ';' */
            char ch;
            if      (mkr_bytes_eq(ns, nlen, "lt",   2)) ch = '<';
            else if (mkr_bytes_eq(ns, nlen, "gt",   2)) ch = '>';
            else if (mkr_bytes_eq(ns, nlen, "amp",  3)) ch = '&';
            else if (mkr_bytes_eq(ns, nlen, "apos", 4)) ch = '\'';
            else if (mkr_bytes_eq(ns, nlen, "quot", 4)) ch = '"';
            else { *status = MKR_XML_ERR_SYNTAX; return NULL; } /* undefined entity */
            mkr_spanbuf_putc(&b, ch);
        }
    }

    /* By construction the buffer was never overrun; finish returns NULL if a
     * write was refused (the "output <= input" invariant broke - our bug). The
     * backing alloc was already checked at init, so a NULL here means a refused
     * write: fail closed rather than return a truncated expansion. */
    const char *out = mkr_spanbuf_finish(&b);
    if (out == NULL) { *status = MKR_XML_ERR_INTERNAL; return NULL; }
    *out_len = (uint32_t)b.pos;
    return out;
}
