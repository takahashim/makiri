#include "mkr_alloc.h"

#ifdef MKR_ALLOC_INJECT
/* See mkr_alloc.h: the OOM sweep's failure injection. The counter counts
 * ATTEMPTS (every consult), armed or not, so the harness can size its sweep
 * from a disarmed baseline run; the countdown fails exactly one allocation
 * and then disarms itself, modelling a single transient OOM per run. */
static long long          mkr_inject_countdown = 0;  /* 0 = disarmed */
static unsigned long long mkr_inject_attempts  = 0;

void
mkr_alloc_inject_arm(long long nth)
{
    mkr_inject_countdown = (nth > 0) ? nth : 0;
    mkr_inject_attempts  = 0;
}

unsigned long long
mkr_alloc_inject_calls(void)
{
    return mkr_inject_attempts;
}

int
mkr_alloc_inject_should_fail(void)
{
    mkr_inject_attempts++;
    if (mkr_inject_countdown > 0 && --mkr_inject_countdown == 0) {
        return 1; /* fail this one allocation; now disarmed */
    }
    return 0;
}
#endif

void *
mkr_reallocarray(void *ptr, size_t count, size_t elem)
{
    if (count == 0) {
        free(ptr);
        return NULL;
    }
    if (elem == 0) {
        /* Fail closed like mkr_callocarray rather than fall through to a
         * realloc(ptr, 0) whose free-or-not is implementation-defined; the
         * caller keeps ownership of ptr. */
        return NULL;
    }
    size_t bytes;
    if (!mkr_size_mul(count, elem, &bytes)) {
        return NULL; /* overflow: leave ptr unchanged */
    }
    if (MKR_ALLOC_INJECT_FAIL()) return NULL;
    return realloc(ptr, bytes);
}

void *
mkr_callocarray(size_t count, size_t elem)
{
    if (count == 0 || elem == 0) {
        return NULL;
    }
    /* 2-arg calloc is itself overflow-safe, but check explicitly so every core
     * allocator fails the SAME way (deterministic NULL) rather than leaving the
     * overflow case to calloc's implementation-defined behaviour. */
    if (count > SIZE_MAX / elem) {
        return NULL; /* overflow */
    }
    if (MKR_ALLOC_INJECT_FAIL()) return NULL;
    return calloc(count, elem);
}

char *
mkr_str_alloc(size_t n)
{
    size_t total;
    if (!mkr_size_add(n, 1, &total)) {
        return NULL; /* n + 1 overflow */
    }
    if (MKR_ALLOC_INJECT_FAIL()) return NULL;
    char *p = malloc(total);
    if (p == NULL) {
        return NULL;
    }
    p[n] = '\0';
    return p;
}

char *
mkr_strndup(const char *s, size_t n)
{
    if (n > 0 && s == NULL) {
        return NULL; /* fail closed: would otherwise return uninitialised bytes */
    }
    char *p = mkr_str_alloc(n);
    if (p == NULL) {
        return NULL;
    }
    if (n > 0) {
        memcpy(p, s, n);
    }
    p[n] = '\0';
    return p;
}

char *
mkr_strdup(const char *s)
{
    if (s == NULL) {
        return NULL;
    }
    return mkr_strndup(s, strlen(s));
}

mkr_status_t
mkr_grow_reserve(void **ptr, size_t *cap, size_t need, size_t elem)
{
    if (need <= *cap) {
        return MKR_OK;
    }
    size_t new_cap;
    if (!mkr_grow_capacity(*cap, need, elem, &new_cap)) {
        return MKR_ERR_OOM;
    }
    void *p = mkr_reallocarray(*ptr, new_cap, elem);
    if (p == NULL) {
        return MKR_ERR_OOM;
    }
    *ptr = p;
    *cap = new_cap;
    return MKR_OK;
}
