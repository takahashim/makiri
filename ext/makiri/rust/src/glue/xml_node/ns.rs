//! Namespace introspection, Nokogiri-compatible.
//!
//! `Makiri::XML::Namespace` is a small (prefix, href) value object. xmlns
//! declarations are stored as ordinary attribute nodes - qname `xmlns` or
//! `xmlns:PREFIX` - so all four queries below are just tree reads:
//!
//!   `#namespace`             the node's own resolved namespace, or nil
//!   `#namespace_definitions` the xmlns declarations ON this element
//!   `#namespaces`            every declaration IN SCOPE here, as a Hash
//!   `#collect_namespaces`    every declaration in the document, as a Hash

use magnus::rb_sys::{AsRawValue, FromRawValue};
use magnus::{prelude::*, Error, RArray, RClass, RHash, RString, Ruby, Value};

use super::abi::*;
use super::unwrap;

/// The `Makiri::XML::Namespace` class, stashed at init so the value object can
/// be built without a constant lookup per call.
static mut NAMESPACE_CLASS: rb_sys::VALUE = 0;

/// # Safety
/// Called once from init, on the Ruby thread.
pub unsafe fn set_namespace_class(klass: RClass) {
    NAMESPACE_CLASS = klass.as_raw();
}

unsafe fn namespace_class() -> RClass {
    RClass::from_value(Value::from_raw(NAMESPACE_CLASS)).expect("Makiri::XML::Namespace")
}

/// The two ivar names, interned once. `rb_intern` on every call would be a
/// hash lookup per namespace read.
unsafe fn ivar_ids() -> (rb_sys::ID, rb_sys::ID) {
    static mut IDS: (rb_sys::ID, rb_sys::ID) = (0, 0);
    if IDS.0 == 0 {
        IDS = (
            rb_sys::rb_intern(c"@prefix".as_ptr()),
            rb_sys::rb_intern(c"@href".as_ptr()),
        );
    }
    IDS
}

/// A (prefix, href) pair as a `Makiri::XML::Namespace`.
///
/// # Safety
/// After init.
pub unsafe fn new_ns(prefix: Value, href: Value) -> Result<Value, Error> {
    let (p_id, h_id) = ivar_ids();
    let ns = rb_sys::rb_obj_alloc(namespace_class().as_raw());
    rb_sys::rb_ivar_set(ns, p_id, prefix.as_raw());
    rb_sys::rb_ivar_set(ns, h_id, href.as_raw());
    Ok(Value::from_raw(ns))
}

pub fn ns_prefix(rb_self: Value) -> Result<Value, Error> {
    unsafe { Ok(Value::from_raw(rb_sys::rb_ivar_get(rb_self.as_raw(), ivar_ids().0))) }
}

pub fn ns_href(rb_self: Value) -> Result<Value, Error> {
    unsafe { Ok(Value::from_raw(rb_sys::rb_ivar_get(rb_self.as_raw(), ivar_ids().1))) }
}

pub fn ns_equal(rb_self: Value, other: Value) -> Result<bool, Error> {
    if !unsafe { is_a(other, namespace_class().as_raw()) } {
        return Ok(false);
    }
    Ok(ns_prefix(rb_self)?.eql(ns_prefix(other)?)? && ns_href(rb_self)?.eql(ns_href(other)?)?)
}

/// The pair's hash, so two equal Namespaces hash alike.
pub fn ns_hash(ruby: &Ruby, rb_self: Value) -> Result<Value, Error> {
    let pair = ruby.ary_new_from_values(&[ns_prefix(rb_self)?, ns_href(rb_self)?]);
    pair.funcall("hash", ())
}

pub fn ns_inspect(ruby: &Ruby, rb_self: Value) -> Result<RString, Error> {
    Ok(ruby.str_new(&format!(
        "#<Makiri::XML::Namespace prefix={} href={}>",
        ns_prefix(rb_self)?.inspect(),
        ns_href(rb_self)?.inspect()
    )))
}

