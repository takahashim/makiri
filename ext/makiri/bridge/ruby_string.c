#include "bridge.h"
#include "../makiri.h"

#include <limits.h>
#include <stdio.h>
#include <string.h>

VALUE
mkr_ruby_str_from_slices(const mkr_borrowed_text_t *slices, size_t n, size_t total)
{
    /* Defensive bounds: the caller (text index) guarantees the slice lengths sum
     * to total, but a wrong total would overflow the output buffer. Fail closed
     * rather than memcpy past the allocation. */
    if (total > (size_t)LONG_MAX) {
        rb_raise(mkr_eError, "text too large to assemble");
    }
    VALUE  str = rb_utf8_str_new(NULL, (long)total);
    char  *dst = RSTRING_PTR(str);
    size_t off = 0;
    for (size_t i = 0; i < n; i++) {
        size_t len = slices[i].len;
        if (len == 0) continue;
        if (len > total - off) { /* off <= total holds, so total - off can't underflow */
            rb_raise(mkr_eError, "text slice length inconsistency");
        }
        memcpy(dst + off, slices[i].ptr, len);
        off += len;
    }
    /* The slices must fill the buffer exactly; a short sum would leave the tail
     * of the (uninitialised) Ruby String unwritten. Fail closed. */
    if (off != total) {
        rb_raise(mkr_eError, "text slice length inconsistency");
    }
    return str;
}

VALUE
mkr_ruby_str_from_borrowed(mkr_borrowed_text_t text)
{
    /* rb_utf8_str_new copies, so the borrow need not outlive this call. NULL is
     * the "absent" sentinel -> "" regardless of len (never read off the "" lit). */
    if (text.ptr == NULL) {
        return rb_utf8_str_new("", 0);
    }
    return rb_utf8_str_new(text.ptr, (long)text.len);
}

void
mkr_verify_text(VALUE str, const char *what)
{
    /* ALLOCATION-FREE by design: this gate runs between a caller taking a
     * borrowed RSTRING pointer and using it, so it must not be a GC point. The
     * former implementation built a throwaway Ruby String (rb_enc_str_new) to
     * ask for its coderange - a Ruby allocation inside every borrow, which both
     * passed the borrowed ptr into an allocating call and opened a GC window
     * under every OTHER borrow already held at multi-borrow call sites. Bytes
     * are validated as UTF-8 regardless of the String's declared encoding,
     * exactly as before. */
    long        len = RSTRING_LEN(str);
    const char *ptr = RSTRING_PTR(str);

    mkr_span_t sv = mkr_span(ptr, (size_t)len);
    size_t nul_at;
    if (mkr_span_find(&sv, '\0', &nul_at)) {
        rb_raise(mkr_eError, "%s must not contain a NUL byte", what);
    }

    /* Cached-coderange fast path (reads flags, never scans, never allocates);
     * NUL is valid UTF-8, so the memchr above stays either way. */
    if (mkr_ruby_str_known_valid_utf8(str)) {
        return;
    }
    if (!mkr_utf8_valid((const unsigned char *)ptr, (size_t)len)) {
        rb_raise(mkr_eError, "%s must be valid UTF-8", what);
    }
}

mkr_ruby_borrowed_text_t
mkr_ruby_verified_text(VALUE in, const char *what)
{
    VALUE s = rb_String(in);
    mkr_verify_text(s, what);
    mkr_ruby_borrowed_text_t v;
    v.value = s;
    v.ptr   = RSTRING_PTR(s);
    v.len   = (size_t)RSTRING_LEN(s);
    return v;
}

mkr_ruby_borrowed_bytes_t
mkr_ruby_bytes_view(VALUE in)
{
    VALUE s = rb_String(in);
    mkr_ruby_borrowed_bytes_t v;
    v.value = s;
    v.ptr   = RSTRING_PTR(s);
    v.len   = (size_t)RSTRING_LEN(s);
    return v;
}

int
mkr_ruby_copy_bytes(VALUE in, mkr_owned_bytes_t *out)
{
    mkr_ruby_borrowed_bytes_t v = mkr_ruby_bytes_view(in);
    out->ptr = NULL;
    out->len = 0;

    size_t alloc_len = (v.len > 0) ? v.len : 1;
    char *buf = mkr_reallocarray(NULL, alloc_len, 1);
    if (buf == NULL) {
        return -1;
    }
    if (v.len > 0) {
        memcpy(buf, v.ptr, v.len);
    }
    out->ptr = buf;
    out->len = v.len;
    RB_GC_GUARD(v.value);
    return 0;
}

