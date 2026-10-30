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

/// The constant `name` of `outer` (`rb_const_get`): raises `NameError` for
/// none, and an autoload runs Ruby code - for a class the extension's own
/// Ruby defines.
pub fn const_get(ruby: &Ruby, outer: Value, name: &core::ffi::CStr) -> Result<Value, Error> {
    let id = intern_cstr(ruby, name)?.as_raw();
    let outer = outer.as_raw();
    let v = call(|| unsafe { rb_sys::rb_const_get(outer, id) })?;
    // SAFETY: the constant's value, live while it is bound.
    Ok(unsafe { Value::from_raw(v) })
}

/// `recv.name(*args)` (`rb_funcallv`): the caller's Ruby, which is why each
/// call of it is allowed where it is made (`rake dylint`'s
/// `runs_callers_ruby`), with the reason it may run there.
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        runs_callers_ruby,
        reason = "rb_funcallv: the wrapper `Module::call_method` is, and its callers allow it"
    )
)]
pub fn call_method(
    ruby: &Ruby,
    recv: Value,
    name: &core::ffi::CStr,
    args: &[Value],
) -> Result<Value, Error> {
    let id = intern_cstr(ruby, name)?.as_raw();
    let argc = c_int::try_from(args.len())
        .map_err(|_| crate::rb::error::arg_error("too many arguments"))?;
    let recv = recv.as_raw();
    /* `Value` is transparent over `VALUE`, so the slice is the C array. */
    let argv = args.as_ptr() as *const VALUE;
    let v = call(|| unsafe { rb_sys::rb_funcallv(recv, id, argc, argv) })?;
    // SAFETY: what the method returned, live on this frame.
    Ok(unsafe { Value::from_raw(v) })
}

/// `rb_module_new`: an anonymous module. Raises `NoMemoryError`.
pub fn module_new(_: &Ruby) -> Result<RModule, Error> {
    let v = call(|| unsafe {
        injected_raise();
        rb_sys::rb_module_new()
    })?;
    typed(v, RModule::from_value, "a C call made no Module")
}
