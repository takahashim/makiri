/* CBMC harness: the XML character-class layer (xml/mkr_xml_chars.c) - the
 * predicates over ALL code points, and the validators cross-checked against
 * the INDEPENDENT UTF-8 validator implementation.
 *
 * The spec-shaped properties:
 *   - NameStartChar is a subset of NameChar, and NameChar of Char (XML 1.0
 *     §2.2/§2.3) - proved over every uint32, not sampled;
 *   - a validate_chars-accepted buffer is valid UTF-8 per mkr_utf8_valid.
 *     This is a real cross-implementation proof: validate_chars decodes with
 *     the strict one-codepoint decoder while mkr_utf8_valid is the separate
 *     word-at-a-time table scan - agreement is not tautological;
 *   - a validate_name-accepted buffer is validate_chars-accepted (Names are
 *     Chars), and non-empty.
 */
#include "verify.h"
#include "core/mkr_utf8.h"
#include "xml/mkr_xml.h"

#include <stdbool.h>

#ifndef VERIFY_XML_CHARS_MAX
#define VERIFY_XML_CHARS_MAX 8
#endif

int
main(void)
{
    /* --- predicate inclusions, over ALL code points --- */
    {
        uint32_t c = (uint32_t)nondet_size_t();
        if (mkr_xml_is_name_start(c))
            VERIFY_ASSERT(mkr_xml_is_name_char(c), "NameStartChar subset of NameChar");
        if (mkr_xml_is_name_char(c))
            VERIFY_ASSERT(mkr_xml_is_char(c), "NameChar subset of Char");
        if (mkr_xml_is_char(c)) {
            VERIFY_ASSERT(c <= 0x10FFFFu, "Char: scalar range");
            VERIFY_ASSERT(!(c >= 0xD800u && c <= 0xDFFFu), "Char: no surrogates");
        }
    }

    /* --- validators over every byte sequence up to the bound --- */
    {
        char buf[VERIFY_XML_CHARS_MAX];
        for (size_t i = 0; i < sizeof buf; ++i) buf[i] = (char)(unsigned char)nondet_uchar();
        size_t len = nondet_size_t();
        VERIFY_ASSUME(len <= sizeof buf);

        if (mkr_xml_validate_chars(buf, (uint32_t)len) == 0) {
            bool utf8_ok = mkr_utf8_valid((const unsigned char *)buf, len);
            VERIFY_ASSERT(utf8_ok, "XML-Char-valid implies UTF-8-valid (cross-implementation)");
        }
        if (mkr_xml_validate_name(buf, (uint32_t)len) == 0) {
            VERIFY_ASSERT(len > 0, "a Name is non-empty");
            VERIFY_ASSERT(mkr_xml_validate_chars(buf, (uint32_t)len) == 0,
                          "a Name is made of Chars");
        }
    }
    return 0;
}
