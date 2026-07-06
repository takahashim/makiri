/* xml_decode.c - XML 1.0 Appendix F byte-encoding autodetection and strict
 * decode-to-UTF-8 for the XML reader. Part of the bridge layer, split out of
 * ruby_string.c so this XML-specific charset subsystem is separate from the
 * generic Ruby String helpers. Raw RSTRING access stays isolated in
 * ruby_string.c; this file borrows bytes only through mkr_ruby_bytes_view.
 * Shares the allocation-free strict-text-contract core (mkr_text_check) via
 * bridge_internal.h. See mkr_xml_decode_input in bridge.h for the contract. */
#include "bridge.h"
#include "../makiri.h"
#include "bridge_internal.h"

#include <string.h>

/* rb_str_encode with no replacement flags: an undefined conversion or invalid
 * byte sequence RAISES (Encoding::UndefinedConversionError /
 * Encoding::InvalidByteSequenceError) instead of substituting U+FFFD. Run under
 * rb_protect so we can remap the Ruby Encoding error to Makiri::XML::SyntaxError. */
static VALUE
mkr_xml_strict_transcode_thunk(VALUE str)
{
    return rb_str_encode(str, rb_enc_from_encoding(rb_utf8_encoding()), 0, Qnil);
}

/* --- XML 1.0 Appendix F: byte-encoding autodetection (BOM, then declaration) ---
 *
 * The leading byte-order mark, or NULL; *bom_len gets its length. UTF-32 BOMs are
 * checked before the UTF-16 LE BOM they share a prefix with.
 *
 * *stride / *ascii_off get the interleave geometry of the ASCII column the decl
 * scanner later extracts (default 1/0 for a single-byte stream). It is resolved
 * HERE, at the match, rather than re-derived downstream, because that derivation
 * needs rb_enc_find (it can autoload an encoding = a GC point) and the decl
 * scanner reads a borrowed RSTRING view that must not be held across one - so
 * the scanner is kept allocation-free until its reads are done. Each span read
 * of p still finishes before the rb_enc_find in the return. */
static rb_encoding *
mkr_xml_bom_encoding(const unsigned char *p, long len, long *bom_len, long *stride, long *ascii_off)
{
    mkr_span_t s = mkr_span((const char *)p, (size_t)len);
    *bom_len = 0;
    *stride = 1;
    *ascii_off = 0;
    if (mkr_span_starts(&s, "\x00\x00\xFE\xFF", 4)) { *bom_len = 4; *stride = 4; *ascii_off = 3; return rb_enc_find("UTF-32BE"); }
    if (mkr_span_starts(&s, "\xFF\xFE\x00\x00", 4)) { *bom_len = 4; *stride = 4; *ascii_off = 0; return rb_enc_find("UTF-32LE"); }
    if (mkr_span_starts(&s, "\xFE\xFF", 2)) { *bom_len = 2; *stride = 2; *ascii_off = 1; return rb_enc_find("UTF-16BE"); }
    if (mkr_span_starts(&s, "\xFF\xFE", 2)) { *bom_len = 2; *stride = 2; *ascii_off = 0; return rb_enc_find("UTF-16LE"); }
    if (mkr_span_starts(&s, "\xEF\xBB\xBF", 3)) { *bom_len = 3; return rb_utf8_encoding(); }
    return NULL;
}

static int
mkr_decl_ws(int c)
{
    return c == ' ' || c == '\t' || c == '\r' || c == '\n';
}

/* The encoding named in the '<?xml ... encoding="NAME" ?>' declaration, or NULL.
 * The declaration is ASCII; for a UTF-16/32-detected document its bytes are
 * stride-interleaved, so the ASCII column is extracted (stride/off resolved by
 * the BOM matcher) before the scan, letting a BOM-vs-declaration conflict be
 * caught even in UTF-16.
 *
 * p is a borrowed RSTRING view, so this stays allocation-free until every read
 * of p is done: the stride/off geometry is passed in (rather than derived here
 * via rb_enc_find, which can autoload = a GC point), and the only rb_enc_find -
 * the final name lookup - runs after the bytes have been copied into head[]. */
