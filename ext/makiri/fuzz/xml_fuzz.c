/* xml_fuzz.c - libFuzzer harness for mkr_xml_parse.
 *
 * Coverage-guided fuzzing of the XML tokenizer + tree builder.
 * Ruby-free; runs directly on the pure-C parser surface.
 */

#include <stdint.h>
#include <stddef.h>
#include "xml/mkr_xml.h"

int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size)
{
    mkr_xml_status_t status;
    /* The parser contract says "valid UTF-8, NUL-free", but the fuzzer feeds
     * raw bytes.  mkr_xml_parse is fail-closed: it validates as it goes and
     * returns an error status on malformed input, never a partial document.
     * We pass the bytes through untouched so the fuzzer can reach the invalid-
     * UTF-8 / unexpected-NUL error paths too. */
    mkr_xml_doc_t *doc = mkr_xml_parse((const char *) data, size, &status);
    if (doc) {
        mkr_xml_doc_destroy(doc);
    }
    return 0;
}
