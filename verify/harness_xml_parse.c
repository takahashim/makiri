/* CBMC harness: the native XML tokenizer + tree builder over a nondet input
 * of up to VERIFY_XML_INPUT_MAX bytes.
 *
 * The input honours mkr_xml_parse's contract (already-decoded valid UTF-8,
 * XML-Char-only, NUL-free) via assumptions; within it, every byte sequence is
 * covered. Properties: no OOB/UB in the parser or the arena (--bounds-check
 * etc.), and the entry either returns a document or fails closed with a
 * non-OK status.
 */
#include "verify.h"
#include "core/mkr_core.h"
#include "xml/mkr_xml.h"

#include <stdbool.h>

#ifndef VERIFY_XML_INPUT_MAX
#define VERIFY_XML_INPUT_MAX 6
#endif

int
main(void)
{
    char buf[VERIFY_XML_INPUT_MAX];
    for (size_t i = 0; i < sizeof buf; ++i) buf[i] = (char)nondet_uchar();
    size_t len = nondet_size_t();
    VERIFY_ASSUME(len <= sizeof buf);

    /* Contract: valid UTF-8, XML Char only (which also excludes NUL). */
    bool utf8_ok = mkr_utf8_valid((const unsigned char *)buf, len);
    VERIFY_ASSUME(utf8_ok);
    bool chars_ok = (mkr_xml_validate_chars(buf, (uint32_t)len) == 0);
    VERIFY_ASSUME(chars_ok);

    mkr_xml_status_t st = MKR_XML_OK;
    mkr_xml_doc_t *doc = mkr_xml_parse(buf, len, &st);
    if (doc != NULL) {
        mkr_xml_doc_destroy(doc);
    } else {
        VERIFY_ASSERT(st != MKR_XML_OK, "parse failure sets a non-OK status");
    }
    return 0;
}