static rb_encoding *
mkr_xml_decl_encoding(const unsigned char *p, long len, long stride, long off)
{
    /* Extract the ASCII column (per the BOM stride) through bounded reads into
     * a bounded writer - neither side trusts the loop arithmetic. */
    mkr_span_t in = mkr_span((const char *)p, len < 0 ? 0 : (size_t)len);
    char head[256];
    mkr_spanbuf_t hw = mkr_spanbuf(head, sizeof(head));
    for (size_t i = (size_t)off; hw.pos < sizeof(head); i += (size_t)stride) {
        int c = mkr_span_at(&in, i);
        if (c < 0) break;
        mkr_spanbuf_putc(&hw, (char)c);
    }
    mkr_span_t h = mkr_span(head, hw.pos);
    size_t hn = hw.pos;

    size_t i = 0;
    while (mkr_decl_ws(mkr_span_at(&h, i))) i++;
    {
        mkr_span_t t = mkr_span_tail(&h, i);
        if (!mkr_span_starts(&t, "<?xml", 5)) return NULL;
    }
    i += 5;
    /* find a whitespace-introduced "encoding" before the '?>' */
    for (; i + 8 <= hn; i++) {
        if (mkr_span_at(&h, i) == '?' && mkr_span_at(&h, i + 1) == '>') return NULL; /* end of decl */
        mkr_span_t t = mkr_span_tail(&h, i);
        if (!mkr_decl_ws(mkr_span_at(&h, i - 1)) || !mkr_span_starts(&t, "encoding", 8)) continue;
        size_t j = i + 8;
        while (mkr_decl_ws(mkr_span_at(&h, j))) j++;
        if (mkr_span_at(&h, j) != '=') return NULL;
        j++;
        while (mkr_decl_ws(mkr_span_at(&h, j))) j++;
        int q = mkr_span_at(&h, j);
        if (q != '"' && q != '\'') return NULL;
        j++;
        size_t ns = j;
        while (mkr_span_at(&h, j) >= 0 && mkr_span_at(&h, j) != q) j++;
        if (j >= hn) return NULL;
        char name[64];
        size_t nl = j - ns;
        if (nl == 0 || nl >= sizeof(name)) return NULL;
        memcpy(name, head + ns, nl);
        name[nl] = '\0';
        return rb_enc_find(name);   /* NULL for an unknown encoding name */
    }
    return NULL;
}

/* Two encodings agree for conflict purposes when identical, or when either is
 * US-ASCII (a subset of UTF-8 and the single-byte encodings). */
static int
mkr_xml_enc_compatible(rb_encoding *a, rb_encoding *b)
{
    return a == b || a == rb_usascii_encoding() || b == rb_usascii_encoding();
}

/* Phase 1: resolve the input's effective byte encoding (XML 1.0 Appendix F). A
 * BOM wins, else the '<?xml encoding=?>' declaration, else the String's own
 * declared encoding; ASCII-8BIT ("raw bytes, no claimed encoding") is decoded by
 * the detected encoding instead. Raises Makiri::XML::SyntaxError on any
 * BOM/declaration/String-encoding conflict, so the caller only ever sees a
 * single, self-consistent encoding. Reads borrowed RSTRING bytes but stays
 * allocation-free until its reads are done (see the BOM/decl scanners). */
static rb_encoding *
mkr_xml_effective_encoding(VALUE str)
{
    rb_encoding              *tag = rb_enc_get(str);
    mkr_ruby_borrowed_bytes_t v   = mkr_ruby_bytes_view(str);
    const unsigned char      *raw = (const unsigned char *)v.ptr;
    long                      rawlen = (long)v.len;

    long bom_len = 0, bom_stride = 1, bom_off = 0;
    rb_encoding *bom = mkr_xml_bom_encoding(raw, rawlen, &bom_len, &bom_stride, &bom_off);
    /* rb_enc_find inside the BOM lookup can autoload an encoding (a Ruby
     * allocation = a GC point), so re-borrow the bytes before reading them
     * again - a borrowed view must not be held across one. The interleave
     * geometry (stride/off) is resolved by the BOM matcher and passed through,
     * keeping the decl scanner itself allocation-free. */
    v   = mkr_ruby_bytes_view(str);
    raw = (const unsigned char *)v.ptr;
    rb_encoding *decl = mkr_xml_decl_encoding(raw + bom_len, rawlen - bom_len, bom_stride, bom_off);
    int is_binary = (tag == rb_ascii8bit_encoding());

    if (bom && decl && !mkr_xml_enc_compatible(bom, decl)) {
        rb_raise(mkr_eXmlSyntaxError,
                 "XML encoding conflict: the byte-order mark and the encoding declaration disagree");
    }
    if (!is_binary && bom && !mkr_xml_enc_compatible(bom, tag)) {
        rb_raise(mkr_eXmlSyntaxError,
                 "XML encoding conflict: the byte-order mark disagrees with the string's encoding");
    }
    if (!is_binary && decl && !mkr_xml_enc_compatible(decl, tag)) {
        /* A concrete String encoding is authoritative for decoding, so the
         * declaration is not used to transcode - but a declaration that names a
         * different encoding than the String is tagged with (e.g. a Shift_JIS
         * String declaring encoding="UTF-8") is a self-inconsistent document and
         * a fatal error, not a silently-ignored mismatch. */
        rb_raise(mkr_eXmlSyntaxError,
                 "XML encoding conflict: the encoding declaration disagrees with the string's encoding");
    }

    return is_binary ? (bom ? bom : (decl ? decl : rb_utf8_encoding())) : tag;
}