/// Read an attribute node's xmlns declaration, if it is one: the declared
/// prefix (empty for the default xmlns) and the URI.
///
/// # Safety
/// `a` must be a live attribute node.
unsafe fn xmlns_decl(a: *const Node) -> Option<(&'static [u8], &'static [u8])> {
    let mut p: *const core::ffi::c_char = core::ptr::null();
    let mut u: *const core::ffi::c_char = core::ptr::null();
    let (mut pl, mut ul) = (0u32, 0u32);
    if mkr_xml_node_xmlns_decl(a, &mut p, &mut pl, &mut u, &mut ul) == 0 {
        return None;
    }
    let sl = |ptr: *const core::ffi::c_char, len: u32| -> &'static [u8] {
        if ptr.is_null() || len == 0 {
            &[]
        } else {
            core::slice::from_raw_parts(ptr as *const u8, len as usize)
        }
    };
    Some((sl(p, pl), sl(u, ul)))
}

/// `#namespace` - the node's own resolved namespace, or nil.
pub fn namespace(ruby: &Ruby, rb_self: Value) -> Result<Value, Error> {
    unsafe {
        let n = &*unwrap(rb_self);
        if !matches!(n.type_, T_ELEMENT | T_ATTRIBUTE) || n.ns_uri_len == 0 {
            return Ok(ruby.qnil().as_value());
        }
        let prefix = if n.prefix_len == 0 {
            ruby.qnil().as_value()
        } else {
            str_field(ruby, n.prefix, n.prefix_len)
        };
        new_ns(prefix, str_field(ruby, n.ns_uri, n.ns_uri_len))
    }
}

/// `#namespace_definitions` - the declarations made ON this element.
pub fn namespace_definitions(ruby: &Ruby, rb_self: Value) -> Result<RArray, Error> {
    let arr = ruby.ary_new();
    unsafe {
        let n = &*unwrap(rb_self);
        if n.type_ == T_ELEMENT {
            let mut a = n.attrs;
            while !a.is_null() {
                if let Some((p, u)) = xmlns_decl(a) {
                    let prefix = if p.is_empty() {
                        ruby.qnil().as_value()
                    } else {
                        utf8(ruby, p).as_value()
                    };
                    arr.push(new_ns(prefix, utf8(ruby, u).as_value())?)?;
                }
                a = (*a).next;
            }
        }
    }
    Ok(arr)
}

/// `#namespaces` - every declaration in scope here, keyed by the declaring
/// attribute's name (`xmlns` or `xmlns:p`). Walks outward from this node, and
/// the inner scope wins because the first binding seen for a key is kept.
pub fn namespaces(ruby: &Ruby, rb_self: Value) -> Result<RHash, Error> {
    let h = ruby.hash_new();
    unsafe {
        let mut e = unwrap(rb_self);
        while !e.is_null() {
            if (*e).type_ == T_ELEMENT {
                let mut a = (*e).attrs;
                while !a.is_null() {
                    if let Some((_, u)) = xmlns_decl(a) {
                        let key = str_field(ruby, (*a).qname, (*a).qname_len);
                        if h.get(key).is_none() {
                            h.aset(key, utf8(ruby, u))?;
                        }
                    }
                    a = (*a).next;
                }
            }
            e = (*e).parent;
        }
    }
    Ok(h)
}

/// `#collect_namespaces` - every declaration anywhere in the document.
///
/// Pre-order over the whole tree through the parent pointers, so no recursion
/// and no depth limit. A later declaration of the same name overwrites an
/// earlier one, which is what Nokogiri does.
pub fn collect_namespaces(ruby: &Ruby, rb_self: Value) -> Result<RHash, Error> {
    let h = ruby.hash_new();
    unsafe {
        let mut root = unwrap(rb_self);
        while !(*root).parent.is_null() {
            root = (*root).parent; /* the DOCUMENT node */
        }
        let mut cur = root;
        while !cur.is_null() {
            if (*cur).type_ == T_ELEMENT {
                let mut a = (*cur).attrs;
                while !a.is_null() {
                    if let Some((_, u)) = xmlns_decl(a) {
                        h.aset(str_field(ruby, (*a).qname, (*a).qname_len), utf8(ruby, u))?;
                    }
                    a = (*a).next;
                }
            }
            if !(*cur).first_child.is_null() {
                cur = (*cur).first_child;
                continue;
            }
            while cur != root && (*cur).next.is_null() {
                cur = (*cur).parent;
            }
            if cur == root {
                break;
            }
            cur = (*cur).next;
        }
    }
    Ok(h)
}
