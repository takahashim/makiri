//! The names a document interns: tag names and attribute local names.
//!
//! Lexbor keeps two hash tables per document (`tags` and `attrs`) that the
//! tokenizer and the DOM share: a name in Lexbor's static table resolves to
//! its static id, and any other name is appended, lower-cased, and gets an id
//! of its own (`lxb_tag_append_lower`, `lxb_dom_attr_local_name_append`).
//! Lexbor's appended ids are pointer values; here they are numbers above the
//! static range, which is all any reader asks of them (equality, and "is it
//! static", which is `id < *__LAST_ENTRY`).
//!
//! Lookups hash the name as they read it (Lexbor's one-at-a-time
//! `lexbor_hash_make_id_lower`) and compare it as Lexbor's
//! `lexbor_str_data_nlocmp_right` does - the stored key against the
//! lower-cased input - so a lookup copies nothing. The static ids live in a
//! table built at compile time from the generated names.

use crate::falloc::{try_vec_with_capacity, VecGrow};
use crate::html::res::names as res;

/// An allocation the table needed failed; nothing was added.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NamesOom;

/// `lexbor_hash_make_id_lower`.
#[inline]
const fn hash_lower(name: &[u8]) -> u32 {
    let mut h: u32 = 0;
    let mut i = 0;
    while i < name.len() {
        h = h.wrapping_add(name[i].to_ascii_lowercase() as u32);
        h = h.wrapping_add(h << 10);
        h ^= h >> 6;
        i += 1;
    }
    h = h.wrapping_add(h << 3);
    h ^= h >> 11;
    h.wrapping_add(h << 15)
}

/// Whether `stored` (lower case) is `input` lower-cased
/// (`lexbor_str_data_nlocmp_right`, with the lengths compared first).
#[inline]
fn same_lower(stored: &[u8], input: &[u8]) -> bool {
    stored.len() == input.len()
        && stored
            .iter()
            .zip(input)
            .all(|(k, c)| *k == c.to_ascii_lowercase())
}

#[inline]
fn has_upper(name: &[u8]) -> bool {
    name.iter().any(u8::is_ascii_uppercase)
}

/// Open-addressing slots for a static table: `id + 1`, 0 empty. `N` is a power
/// of two at least twice the number of names - checked here, so a table
/// that outgrew its slots is a compile error naming the reason rather than
/// a probe loop that never ends.
const fn static_slots<const N: usize>(names: &[&[u8]]) -> [u16; N] {
    assert!(
        N.is_power_of_two(),
        "static slots: N must be a power of two"
    );
    assert!(
        names.len() * 2 <= N,
        "static slots: N must be at least twice the names - grow it"
    );
    let mut slots = [0u16; N];
    let mut id = 0;
    while id < names.len() {
        let mut slot = hash_lower(names[id]) as usize & (N - 1);
        while slots[slot] != 0 {
            slot = (slot + 1) & (N - 1);
        }
        slots[slot] = (id + 1) as u16;
        id += 1;
    }
    slots
}

static TAG_SLOTS: [u16; 512] = static_slots::<512>(&res::TAG_NAMES);
static ATTR_SLOTS: [u16; 256] = static_slots::<256>(&res::ATTR_NAMES);

/// The longest of `names`.
const fn longest_of(names: &[&[u8]]) -> usize {
    let mut longest = 0;
    let mut i = 0;
    while i < names.len() {
        if names[i].len() > longest {
            longest = names[i].len();
        }
        i += 1;
    }
    longest
}

/// The longest static tag or attribute name, from the tables themselves.
const STATIC_LONGEST: usize = {
    let (t, a) = (longest_of(&res::TAG_NAMES), longest_of(&res::ATTR_NAMES));
    if t > a {
        t
    } else {
        a
    }
};

/// The static id for `name`, matched as Lexbor's static lookup matches it.
#[inline]
fn static_id(slots: &[u16], names: &[&[u8]], name: &[u8], h: u32) -> Option<usize> {
    let mask = slots.len() - 1;
    let mut slot = h as usize & mask;
    loop {
        let id = *slots.get(slot)? as usize;
        if id == 0 {
            return None;
        }
        if same_lower(names.get(id - 1)?, name) {
            return Some(id - 1);
        }
        slot = (slot + 1) & mask;
    }
}

