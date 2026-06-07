/* Element-name index for the XML arena (see mkr_xml_index.h). Open-addressing
 * hash keyed by (local name + namespace URI); each entry holds the
 * document-ordered elements with that name. Two tree passes: count elements to
 * size the table, then fill in document order. Heap-allocated (not the arena),
 * freed on invalidate. OOM fails closed (NULL -> caller walks the tree). */

#include "mkr_xml_index.h"
#include "../core/mkr_core.h"

#include <stdlib.h>
#include <string.h>

typedef struct {
  const char     *local;      /* borrowed from the first element (arena-stable) */
  const char     *ns_uri;     /* may be NULL (no namespace) */
  uint32_t        local_len;
  uint32_t        ns_uri_len;
  mkr_xml_node_t **nodes;      /* document order */
  size_t          count, cap;
} mkr_xml_index_entry_t;

struct mkr_xml_name_index {
  mkr_xml_index_entry_t *buckets;
  size_t                 cap;   /* power of two; 0 only for an empty document */
};

/* FNV-1a over the local name then the namespace URI. */
static uint64_t
key_hash(const char *local, size_t local_len, const char *ns_uri, size_t ns_uri_len)
{
  uint64_t h = 1469598103934665603ULL;
  for (size_t i = 0; i < local_len; i++)  { h ^= (unsigned char)local[i];  h *= 1099511628211ULL; }
  h ^= 0xff; h *= 1099511628211ULL;  /* separator so "ab"+"" != "a"+"b" */
  for (size_t i = 0; i < ns_uri_len; i++) { h ^= (unsigned char)ns_uri[i]; h *= 1099511628211ULL; }
  return h;
}

static int
key_eq(const mkr_xml_index_entry_t *e, const char *local, size_t local_len,
       const char *ns_uri, size_t ns_uri_len)
{
  if (e->local_len != local_len || e->ns_uri_len != ns_uri_len) return 0;
  if (local_len && memcmp(e->local, local, local_len) != 0) return 0;
  if (ns_uri_len && memcmp(e->ns_uri, ns_uri, ns_uri_len) != 0) return 0;
  return 1;
}


/* Find the entry for the key, or the empty slot to create it (open addressing,
 * power-of-two mask). The table is sized so it never fills (load < 0.5). */
static mkr_xml_index_entry_t *
slot_for(mkr_xml_name_index_t *idx, const char *local, size_t local_len,
         const char *ns_uri, size_t ns_uri_len)
{
  size_t mask = idx->cap - 1;
  size_t i = (size_t)key_hash(local, local_len, ns_uri, ns_uri_len) & mask;
  for (;;) {
    mkr_xml_index_entry_t *e = &idx->buckets[i];
    if (e->local == NULL && e->count == 0) return e;            /* empty */
    if (key_eq(e, local, local_len, ns_uri, ns_uri_len)) return e;
    i = (i + 1) & mask;
  }
}

/* Append +node+ to its key's bucket, creating the entry if new. 0 / -1 (OOM). */
static int
index_push(mkr_xml_name_index_t *idx, mkr_xml_node_t *node)
{
  mkr_xml_index_entry_t *e =
      slot_for(idx, node->local, node->local_len, node->ns_uri, node->ns_uri_len);
  if (e->local == NULL && e->count == 0) {       /* fresh entry: borrow the key */
    e->local = node->local;          e->local_len = node->local_len;
    e->ns_uri = node->ns_uri;        e->ns_uri_len = node->ns_uri_len;
  }
  if (mkr_grow_reserve((void **)&e->nodes, &e->cap, e->count + 1, sizeof(*e->nodes)) != MKR_OK) {
    return -1;  /* grows geometrically + overflow-safely internally */
  }
  e->nodes[e->count++] = node;
  return 0;
}

