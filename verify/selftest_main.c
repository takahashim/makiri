/* Standalone runner for the carve-out (docs/formal_verification.ja.md §3):
 * proves the XML parser + XML XPath engine instance work with no Ruby VM and
 * no Lexbor library linked (only unmodified vendor encoding sources). */
#include <stdio.h>

int mkr_xml_parse_selftest(void);
int mkr_xml_xpath_selftest(void);

int
main(void)
{
    int a = mkr_xml_parse_selftest();
    int b = mkr_xml_xpath_selftest();
    printf("xml_parse_selftest=%d xml_xpath_selftest=%d\n", a, b);
    return (a == 0 && b == 0) ? 0 : 1;
}