/// One table: names appended past the static ones.
///
/// An entry is stored lower-cased (Lexbor's `lexbor_hash_insert_lower`) or
/// as given (`lexbor_hash_insert_raw`). Lookups hash ASCII case-insensitively
/// either way, so both kinds share the slots; a lower-casing lookup compares
/// the stored key against the lower-cased input, which an entry holding
/// upper case never equals - exactly Lexbor's one table of both kinds.
#[derive(Default)]
struct Appended {
    /// Whether the table keeps each entry's upper-case form: a property of
    /// the table, fixed when it is made, so `uppers` is index for index with
    /// `names` or empty - never a mix that would answer an entry with
    /// another's.
    keeps_upper: bool,
    /// Open addressing over `names`: index + 1, 0 empty; empty or a power of
    /// two, at most half full.
    slots: Vec<u32>,
    names: Vec<Vec<u8>>,
    hashes: Vec<u32>,
    /// Each entry's ASCII upper-case form, when the table keeps them (tags,
    /// for the HTML `tagName`; qualified names). Empty otherwise.
    uppers: Vec<Vec<u8>>,
    /// The longest entry's length.
    longest: usize,
    /// The bytes of every entry, both forms: an O(1) measure of the table's
    /// growth.
    stored: usize,
}

/// How a table compares an input with a stored key.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Match {
    /// The stored key against the lower-cased input.
    Lower,
    /// Byte for byte.
    Exact,
}

impl Appended {
    /// An empty table, keeping upper-case forms when `keeps_upper`.
    fn new(keeps_upper: bool) -> Self {
        Appended {
            keeps_upper,
            ..Appended::default()
        }
    }

    fn find(&self, name: &[u8], h: u32) -> Option<usize> {
        self.find_by(name, h, Match::Lower)
    }

    fn find_by(&self, name: &[u8], h: u32, how: Match) -> Option<usize> {
        if self.slots.is_empty() {
            return None;
        }
        let mask = self.slots.len() - 1;
        let mut slot = h as usize & mask;
        loop {
            let i = *self.slots.get(slot)? as usize;
            if i == 0 {
                return None;
            }
            if self.hashes.get(i - 1) == Some(&h) {
                let stored = self.names.get(i - 1)?;
                let same = match how {
                    Match::Lower => same_lower(stored, name),
                    Match::Exact => stored.as_slice() == name,
                };
                if same {
                    return Some(i - 1);
                }
            }
            slot = (slot + 1) & mask;
        }
    }

    fn place(slots: &mut [u32], h: u32, index: usize) {
        let mask = slots.len() - 1;
        let mut slot = h as usize & mask;
        while slots[slot] != 0 {
            slot = (slot + 1) & mask;
        }
        slots[slot] = (index + 1) as u32;
    }

    /// Room for one more name at most half full; `Err` leaves it unchanged.
    fn grow(&mut self) -> Result<(), NamesOom> {
        if (self.names.len() + 1) * 2 <= self.slots.len() {
            return Ok(());
        }
        let size = (self.slots.len() * 2).max(16);
        let mut slots: Vec<u32> = crate::falloc::try_vec_filled(size, 0).ok_or(NamesOom)?;
        for (i, &h) in self.hashes.iter().enumerate() {
            Self::place(&mut slots, h, i);
        }
        self.slots = slots;
        Ok(())
    }