/* Count elements (document order is irrelevant here) to size the table. */
static size_t
count_elements(mkr_xml_node_t *root)
{
  size_t n = 0;
  for (mkr_xml_node_t *cur = root; cur != NULL; ) {
    if (cur->type == MKR_XML_NODE_TYPE_ELEMENT) n++;
    if (cur->first_child != NULL) { cur = cur->first_child; continue; }
    while (cur != root && cur->next == NULL) cur = cur->parent;
    if (cur == root) break;
    cur = cur->next;
  }
  return n;
}

static mkr_xml_name_index_t *
build(mkr_xml_doc_t *doc)
{
  mkr_xml_node_t *root = doc->doc_node;
  if (root == NULL) return NULL;

  mkr_xml_name_index_t *idx = (mkr_xml_name_index_t *)mkr_callocarray(1, sizeof(*idx));
  if (idx == NULL) return NULL;

  size_t n = count_elements(root);
  /* Size for load factor < 0.5 (2n+1 slots). The overflow-checked sizer fails
   * closed - unlike the old next_pow2, which saturated to a too-small table on
   * overflow, where open addressing could never find a free slot. */
  size_t want;
  if (!mkr_size_mul(n, 2, &want) || !mkr_size_add(want, 1, &want)
      || !mkr_pow2_ceil(want, &idx->cap)) { free(idx); return NULL; }
  if (idx->cap < 8) idx->cap = 8;              /* small floor */
  idx->buckets = (mkr_xml_index_entry_t *)mkr_callocarray(idx->cap, sizeof(*idx->buckets));
  if (idx->buckets == NULL) { free(idx); return NULL; }

  /* Fill pass: pre-order (document order), elements only. */
  for (mkr_xml_node_t *cur = root; cur != NULL; ) {
    if (cur->type == MKR_XML_NODE_TYPE_ELEMENT) {
      if (index_push(idx, cur) != 0) { mkr_xml_name_index_free(idx); return NULL; }
    }
    if (cur->first_child != NULL) { cur = cur->first_child; continue; }
    while (cur != root && cur->next == NULL) cur = cur->parent;
    if (cur == root) break;
    cur = cur->next;
  }
  return idx;
}

mkr_xml_name_index_t *
mkr_xml_name_index_get(mkr_xml_doc_t *doc)
{
  if (doc == NULL) return NULL;
  if (doc->name_index != NULL) return (mkr_xml_name_index_t *)doc->name_index;
  mkr_xml_name_index_t *idx = build(doc);
  doc->name_index = idx;          /* NULL on OOM: caller walks, retries next time */
  return idx;
}

void
mkr_xml_name_index_free(mkr_xml_name_index_t *idx)
{
  if (idx == NULL) return;
  if (idx->buckets != NULL) {
    for (size_t i = 0; i < idx->cap; i++) free(idx->buckets[i].nodes);
    free(idx->buckets);
  }
  free(idx);
}

void
mkr_xml_name_index_invalidate(mkr_xml_doc_t *doc)
{
  if (doc == NULL || doc->name_index == NULL) return;
  mkr_xml_name_index_free((mkr_xml_name_index_t *)doc->name_index);
  doc->name_index = NULL;
}

mkr_xml_node_t *const *
mkr_xml_name_index_lookup(const mkr_xml_name_index_t *idx,
                          const char *local, size_t local_len,
                          const char *ns_uri, size_t ns_uri_len, size_t *out_count)
{
  if (out_count != NULL) *out_count = 0;
  if (idx == NULL || idx->cap == 0 || local == NULL) return NULL;
  size_t mask = idx->cap - 1;
  size_t i = (size_t)key_hash(local, local_len, ns_uri, ns_uri_len) & mask;
  for (;;) {
    const mkr_xml_index_entry_t *e = &idx->buckets[i];
    if (e->local == NULL && e->count == 0) return NULL;          /* miss */
    if (key_eq(e, local, local_len, ns_uri, ns_uri_len)) {
      if (out_count != NULL) *out_count = e->count;
      return e->nodes;
    }
    i = (i + 1) & mask;
  }
}