VALUE
mkr_ruby_to_utf8(VALUE str)
{
    /* Honour the Ruby String's declared encoding so its content survives:
     *
     *  - UTF-8 / US-ASCII / ASCII-8BIT (binary): returned unchanged. These are
     *    already UTF-8 bytes (or deliberately raw bytes), and the native parser
     *    does the WHATWG invalid-byte replacement for them. The UTF-8 common
     *    case costs only this encoding comparison - no transcode, no copy.
     *
     *  - any other encoding (Shift_JIS, EUC-JP, ISO-8859-1, Windows-1252, ...):
     *    transcoded to UTF-8 with invalid/undef -> U+FFFD, so e.g. Shift_JIS
     *    text becomes the right UTF-8 characters instead of being read as raw
     *    UTF-8 bytes and mangled. Only non-UTF-8 input pays this. */
    rb_encoding *enc = rb_enc_get(str);
    if (enc == rb_utf8_encoding()
        || enc == rb_usascii_encoding()
        || enc == rb_ascii8bit_encoding()) {
        return str;
    }
    return rb_str_encode(str, rb_enc_from_encoding(rb_utf8_encoding()),
                         ECONV_INVALID_REPLACE | ECONV_UNDEF_REPLACE, Qnil);
}

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
 * checked before the UTF-16 LE BOM they share a prefix with. */
static rb_encoding *
mkr_xml_bom_encoding(const unsigned char *p, long len, long *bom_len)
{
    mkr_span_t s = mkr_span((const char *)p, (size_t)len);
    *bom_len = 0;
    if (mkr_span_starts(&s, "\x00\x00\xFE\xFF", 4)) { *bom_len = 4; return rb_enc_find("UTF-32BE"); }
    if (mkr_span_starts(&s, "\xFF\xFE\x00\x00", 4)) { *bom_len = 4; return rb_enc_find("UTF-32LE"); }
    if (mkr_span_starts(&s, "\xFE\xFF", 2)) { *bom_len = 2; return rb_enc_find("UTF-16BE"); }
    if (mkr_span_starts(&s, "\xFF\xFE", 2)) { *bom_len = 2; return rb_enc_find("UTF-16LE"); }
    if (mkr_span_starts(&s, "\xEF\xBB\xBF", 3)) { *bom_len = 3; return rb_utf8_encoding(); }
    return NULL;
}

/* The encoding named in the '<?xml ... encoding="NAME" ?>' declaration, or NULL.
 * The declaration is ASCII; for a UTF-16/32-detected document its bytes are
 * stride-interleaved, so the ASCII column is extracted (per the BOM) before the
 * scan, letting a BOM-vs-declaration conflict be caught even in UTF-16. */
static int
mkr_decl_ws(int c)
{
    return c == ' ' || c == '\t' || c == '\r' || c == '\n';
}

/* The remaining head bytes starting at offset i (a sub-span; reads bounded). */
static mkr_span_t
mkr_decl_tail(const mkr_span_t *h, size_t i)
{
    mkr_span_t t = *h;
    mkr_span_skip(&t, i);
    return t;
}

