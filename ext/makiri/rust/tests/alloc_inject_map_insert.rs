//! `MapInsert::falloc_insert` must reserve even when it replaces an existing
//! key, because `std`'s `HashMap::insert` calls `reserve(1)` before it looks
//! the key up: a replace in a full table grows the table (measured), so the
//! reserve is a real allocation attempt. Skipping it for an existing key would
//! perform that allocation outside the injection counter, where an OOM aborts
//! the process instead of failing closed.
//!
//! Gated on `alloc-inject`, and an integration test rather than a unit test on
//! purpose: the counter is process-global, so arming it inside the parallel
//! unit-test binary would fail unrelated tests. This binary holds only this
//! test, so the only consultation is the one it asks for.
//!
//!     cargo test --no-default-features --features alloc-inject \
//!         --test alloc_inject_map_insert

#![cfg(feature = "alloc-inject")]

use std::collections::HashMap;

use makiri::falloc::{alloc_inject_arm, alloc_inject_call_count, MapInsert};

#[test]
fn replacing_an_existing_key_in_a_full_map_consults_the_hook() {
    let mut map: HashMap<u8, u8> = HashMap::new();
    // Fill to capacity with plain `std` inserts, which do not consult the hook.
    alloc_inject_arm(0);
    map.insert(0, 0);
    while map.len() < map.capacity() {
        let k = map.len() as u8;
        map.insert(k, k);
    }
    assert_eq!(map.len(), map.capacity(), "the map is full");
    let before: Vec<(u8, u8)> = map.iter().map(|(k, v)| (*k, *v)).collect();

    // Arm the next consultation to fail: replacing an existing key must still
    // reserve, so the insert fails and leaves the map unchanged.
    alloc_inject_arm(1);
    assert_eq!(
        map.falloc_insert(0, 99),
        Err(()),
        "the replace's reserve must consult the hook"
    );
    assert_eq!(alloc_inject_call_count(), 1, "exactly one consultation");
    assert_eq!(map[&0], 0, "the value was not replaced");
    let after: Vec<(u8, u8)> = map.iter().map(|(k, v)| (*k, *v)).collect();
    assert_eq!(before, after, "a failed insert left the map unchanged");
}
