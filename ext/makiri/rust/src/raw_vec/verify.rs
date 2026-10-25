//! Kani proofs over `RawVec`, against an allocator that may fail at any call.

use super::block::MaybeFail;
use super::*;

/// Any sequence of up to three pushes and reserves, each of which may find
/// the allocator failing, keeps the array's contents what was pushed and
/// accepted, in order, with every access in bounds and the block freed
/// once. (Three, and reserves of at most four: four of up to eight ran
/// CI's runner out of memory beside the other harnesses. Verus's
/// `verus/raw_vec.rs` covers any length, over `Block`'s contract.)
#[kani::proof]
#[kani::unwind(10)]
fn any_pushes_and_reserves_keep_the_contents() {
    let mut v: RawVec<u8, MaybeFail> = RawVec::new();
    let mut want = [0u8; 3];
    let mut n = 0usize;
    for _ in 0..3 {
        if kani::any() {
            let x: u8 = kani::any();
            let r = v.push(x);
            kani::cover!(r.is_err(), "a push the allocator refused");
            if r.is_ok() {
                want[n] = x;
                n += 1;
            }
        } else {
            let r: usize = kani::any();
            kani::assume(r <= 4);
            let _ = v.reserve(r);
        }
        assert!(v.len() <= v.capacity());
        assert_eq!(v.as_slice(), &want[..n]);
    }
    kani::cover!(n == 3, "three pushes accepted");
}
