//! Calling a method of any object: the caller's Ruby, run when an
//! extension asks for it.
//!
//! Nupur runs none of the caller's Ruby on its own: no argument is converted
//! (`args`), no `to_s` is called for a message (`describe`). An extension
//! that does call a method (a JSON generator calling an object's `to_json`)
//! does it here, and `rake dylint`'s `runs_callers_ruby` reports each call
//! site, so each carries an `allow` that says why it may run there.
//!
//! The call is protected as every raising C call is: a raise, a `throw`, a
//! `break` out of a block the method yields to comes back as `Err` and is
//! resumed by the method's trampoline. What the call may do besides - call
//! back into the extension, change the Hash or Array being walked, switch to
//! another thread or Fiber - is the extension's to allow for, and the rule
//! that keeps it safe is one: hold nothing across the call that the callee
//! could invalidate.
//!
//! - No borrow of a wrapped object's payload (`Typed::data()`), unless what
//!   it borrows is only ever changed through a `Cell`; and no
//!   `RefCell` guard - a call back into the extension would panic on it.
//! - No guard of the extension's own (Makiri's document guard is one).
//! - No slice of a Ruby collection: read an Array by index, its length
//!   again at each step. A Hash may be walked in place (`hash::hash_foreach`
//!   is `rb_hash_foreach`, the walk `Hash#each` is), and Ruby guards it as
//!   it guards a block: a key added during the walk raises. Copy its pairs
//!   first (`hash::each_pair`) only where a walk must not raise.
//!
//! A Fiber switched away from inside the call and never resumed leaves the
//! Rust frames under it unfinished: their values are leaked, never freed
//! twice or used after.

#![forbid(unsafe_code)]

use std::sync::OnceLock;

use super::{Error, Id, ReprValue, Ruby, Value};

/// `recv.public_send(id, *args)`: a public method only - a private or
/// protected one is `NoMethodError`, as `public_send` answers - which is
/// what a library calling an object's conversion (`to_json`, `to_s`) means.
#[cfg_attr(
    dylint_lib = "nupur_lints",
    expect(
        runs_callers_ruby,
        reason = "the one way to call a public method; each caller's own allow says why"
    )
)]
pub fn public_method(recv: impl ReprValue, id: Id, args: &[Value]) -> Result<Value, Error> {
    crate::raw::call_public(recv.as_value(), id, args)
}

/// Whether `recv` has a public method `id`: Ruby's own `respond_to?`, which
/// a class may redefine (so the caller's Ruby too), and which a
/// `respond_to_missing?` answers for a method `method_missing` handles.
#[cfg_attr(
    dylint_lib = "nupur_lints",
    expect(
        runs_callers_ruby,
        reason = "respond_to? may be redefined; each caller's own allow says why"
    )
)]
pub fn responds_to(recv: impl ReprValue, id: Id) -> Result<bool, Error> {
    crate::raw::respond_to(recv.as_value(), id)
}

/// A method name interned once (`rb_intern`) and kept: `IdConst::new(c"to_json")`
/// in a static, read with [`get`](IdConst::get). An interned name is never
/// collected and is the same integer in every Ractor, so the static is
/// shared freely. A failed intern (`NoMemoryError`) is `Err`, and the next
/// read tries again.
pub struct IdConst {
    name: &'static core::ffi::CStr,
    id: OnceLock<Id>,
}

impl IdConst {
    pub const fn new(name: &'static core::ffi::CStr) -> IdConst {
        IdConst {
            name,
            id: OnceLock::new(),
        }
    }

    /// The interned name.
    pub fn get(&self, ruby: &Ruby) -> Result<Id, Error> {
        if let Some(id) = self.id.get() {
            return Ok(*id);
        }
        let id = Id::intern(ruby, self.name)?;
        Ok(*self.id.get_or_init(|| id))
    }
}
