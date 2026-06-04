/* mkr_xml_chars.c — XML Char validation + character/entity reference expansion.
 *
 * Ruby-free. §9.1: only the 5 predefined entities (lt gt amp apos quot) and
 * numeric character references (&#nn; / &#xhh;) are expanded; any other &name;
 * is an undefined entity (SyntaxError) — there is no DTD, so no entity can be
 * defined, which makes billion-laughs structurally impossible. §9.2: every
 * resulting codepoint (literal or from a numeric reference) must be an XML 1.0
 * Char; control characters / surrogates / out-of-range are rejected. The input
 * is already valid UTF-8 (§2.1).
 */
#include "mkr_xml.h"
#include "mkr_xml_node.h"

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

/* Decode one codepoint from UTF-8. STRICT (self-contained, not trusting the
 * caller): rejects truncation, bad continuation bytes, overlong encodings,
 * surrogates and out-of-range values. Returns the byte length (1-4) or 0 on any
 * violation — fail closed, even if some future caller feeds unvalidated bytes. */
static int
is_cont(const char *p, const char *end)
{
    return p < end && ((unsigned char)*p & 0xC0u) == 0x80u;
}

/* forward decl: utf8_decode is defined below but mkr_xml_validate_chars uses it. */
static int utf8_decode(const char *p, const char *end, uint32_t *cp);

/* Validate that [src, src+len) is entirely XML 1.0 Char, with NO entity/reference
 * recognition (for comment/CDATA/PI content, where '&' and '<' are literal). 0 if
 * all valid, -1 on the first malformed UTF-8 or non-Char (caller raises SYNTAX). */
int
mkr_xml_validate_chars(const char *src, uint32_t len)
{
    const char *p = src, *end = src + len;
    while (p < end) {
        uint32_t cp;
        int bl = utf8_decode(p, end, &cp);
        if (bl == 0 || !mkr_xml_is_char(cp)) return -1;
        p += bl;
    }
    return 0;
}

static int
utf8_decode(const char *p, const char *end, uint32_t *cp)
{
    unsigned char c = (unsigned char)p[0];
    if (c < 0x80u) { *cp = c; return 1; }
    if ((c & 0xE0u) == 0xC0u) {
        if (!is_cont(p + 1, end)) return 0;
        uint32_t v = ((uint32_t)(c & 0x1Fu) << 6) | ((unsigned char)p[1] & 0x3Fu);
        if (v < 0x80u) return 0;                                /* overlong */
        *cp = v; return 2;
    }
    if ((c & 0xF0u) == 0xE0u) {
        if (!is_cont(p + 1, end) || !is_cont(p + 2, end)) return 0;
        uint32_t v = ((uint32_t)(c & 0x0Fu) << 12) | (((unsigned char)p[1] & 0x3Fu) << 6)
            | ((unsigned char)p[2] & 0x3Fu);
        if (v < 0x800u) return 0;                               /* overlong */
        if (v >= 0xD800u && v <= 0xDFFFu) return 0;             /* surrogate */
        *cp = v; return 3;
    }
    if ((c & 0xF8u) == 0xF0u) {
        if (!is_cont(p + 1, end) || !is_cont(p + 2, end) || !is_cont(p + 3, end)) return 0;
        uint32_t v = ((uint32_t)(c & 0x07u) << 18) | (((unsigned char)p[1] & 0x3Fu) << 12)
            | (((unsigned char)p[2] & 0x3Fu) << 6) | ((unsigned char)p[3] & 0x3Fu);
        if (v < 0x10000u || v > 0x10FFFFu) return 0;            /* overlong / out of range */
        *cp = v; return 4;
    }
    return 0;
}

/* Public, bounds-checked one-codepoint decode for the tokenizer's name scanning
 * (returns 0 at end-of-input as well as on any malformed byte). */
