//! Verus proofs over `src/limits.rs`'s `admit`: the one rule every work
//! budget keeps - XPath's evaluator ops, AST nodes and recursion depth, and
//! the CSS matcher's steps, each charged through it.
//!
//! Proved over the shipped code (`admit` is copied, and `rake verus:sync`
//! checks it against the source): an admitted charge is exactly `count + n`
//! and never past `cap`, so the add cannot overflow; a refusal is exactly a
//! charge that would pass `cap`. And over any sequence of charges from a
//! count within the cap - zero, as each budget starts - the count never
//! passes it (`run_stays_within`): the work a budget admits is at most its
//! cap, whatever the order or size of the charges.
//!
//! The budgets hold the count in a `Cell` and store `admit`'s answer in it;
//! that store is not copied here (Verus has no `Cell` through `&self`), and is
//! one `set` per charge in `xpath::limits::take` and `css_match::Budget::charge`.
//!
//! Nothing is assumed: no `external_body`, no `assume_specification`.

use vstd::prelude::*;

verus! {

pub fn admit(count: usize, n: usize, cap: usize) -> (r: Option<usize>)
    ensures // [verus]
        r.is_some() <==> count <= cap && n <= cap - count, // [verus]
        r.is_some() ==> r.unwrap() == count + n && r.unwrap() <= cap, // [verus]
{
    if count > cap || cap - count < n {
        // [canary]
        return None;
    }
    // [canary]
    Some(count + n)
}

/// The count after the charges `ns`, each admitted or refused by `admit`'s
/// rule: what a budget's counter holds after a run.
pub open spec fn run(count: int, ns: Seq<int>, cap: int) -> int
    decreases ns.len(),
{
    if ns.len() == 0 {
        count
    } else {
        let next = if count <= cap && 0 <= ns[0] && ns[0] <= cap - count {
            count + ns[0]
        } else {
            count
        };
        run(next, ns.drop_first(), cap)
    }
}

/// From a count within the cap, no sequence of charges takes it past.
proof fn run_stays_within(count: int, ns: Seq<int>, cap: int)
    requires
        0 <= count <= cap,
    ensures
        0 <= run(count, ns, cap) <= cap,
    decreases ns.len(),
{
    if ns.len() > 0 {
        let next = if count <= cap && 0 <= ns[0] && ns[0] <= cap - count {
            count + ns[0]
        } else {
            count
        };
        run_stays_within(next, ns.drop_first(), cap);
    }
}

/// `admit` is `run`'s step: what the budgets do, one charge at a time.
proof fn admit_is_run_step(count: usize, n: usize, cap: usize, r: Option<usize>)
    requires
        r.is_some() <==> count <= cap && n <= cap - count,
        r.is_some() ==> r.unwrap() == count + n,
    ensures
        run(count as int, seq![n as int], cap as int) == if r.is_some() {
            r.unwrap() as int
        } else {
            count as int
        },
{
    reveal_with_fuel(run, 2);
    assert(seq![n as int].drop_first() =~= Seq::<int>::empty());
}

} // verus!

fn main() {}