    /// The id for `name`, comparing and storing it as `how` says and
    /// appending it at `first + index` when new.
    fn append_by(
        &mut self,
        name: &[u8],
        h: u32,
        first: usize,
        how: Match,
    ) -> Result<usize, NamesOom> {
        if let Some(i) = self.find_by(name, h, how) {
            return Ok(first + i);
        }
        self.grow()?;
        let mut stored: Vec<u8> = try_vec_with_capacity(name.len()).ok_or(NamesOom)?;
        match how {
            Match::Lower => stored.falloc_extend(name.iter().map(u8::to_ascii_lowercase)),
            Match::Exact => stored.falloc_extend_from_slice(name),
        }
        .map_err(|()| NamesOom)?;
        let up = if self.keeps_upper {
            let mut u: Vec<u8> = try_vec_with_capacity(name.len()).ok_or(NamesOom)?;
            u.falloc_extend(name.iter().map(u8::to_ascii_uppercase))
                .map_err(|()| NamesOom)?;
            Some(u)
        } else {
            None
        };
        /* Every slot first, before any table changes, so the tables stay the
         * same length: each a fallible reserve, then fills that cannot fail. */
        let upper_slot = match up {
            Some(_) => Some(self.uppers.falloc_slot().map_err(|()| NamesOom)?),
            None => None,
        };
        let name_slot = self.names.falloc_slot().map_err(|()| NamesOom)?;
        let hash_slot = self.hashes.falloc_slot().map_err(|()| NamesOom)?;
        let index = name_slot.index();
        self.longest = self.longest.max(stored.len());
        self.stored += stored.len() * if up.is_some() { 2 } else { 1 };
        name_slot.push(stored);
        hash_slot.push(h);
        if let (Some(slot), Some(u)) = (upper_slot, up) {
            /* Every entry has one in this table (`keeps_upper`), so the
             * index is the same. */
            slot.push(u);
        }
        Self::place(&mut self.slots, h, index);
        Ok(first + index)
    }

    fn upper(&self, id: usize, first: usize) -> Option<&[u8]> {
        self.uppers.get(id.checked_sub(first)?).map(Vec::as_slice)
    }

    fn bytes(&self) -> usize {
        self.slots.capacity() * 4
            + self.hashes.capacity() * 4
            + self.names.iter().map(Vec::capacity).sum::<usize>()
            + self.uppers.iter().map(Vec::capacity).sum::<usize>()
            + (self.names.capacity() + self.uppers.capacity()) * core::mem::size_of::<Vec<u8>>()
    }

    fn name(&self, id: usize, first: usize) -> Option<&[u8]> {
        self.names.get(id.checked_sub(first)?).map(Vec::as_slice)
    }
}

/// An [`Appended`] table that matches and stores byte for byte: the
/// namespace and prefix tables' own entries (`html::dom`'s `ExtTable`),
/// numbered from 0.
#[derive(Default)]
pub struct ExactNames(Appended);

impl ExactNames {
    /// The index of `name`, if the table holds it.
    pub fn find(&self, name: &[u8]) -> Option<usize> {
        self.0.find_by(name, hash_lower(name), Match::Exact)
    }

    /// The index of `name`, appended when new.
    pub fn intern(&mut self, name: &[u8]) -> Result<usize, NamesOom> {
        self.0.append_by(name, hash_lower(name), 0, Match::Exact)
    }

    /// The entry at `index`.
    pub fn get(&self, index: usize) -> Option<&[u8]> {
        self.0.name(index, 0)
    }

    /// The bytes the table owns.
    pub fn owned_bytes(&self) -> usize {
        self.0.bytes()
    }

    /// The bytes of its entries, in O(1).
    pub fn stored_bytes(&self) -> usize {
        self.0.stored
    }

    /// The longest entry's length.
    pub fn longest(&self) -> usize {
        self.0.longest
    }
}

/// A document's interned tag and attribute names.
pub struct Names {
    tags: Appended,
    attrs: Appended,
    /// Qualified names as written (Lexbor's `qualified_name` entries, which
    /// it keeps in the tag and attribute tables, raw), with their upper-case
    /// forms. Ids from 1.
    quals: Appended,
}

impl Default for Names {
    /// Tags and qualified names keep their upper-case forms (the HTML
    /// `tagName`); attribute local names need none.
    fn default() -> Self {
        Names {
            tags: Appended::new(true),
            attrs: Appended::new(false),
            quals: Appended::new(true),
        }
    }
}

/// The first id an appended tag gets. Lexbor reserves `LAST_ENTRY` itself
/// (its by-id lookup answers "none" for it), so appended ids start past it.
pub const FIRST_APPENDED_TAG: usize = res::TAG__LAST_ENTRY + 1;
/// As [`FIRST_APPENDED_TAG`], for attribute names - in a range of their own,
/// far past every tag id. Lexbor's appended ids are pointers into two
/// different tables, so an appended attribute id never equals a tag id; a
/// node's `local_name` is read as a tag id whatever kind the node is, and the
/// two must not meet there either.
pub const FIRST_APPENDED_ATTR: usize = 0x4000_0000;