int
mkr_xml_utf8_decode(const char *p, const char *end, uint32_t *cp)
{
    if (p >= end) return 0;
    return utf8_decode(p, end, cp);
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
 * bytes), so a single arena buffer of `len` bytes suffices. Returns the arena
 * slice and sets *out_len; returns NULL on an undefined entity / bad reference /
 * non-XML-Char (sets *status = MKR_XML_ERR_SYNTAX) or arena OOM/LIMIT. */
const char *
mkr_xml_expand(mkr_xml_doc_t *doc, const char *src, uint32_t len,
               mkr_xml_expand_mode_t mode, uint32_t *out_len, mkr_xml_status_t *status)
{
    if (len == 0) { *out_len = 0; return ""; }

    char *buf = mkr_xml_arena_scratch_bytes(doc, len);
    if (buf == NULL) { *status = doc->oom; return NULL; }

    size_t      o   = 0;
    const char *p   = src;
    const char *end = src + len;

    while (p < end) {
        if (*p != '&') {
            uint32_t cp;
            int bl = utf8_decode(p, end, &cp);
            if (bl == 0 || !mkr_xml_is_char(cp)) { *status = MKR_XML_ERR_SYNTAX; return NULL; }
            /* §3.3.3: in an attribute value a *literal* TAB/LF/CR normalizes to a
             * space (reference-derived whitespace is preserved — see below). */
            if (mode == MKR_XML_EXPAND_ATTR
                && (cp == 0x9u || cp == 0xAu || cp == 0xDu)) {
                buf[o++] = ' ';
                p += bl;
                continue;
            }
            memcpy(buf + o, p, (size_t)bl);
            o += (size_t)bl;
            p += bl;
            continue;
        }

        /* a reference: '&' ... ';' */
        p++; /* past '&' */
        if (p < end && *p == '#') {                 /* numeric character reference */
            p++;
            int hex = 0;
            if (p < end && (*p == 'x' || *p == 'X')) { hex = 1; p++; }
            const char *digits = p;
            uint32_t base = hex ? 16u : 10u;
            uint32_t cp = 0;
            while (p < end && *p != ';') {
                unsigned char d = (unsigned char)*p;
                uint32_t dig;
                if (d >= '0' && d <= '9')              dig = (uint32_t)(d - '0');
                else if (hex && d >= 'a' && d <= 'f')  dig = (uint32_t)(d - 'a' + 10);
                else if (hex && d >= 'A' && d <= 'F')  dig = (uint32_t)(d - 'A' + 10);
                else { *status = MKR_XML_ERR_SYNTAX; return NULL; }
                /* check BEFORE the multiply-add so a giant &#999…; can never wrap
                 * uint32_t into the valid range and be falsely accepted. */
                if (cp > (0x10FFFFu - dig) / base) { *status = MKR_XML_ERR_SYNTAX; return NULL; }
                cp = cp * base + dig;
                p++;
            }
            if (p >= end || p == digits) { *status = MKR_XML_ERR_SYNTAX; return NULL; } /* no ';' / no digits */
            p++; /* past ';' */
            if (!mkr_xml_is_char(cp)) { *status = MKR_XML_ERR_SYNTAX; return NULL; }
            o += (size_t)utf8_encode(cp, buf + o);
        } else {                                    /* named entity */
            const char *ns = p;
            while (p < end && *p != ';') p++;
            if (p >= end) { *status = MKR_XML_ERR_SYNTAX; return NULL; } /* unterminated */
            size_t nlen = (size_t)(p - ns);
            p++; /* past ';' */
            char ch;
            if      (nlen == 2 && memcmp(ns, "lt",   2) == 0) ch = '<';
            else if (nlen == 2 && memcmp(ns, "gt",   2) == 0) ch = '>';
            else if (nlen == 3 && memcmp(ns, "amp",  3) == 0) ch = '&';
            else if (nlen == 4 && memcmp(ns, "apos", 4) == 0) ch = '\'';
            else if (nlen == 4 && memcmp(ns, "quot", 4) == 0) ch = '"';
            else { *status = MKR_XML_ERR_SYNTAX; return NULL; } /* undefined entity */
            buf[o++] = ch;
        }
    }

    *out_len = (uint32_t)o;
    return buf;
}
