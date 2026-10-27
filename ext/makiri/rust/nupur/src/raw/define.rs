//! Class, module and method definitions, and the constants they hang from, at `Init_`.

use super::*;

/// `rb_define_class_under`: raises `TypeError` when `name` is bound to
/// something else, or to a class with another superclass.
pub fn define_class_under(
    outer: Value,
    name: &core::ffi::CStr,
    sup: RClass,
) -> Result<Value, Error> {
    let (outer, name, sup) = (outer.as_raw(), name.as_ptr(), sup.as_raw());
    let v = call(|| unsafe { rb_sys::rb_define_class_under(outer, name, sup) })?;
    // SAFETY: the class just defined, live (it is a constant now).
    Ok(unsafe { Value::from_raw(v) })
}

/// `rb_define_module_under`: raises `TypeError` when `name` is bound to
/// something that is not a module.
pub fn define_module_under(outer: Value, name: &core::ffi::CStr) -> Result<Value, Error> {
    let (outer, name) = (outer.as_raw(), name.as_ptr());
    let v = call(|| unsafe { rb_sys::rb_define_module_under(outer, name) })?;
    // SAFETY: as `define_class_under`.
    Ok(unsafe { Value::from_raw(v) })
}

/// `rb_define_module`: a top-level module; raises as `define_module_under`.
pub fn define_module(_: &Ruby, name: &core::ffi::CStr) -> Result<Value, Error> {
    let name = name.as_ptr();
    let v = call(|| unsafe { rb_sys::rb_define_module(name) })?;
    // SAFETY: as `define_class_under`.
    Ok(unsafe { Value::from_raw(v) })
}

/// `rb_const_set`: raises `FrozenError` on a frozen namespace.
pub fn const_set(ruby: &Ruby, outer: Value, name: &core::ffi::CStr, v: Value) -> Result<(), Error> {
    let id = intern_cstr(ruby, name)?.as_raw();
    let (outer, v) = (outer.as_raw(), v.as_raw());
    call(|| unsafe { rb_sys::rb_const_set(outer, id, v) })
}

/// `rb_include_module`: raises `ArgumentError` for a cyclic include,
/// `FrozenError` on a frozen class.
pub fn include_module(klass: Value, module: Value) -> Result<(), Error> {
    let (klass, module) = (klass.as_raw(), module.as_raw());
    call(|| unsafe { rb_sys::rb_include_module(klass, module) })
}

/// `rb_undef_alloc_func`: raises `TypeError` for a singleton class, which
/// may not have an allocator of its own taken.
pub fn undef_alloc_func(klass: RClass) -> Result<(), Error> {
    let k = klass.as_raw();
    call(|| unsafe { rb_sys::rb_undef_alloc_func(k) })
}

/// `rb_obj_freeze`: raises `NoMemoryError` - freezing moves an object with
/// instance variables to a frozen shape, which may allocate.
pub fn obj_freeze(v: Value) -> Result<(), Error> {
    let v = v.as_raw();
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_obj_freeze(v)
    })
    .map(|_| ())
}

/// `rb_obj_reveal`: show `obj`, hidden by `rb_obj_hide`, as an instance of
/// `klass`. Raises `NoMemoryError`: the class is written with a write
/// barrier, which may grow the remembered set (found by the C API raise
/// survey, takahashim.github.io/ruby-c-apis - Ruby's header says nothing of
/// it). On `Err` the object stays hidden.
pub fn obj_reveal(obj: Value, klass: Value) -> Result<(), Error> {
    let (o, k) = (obj.as_raw(), klass.as_raw());
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_obj_reveal(o, k)
    })
    .map(|_| ())
}

/// `rb_define_method` / `_private_method` / `_singleton_method`: raise
/// `FrozenError` on a frozen target.
pub(crate) fn define_method(
    target: Value,
    name: &core::ffi::CStr,
    func: crate::rb::method::Func,
    visibility: crate::rb::class::Visibility,
) -> Result<(), Error> {
    use crate::rb::class::Visibility;
    let (target, name, ptr, arity) = (target.as_raw(), name.as_ptr(), Some(func.ptr), func.arity);
    match visibility {
        Visibility::Public => {
            call(|| unsafe { rb_sys::rb_define_method(target, name, ptr, arity) })
        }
        Visibility::Private => {
            call(|| unsafe { rb_sys::rb_define_private_method(target, name, ptr, arity) })
        }
        Visibility::Singleton => {
            call(|| unsafe { rb_sys::rb_define_singleton_method(target, name, ptr, arity) })
        }
        Visibility::ModuleFunction => {
            call(|| unsafe { rb_sys::rb_define_module_function(target, name, ptr, arity) })
        }
    }
}

/// `rb_gc_register_mark_object`: raises `NoMemoryError` when the root table
/// cannot grow.
pub fn gc_register_mark_object(v: Value) -> Result<(), Error> {
    let v = v.as_raw();
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_gc_register_mark_object(v)
    })
}