VALUE
mkr_xml_decode_input(VALUE str, size_t max_bytes)
{
    /* Phase 1: resolve the single effective byte encoding (raises on conflict). */
    rb_encoding *eff = mkr_xml_effective_encoding(str);

    /* Phase 2: decode to UTF-8 (strict). UTF-8 / US-ASCII / ASCII-8BIT are
     * already UTF-8 bytes (validated below); anything else is strict-transcoded,
     * raising rather than substituting U+FFFD. */
    VALUE s;
    if (eff == rb_utf8_encoding() || eff == rb_usascii_encoding() || eff == rb_ascii8bit_encoding()) {
        s = str;
    } else {
        VALUE in = str;
        if (rb_enc_get(str) != eff) { in = rb_str_dup(str); rb_enc_associate(in, eff); }
        int state = 0;
        s = rb_protect(mkr_xml_strict_transcode_thunk, in, &state);
        if (state != 0) {
            VALUE exc = rb_errinfo();
            rb_set_errinfo(Qnil);
            char msg[256];
            mkr_ruby_exception_message(exc, msg, sizeof msg);
            rb_raise(mkr_eXmlSyntaxError,
                     "XML input could not be decoded to UTF-8: %s", msg);
        }
    }

    mkr_ruby_borrowed_bytes_t sv_bytes = mkr_ruby_bytes_view(s);
    const char *ptr = sv_bytes.ptr;
    long        len = (long)sv_bytes.len;
    long        off = 0;
    /* §4.3.3: a leading BOM is the encoding signature, not document content -
     * strip a U+FEFF (the transcode above turns any UTF-16/32 BOM into one). */
    mkr_span_t sv = mkr_span(ptr, (size_t)len);
    if (mkr_span_starts(&sv, "\xEF\xBB\xBF", 3)) {
        off = 3; len -= 3;
        mkr_span_skip(&sv, 3);
    }

    /* Fail closed on an over-budget input BEFORE the validation scan and the
     * caller's GVL-release copy (an input whose UTF-8 length exceeds the arena
     * budget can never parse). max_bytes == 0 disables the check (__decode). */
    if (max_bytes != 0 && (size_t)len > max_bytes) {
        rb_raise(mkr_eXmlLimitExceeded, "XML input exceeds the byte budget");
    }

    /* Strict UTF-8 validation via the shared, allocation-free core - no GC point
     * while `ptr` is borrowed: an embedded NUL or any invalid UTF-8 is fatal (no
     * U+FFFD repair - unlike the HTML mkr_utf8_sanitize path). The whole-string
     * `s` is consulted for the cached coderange (it covers the BOM-stripped
     * suffix too - the BOM is one complete UTF-8 character), while the validated
     * bytes are the stripped suffix `ptr + off`. */
    switch (mkr_text_check(s, ptr + off, (size_t)len)) {
        case MKR_TEXT_HAS_NUL:
            rb_raise(mkr_eXmlSyntaxError, "XML input must not contain a NUL byte");
        case MKR_TEXT_INVALID_UTF8:
            rb_raise(mkr_eXmlSyntaxError, "XML input must be valid UTF-8");
        case MKR_TEXT_OK:
            break;
    }
    /* Build the result through the VALUE, not the borrowed ptr (rb_str_subseq
     * allocates, so the ptr must not be what it copies from). */
    VALUE u = rb_str_subseq(s, off, len);
    rb_enc_associate(u, rb_utf8_encoding());
    return u; /* validated, UTF-8-tagged, BOM-stripped */
}
