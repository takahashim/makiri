//! What an object is asked or told outside a method call: its hidden
//! instance variables, its hiding from `ObjectSpace`, and whether it is an
//! instance of a class - each a C read or a protected call that runs no Ruby
//! code.

#![allow(unsafe_code)]

use rb_sys::VALUE;

use super::error::Error;
use super::value::{AsRawValue, HiddenId, RClass, RModule, ReprValue, Value};

/// `rb_attr_get`: a hidden ivar's value, or nil. Runs no Ruby code (an
/// attribute read, not a method call), and raises nothing in any Ractor (a
/// [`HiddenId`] is a name the Ractor checks do not apply to).
#[inline]
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        raising_call_outside_protect,
        reason = "rb_attr_get raises only RactorIsolationError, and only for an @ name (rb_is_instance_id) of a class, a module or a shareable object read from another Ractor; a HiddenId is never an @ name"
    )
)]
pub fn ivar_get(obj: Value, id: HiddenId) -> Value {
    // SAFETY: a live object; the read raises nothing.
    unsafe { Value::from_raw(rb_sys::rb_attr_get(obj.as_raw(), id.as_raw())) }
}

/// Set the hidden ivar `id` (a name without `@`, so Ruby code cannot see it)
/// of `obj` to `v`: `FrozenError` for a frozen object, and `NoMemoryError`.
#[inline]
pub fn ivar_set(obj: Value, id: HiddenId, v: impl ReprValue) -> Result<(), Error> {
    crate::raw::ivar_set(obj, id, v.as_value())
}

/// `rb_obj_hide`: take `v`'s class away, so `ObjectSpace` does not list it.
///
/// # Safety
/// `v` is an object no Ruby code holds and none will be handed while it is
/// hidden: a hidden object has no class, and Ruby code given one crashes.
#[inline]
pub(crate) unsafe fn obj_hide(v: Value) {
    // SAFETY: a live object; hiding writes only its class field.
    unsafe { rb_sys::rb_obj_hide(v.as_raw()) };
}

/// Whether `v` is a class or a module - what `rb_obj_is_kind_of` and
/// `rb_class_inherited_p` require, and raise `TypeError` without.
#[inline]
fn is_module(v: Value) -> bool {
    RClass::from_value(v).is_some() || RModule::from_value(v).is_some()
}

/// `rb_obj_is_kind_of`: an instance of `class` or of one deriving from it?
/// `false` when `class` is not a class or module, rather than the
/// `TypeError` the C function raises. Runs no Ruby code.
#[inline]
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        raising_call_outside_protect,
        reason = "its one raise, a non-class, is ruled out above"
    )
)]
pub(crate) fn obj_is_kind_of(v: Value, class: Value) -> bool {
    if !is_module(class) {
        return false;
    }
    // SAFETY: both live; `class` is a class or module, so this raises nothing.
    unsafe { rb_sys::rb_obj_is_kind_of(v.as_raw(), class.as_raw()) == rb_sys::Qtrue as VALUE }
}

/// `rb_class_inherited_p`: is `sub` `sup` or a descendant of it? `false`
/// when either is not a class or module. Runs no Ruby code.
#[inline]
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        raising_call_outside_protect,
        reason = "its one raise, a non-class, is ruled out above"
    )
)]
pub(crate) fn class_inherits(sub: Value, sup: Value) -> bool {
    if !is_module(sub) || !is_module(sup) {
        return false;
    }
    // SAFETY: both live modules, so this raises nothing.
    unsafe { rb_sys::rb_class_inherited_p(sub.as_raw(), sup.as_raw()) == rb_sys::Qtrue as VALUE }
}