static rb_encoding *
mkr_xml_decl_encoding(const unsigned char *p, long len, rb_encoding *bom)
{
    long stride = 1, off = 0;
    if (bom == rb_enc_find("UTF-16LE"))      { stride = 2; off = 0; }
    else if (bom == rb_enc_find("UTF-16BE")) { stride = 2; off = 1; }
    else if (bom == rb_enc_find("UTF-32LE")) { stride = 4; off = 0; }
    else if (bom == rb_enc_find("UTF-32BE")) { stride = 4; off = 3; }

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
        mkr_span_t t = mkr_decl_tail(&h, i);
        if (!mkr_span_starts(&t, "<?xml", 5)) return NULL;
    }
    i += 5;
    /* find a whitespace-introduced "encoding" before the '?>' */
    for (; i + 8 <= hn; i++) {
        if (mkr_span_at(&h, i) == '?' && mkr_span_at(&h, i + 1) == '>') return NULL; /* end of decl */
        mkr_span_t t = mkr_decl_tail(&h, i);
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

VALUE
mkr_xml_decode_input(VALUE str, size_t max_bytes)
{
    rb_encoding   *tag    = rb_enc_get(str);
    const unsigned char *raw = (const unsigned char *)RSTRING_PTR(str);
    long           rawlen = RSTRING_LEN(str);

    /* Detect the byte encoding (XML 1.0 Appendix F): a BOM wins, else the
     * declaration. The Ruby String's encoding is authoritative when it is a
     * concrete text encoding; a BOM/declaration that disagrees is a fatal
     * conflict. ASCII-8BIT means "raw bytes, no claimed encoding", so there the
     * detected encoding decodes the input (a UTF-16/Shift_JIS/BOM'd file read
     * with File.binread now parses). */
    long bom_len = 0;
    rb_encoding *bom  = mkr_xml_bom_encoding(raw, rawlen, &bom_len);
    /* rb_enc_find inside the BOM lookup can autoload an encoding (a Ruby
     * allocation = a GC point), so re-borrow the bytes before reading them
     * again - a borrowed RSTRING pointer must not be held across one. */
    raw = (const unsigned char *)RSTRING_PTR(str);
    rb_encoding *decl = mkr_xml_decl_encoding(raw + bom_len, rawlen - bom_len, bom);
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

    rb_encoding *eff = is_binary ? (bom ? bom : (decl ? decl : rb_utf8_encoding())) : tag;

    /* Decode to UTF-8 (strict). UTF-8 / US-ASCII / ASCII-8BIT are already UTF-8
     * bytes (validated below); anything else is strict-transcoded, raising rather
     * than substituting U+FFFD. */
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

    const char *ptr = RSTRING_PTR(s);
    long        len = RSTRING_LEN(s);
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

    /* Strict UTF-8 validation, allocation-free - no GC point while `ptr` is
     * borrowed (the former rb_enc_str_new copy handed the borrow straight into
     * an allocating call): an embedded NUL or any invalid UTF-8 is fatal (no
     * U+FFFD repair - unlike the HTML mkr_utf8_sanitize path). A whole-string
     * cached coderange covers the BOM-stripped suffix too (the BOM is one
     * complete UTF-8 character). */
    size_t nul_at;
    if (mkr_span_find(&sv, '\0', &nul_at)) {
        rb_raise(mkr_eXmlSyntaxError, "XML input must not contain a NUL byte");
    }
    if (!mkr_ruby_str_known_valid_utf8(s)
        && !mkr_utf8_valid((const unsigned char *)ptr + off, (size_t)len)) {
        rb_raise(mkr_eXmlSyntaxError, "XML input must be valid UTF-8");
    }
    /* Build the result through the VALUE, not the borrowed ptr (rb_str_subseq
     * allocates, so the ptr must not be what it copies from). */
    VALUE u = rb_str_subseq(s, off, len);
    rb_enc_associate(u, rb_utf8_encoding());
    return u; /* validated, UTF-8-tagged, BOM-stripped */
}

bool
mkr_ruby_str_known_valid_utf8(VALUE str)
{
    if (!RB_TYPE_P(str, T_STRING)) {
        return false;
    }
    /* ENC_CODERANGE reads the *cached* classification from the object's flags;
     * it does NOT scan (rb_enc_str_coderange would, costing as much as our own
     * validator). So this only wins when Ruby already knows the answer. */
    int cr = ENC_CODERANGE(str);
    if (cr == ENC_CODERANGE_7BIT) {
        return true; /* all bytes < 0x80 in an ASCII-compatible encoding */
    }
    if (cr == ENC_CODERANGE_VALID) {
        return rb_enc_get(str) == rb_utf8_encoding(); /* valid AND UTF-8 */
    }
    return false; /* UNKNOWN or BROKEN: let mkr_utf8_sanitize handle it */
}

const char *
mkr_ruby_try_verified_text(VALUE sv, size_t max_bytes, mkr_ruby_borrowed_text_t *out)
{
    /* ALLOCATION-FREE, like mkr_verify_text: the returned borrow must not have
     * crossed a Ruby allocation (the former rb_utf8_str_new + valid_encoding?
     * funcall allocated twice with `ptr` already taken). */
    long len = RSTRING_LEN(sv);
    if ((size_t)len > max_bytes) {
        return "string exceeds the maximum length";
    }
    const char *ptr = RSTRING_PTR(sv);
    mkr_span_t view = mkr_span(ptr, (size_t)len);
    size_t nul_at;
    if (mkr_span_find(&view, '\0', &nul_at)) {
        return "string contains a NUL byte";
    }
    if (!mkr_ruby_str_known_valid_utf8(sv)
        && !mkr_utf8_valid((const unsigned char *)ptr, (size_t)len)) {
        return "string is not valid UTF-8";
    }
    out->value = sv;
    out->ptr   = ptr;
    out->len   = (size_t)len;
    return NULL;
}

static VALUE
mkr_exception_message_thunk(VALUE exc)
{
    return rb_obj_as_string(rb_funcall(exc, rb_intern("message"), 0));
}

void
mkr_ruby_exception_message(VALUE exc, char *buf, size_t len)
{
    if (buf == NULL || len == 0) {
        return;
    }
    int state = 0;
    VALUE msg = rb_protect(mkr_exception_message_thunk, exc, &state);
    if (state != 0) {
        rb_set_errinfo(Qnil);
        snprintf(buf, len, "%s", "error");
        return;
    }
    if (!RB_TYPE_P(msg, T_STRING)) {
        snprintf(buf, len, "%s", "error");
        return;
    }
    snprintf(buf, len, "%s", RSTRING_PTR(msg));
}