impl Names {
    /// `lxb_tag_append_lower`: the tag id for `name`, ASCII case-insensitively,
    /// appending it (lower-cased) when Lexbor's static table lacks it.
    #[inline]
    pub fn tag_append_lower(&mut self, name: &[u8]) -> Result<usize, NamesOom> {
        let h = hash_lower(name);
        if let Some(id) = static_id(&TAG_SLOTS, &res::TAG_NAMES, name, h) {
            return Ok(id);
        }
        self.tags
            .append_by(name, h, FIRST_APPENDED_TAG, Match::Lower)
    }

    /// `lxb_dom_attr_local_name_append`: as [`tag_append_lower`](Self::tag_append_lower),
    /// for an attribute's local name.
    #[inline]
    pub fn attr_local_name_append(&mut self, name: &[u8]) -> Result<usize, NamesOom> {
        let h = hash_lower(name);
        if let Some(id) = static_id(&ATTR_SLOTS, &res::ATTR_NAMES, name, h) {
            return Ok(id);
        }
        self.attrs
            .append_by(name, h, FIRST_APPENDED_ATTR, Match::Lower)
    }

    /// `lxb_tag_append` with a new name: the tag id for `name` EXACTLY as
    /// written, appended as written when new - the tag `createElementNS`
    /// gives an HTML element whose name has upper case, so that `BR` is not
    /// the void `br`. A lower-casing lookup never finds such an entry.
    pub fn tag_append_raw(&mut self, name: &[u8]) -> Result<usize, NamesOom> {
        let h = hash_lower(name);
        if !has_upper(name) {
            if let Some(id) = static_id(&TAG_SLOTS, &res::TAG_NAMES, name, h) {
                return Ok(id);
            }
        }
        self.tags
            .append_by(name, h, FIRST_APPENDED_TAG, Match::Exact)
    }

    /// The id of the qualified name `name` as written, interned when new.
    pub fn qualified_append(&mut self, name: &[u8]) -> Result<u32, NamesOom> {
        let h = hash_lower(name);
        let id = self.quals.append_by(name, h, 1, Match::Exact)?;
        u32::try_from(id).map_err(|_| NamesOom)
    }

    /// The id of the qualified name `name` as written, when it is interned:
    /// the one [`qualified_append`](Self::qualified_append) gave it. Written
    /// names are interned exactly, so two ids are equal when, and only when,
    /// their texts are.
    pub fn qualified_id(&self, name: &[u8]) -> Option<u32> {
        let i = self.quals.find_by(name, hash_lower(name), Match::Exact)?;
        u32::try_from(1 + i).ok()
    }

    /// A qualified name's text.
    pub fn qualified(&self, id: u32) -> Option<&[u8]> {
        self.quals.name(id as usize, 1)
    }

    /// A qualified name's ASCII upper-case form.
    pub fn qualified_upper(&self, id: u32) -> Option<&[u8]> {
        self.quals.upper(id as usize, 1)
    }

    /// A tag id's upper-case name: Lexbor's static one, or an appended
    /// tag's.
    pub fn tag_upper(&self, id: usize) -> Option<&[u8]> {
        match res::TAG_UPPER_NAMES.get(id) {
            Some(n) => (!n.is_empty()).then_some(*n),
            None => self.tags.upper(id, FIRST_APPENDED_TAG),
        }
    }

    /// The attribute id for `name` if it is already known
    /// (`lxb_dom_attr_data_by_local_name`), without appending.
    pub fn attr_id(&self, name: &[u8]) -> Option<usize> {
        let h = hash_lower(name);
        static_id(&ATTR_SLOTS, &res::ATTR_NAMES, name, h)
            .or_else(|| self.attrs.find(name, h).map(|i| FIRST_APPENDED_ATTR + i))
    }

    /// The longest name any table holds, static ones included.
    pub fn longest(&self) -> usize {
        STATIC_LONGEST
            .max(self.tags.longest)
            .max(self.attrs.longest)
            .max(self.quals.longest)
    }

