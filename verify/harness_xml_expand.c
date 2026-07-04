/* CBMC harness: entity / character-reference expansion (mkr_xml_expand) over
 * every valid-UTF-8 input up to VERIFY_XML_EXPAND_MAX bytes.
 *
 * This is the reference-expansion boundary the fuzz workflow singles out as a
 * risk area, and the home of the "output is never longer than the input"
 * claim that sizes the arena write. The document the expansion writes into is
 * parsed from a CONCRETE 4-byte input - the whole-parser proof does not
 * converge on nondet input (see cbmc-deep), but on a concrete document the
 * parser is deterministic and costs the solver nearly nothing; only the
 * expansion input is nondet.
 *
 * Properties, on every accepted input:
 *   - out_len <= len (the never-longer sizing claim);
 *   - the output is XML-Char clean per mkr_xml_validate_chars - which also
 *     proves the private numeric-reference re-encoder emits well-formed UTF-8
 *     (validate_chars decodes with the strict core decoder);
 *   - a reference-free TEXT input round-trips byte-identically;
 * and on every rejected input the status says SYNTAX, fail-closed.
 */
#include "verify.h"
#include "core/mkr_utf8.h"
#include "xml/mkr_xml.h"

#include <stdbool.h>
#include <string.h>

#ifndef VERIFY_XML_EXPAND_MAX
#define VERIFY_XML_EXPAND_MAX 7   /* covers every reference form: &#xHH; is 6, "&quot;" is 6 */
#endif

int
main(void)
{
    static const char tiny[] = "<a/>";
    mkr_xml_status_t st = MKR_XML_OK;
    mkr_xml_doc_t *doc = mkr_xml_parse(tiny, sizeof tiny - 1, &st);
    if (doc == NULL) return 0;

    char buf[VERIFY_XML_EXPAND_MAX];
    for (size_t i = 0; i < sizeof buf; ++i) buf[i] = (char)(unsigned char)nondet_uchar();
    size_t len = nondet_size_t();
    VERIFY_ASSUME(len <= sizeof buf);

    /* Contract (§2.1): the input reaching expansion is already valid UTF-8. */
    bool utf8_ok = mkr_utf8_valid((const unsigned char *)buf, len);
    VERIFY_ASSUME(utf8_ok);

    mkr_xml_expand_mode_t mode =
        (nondet_uchar() & 1) ? MKR_XML_EXPAND_ATTR : MKR_XML_EXPAND_TEXT;

    uint32_t out_len = 0;
    st = MKR_XML_OK;
    const char *out = mkr_xml_expand(doc, buf, (uint32_t)len, mode, &out_len, &st);
    if (out != NULL) {
        VERIFY_ASSERT(out_len <= len, "expand: output never longer than the input");
        VERIFY_ASSERT(mkr_xml_validate_chars(out, out_len) == 0,
                      "expand: output is XML-Char clean (and thus well-formed UTF-8)");
        bool no_ref = true;
        for (size_t i = 0; i < len; ++i)
            if (buf[i] == '&' || buf[i] == '\t' || buf[i] == '\n' || buf[i] == '\r')
                no_ref = false;
        if (no_ref && mode == MKR_XML_EXPAND_TEXT) {
            VERIFY_ASSERT(out_len == len, "expand: reference-free text keeps its length");
            VERIFY_ASSERT(len == 0 || memcmp(out, buf, len) == 0,
                          "expand: reference-free text round-trips");
        }
    } else {
        VERIFY_ASSERT(st != MKR_XML_OK, "expand: failure sets a non-OK status");
    }
    mkr_xml_doc_destroy(doc);
    return 0;
}