    /// Bytes the tables own.
    pub fn owned_bytes(&self) -> usize {
        self.tags.bytes() + self.attrs.bytes() + self.quals.bytes()
    }

    /// The bytes of the names appended so far, in O(1): what the tables grow
    /// by, for deciding when to measure [`owned_bytes`](Self::owned_bytes)
    /// again.
    pub fn stored_bytes(&self) -> usize {
        self.tags.stored + self.attrs.stored + self.quals.stored
    }

    /// The tag id for `name` if it is already known (`lxb_tag_id_by_name`),
    /// without appending.
    pub fn tag_id(&self, name: &[u8]) -> Option<usize> {
        let h = hash_lower(name);
        static_id(&TAG_SLOTS, &res::TAG_NAMES, name, h)
            .or_else(|| self.tags.find(name, h).map(|i| FIRST_APPENDED_TAG + i))
    }

    /// A tag id's lower-case name.
    pub fn tag_name(&self, id: usize) -> Option<&[u8]> {
        match res::TAG_NAMES.get(id) {
            Some(n) => Some(n),
            None => self.tags.name(id, FIRST_APPENDED_TAG),
        }
    }

    /// An attribute id's lower-case local name.
    pub fn attr_name(&self, id: usize) -> Option<&[u8]> {
        match res::ATTR_NAMES.get(id) {
            Some(n) => Some(n),
            None => self.attrs.name(id, FIRST_APPENDED_ATTR),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_methods)] /* test inputs */
    use super::*;

    #[test]
    fn static_names_keep_lexbor_ids_and_others_are_appended_once() {
        let mut n = Names::default();
        assert_eq!(n.tag_append_lower(b"DiV"), Ok(res::TAG_DIV));
        let x = n.tag_append_lower(b"my-Element").expect("appends");
        assert!(x >= FIRST_APPENDED_TAG);
        assert_eq!(n.tag_append_lower(b"MY-ELEMENT"), Ok(x));
        assert_eq!(n.tag_name(x), Some(&b"my-element"[..]));
        assert_eq!(n.tag_id(b"my-element"), Some(x));
        assert_eq!(n.tag_id(b"other"), None);
        assert_eq!(n.attr_local_name_append(b"CLASS"), Ok(res::ATTR_CLASS));
        assert_eq!(n.attr_local_name_append(b"#undef"), Ok(res::ATTR__UNDEF));
        let a = n.attr_local_name_append(b"data-x").expect("appends");
        assert_eq!(n.attr_name(a), Some(&b"data-x"[..]));
    }

    #[test]
    fn every_static_name_resolves_to_its_own_id() {
        let mut n = Names::default();
        for (id, name) in res::TAG_NAMES.iter().enumerate() {
            /* Lexbor's static lookup lower-cases the input and compares it
             * with the stored key, so a key with upper case is never found. */
            if name.iter().any(u8::is_ascii_uppercase) {
                assert_ne!(n.tag_id(name), Some(id));
                continue;
            }
            assert_eq!(
                n.tag_append_lower(name),
                Ok(id),
                "{}",
                String::from_utf8_lossy(name)
            );
        }
        for (id, name) in res::ATTR_NAMES.iter().enumerate() {
            assert_eq!(n.attr_local_name_append(name), Ok(id));
        }
    }

    #[test]
    fn the_static_longest_is_the_tables_longest() {
        let longest = res::TAG_NAMES
            .iter()
            .chain(res::ATTR_NAMES.iter())
            .map(|n| n.len())
            .max();
        assert_eq!(Some(STATIC_LONGEST), longest);
        assert_eq!(Names::default().longest(), STATIC_LONGEST);
    }

    #[test]
    fn many_appended_names_survive_growth() {
        let mut n = Names::default();
        let ids: Vec<usize> = (0..1000)
            .map(|i| {
                n.tag_append_lower(format!("x-{i}").as_bytes())
                    .expect("appends")
            })
            .collect();
        for (i, id) in ids.iter().enumerate() {
            assert_eq!(n.tag_id(format!("X-{i}").as_bytes()), Some(*id));
        }
    }
}
