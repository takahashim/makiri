//! Namespace introspection, Nokogiri-compatible.
//!
//! `Makiri::XML::Namespace` is a small (prefix, href) value object. xmlns
//! declarations are stored as ordinary attribute nodes - qname `xmlns` or
//! `xmlns:PREFIX` - so all four queries below are just tree reads.

use magnus::rb_sys::{AsRawValue, FromRawValue};
use magnus::{prelude::*, Error, RArray, RClass, RHash, RString, Ruby, Value};
use std::sync::OnceLock;

use super::abi::*;

/// The `Makiri::XML::Namespace` class, stashed at init.
static NAMESPACE_CLASS: OnceLock<rb_sys::VALUE> = OnceLock::new();

pub fn set_namespace_class(klass: RClass) {
    NAMESPACE_CLASS
        .set(klass.as_raw())
        .expect("Makiri::XML::Namespace is initialized once");
}

unsafe fn namespace_class() -> RClass {
    RClass::from_value(Value::from_raw(
        *NAMESPACE_CLASS
            .get()
            .expect("Makiri::XML::Namespace initialized"),
    ))
    .expect("Makiri::XML::Namespace")
}

fn ivar_ids() -> (rb_sys::ID, rb_sys::ID) {
    static IDS: OnceLock<(rb_sys::ID, rb_sys::ID)> = OnceLock::new();
    *IDS.get_or_init(|| {
        // SAFETY: namespace methods run under the GVL; Ruby interns IDs for
        // the VM lifetime.
        unsafe {
            (
                rb_sys::rb_intern(c"@prefix".as_ptr()),
                rb_sys::rb_intern(c"@href".as_ptr()),
            )
        }
    })
}

pub unsafe fn new_ns(prefix: Value, href: Value) -> Result<Value, Error> {
    let (p_id, h_id) = ivar_ids();
    let ns = rb_sys::rb_obj_alloc(namespace_class().as_raw());
    rb_sys::rb_ivar_set(ns, p_id, prefix.as_raw());
    rb_sys::rb_ivar_set(ns, h_id, href.as_raw());
    Ok(Value::from_raw(ns))
}

pub fn ns_prefix(rb_self: Value) -> Result<Value, Error> {
    unsafe {
        Ok(Value::from_raw(rb_sys::rb_ivar_get(
            rb_self.as_raw(),
            ivar_ids().0,
        )))
    }
}

pub fn ns_href(rb_self: Value) -> Result<Value, Error> {
    unsafe {
        Ok(Value::from_raw(rb_sys::rb_ivar_get(
            rb_self.as_raw(),
            ivar_ids().1,
        )))
    }
}

pub fn ns_equal(rb_self: Value, other: Value) -> Result<bool, Error> {
    if !other.is_kind_of(unsafe { namespace_class() }) {
        return Ok(false);
    }
    Ok(ns_prefix(rb_self)?.eql(ns_prefix(other)?)? && ns_href(rb_self)?.eql(ns_href(other)?)?)
}

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
fn xmlns_decl(d: &XmlDoc, a: NodeId) -> Option<(&[u8], &[u8])> {
    let p = crate::xml::qname::xmlns_prefix(d.qname(a))?;
    Some((p, d.value(a)))
}

/// `#namespace` - the node's own resolved namespace, or nil.
pub fn namespace(ruby: &Ruby, this: super::XmlSelf) -> Result<Value, Error> {
    unsafe {
        let d = &*this.doc();
        let id = this.id;
        if !matches!(d.type_(id), Some(NodeType::Element | NodeType::Attribute))
            || d.node(id).ns_uri.len == 0
        {
            return Ok(ruby.qnil().as_value());
        }
        let prefix = if d.node(id).prefix.len == 0 {
            ruby.qnil().as_value()
        } else {
            str_span(ruby, d, d.node(id).prefix)
        };
        new_ns(prefix, str_span(ruby, d, d.node(id).ns_uri))
    }
}

/// `#namespace_definitions` - the declarations made ON this element.
pub fn namespace_definitions(ruby: &Ruby, this: super::XmlSelf) -> Result<RArray, Error> {
    let arr = ruby.ary_new();
    unsafe {
        let d = &*this.doc();
        let id = this.id;
        if d.type_(id) == Some(NodeType::Element) {
            let mut a = d.attrs(id);
            while let Some(at) = a {
                if let Some((p, u)) = xmlns_decl(d, at) {
                    let prefix = if p.is_empty() {
                        ruby.qnil().as_value()
                    } else {
                        utf8(ruby, p).as_value()
                    };
                    arr.push(new_ns(prefix, utf8(ruby, u).as_value())?)?;
                }
                a = d.next(at);
            }
        }
    }
    Ok(arr)
}

/// `#namespaces` - every declaration in scope here, keyed by the declaring
/// attribute's name. The inner scope wins because the first binding seen is kept.
pub fn namespaces(ruby: &Ruby, this: super::XmlSelf) -> Result<RHash, Error> {
    let h = ruby.hash_new();
    unsafe {
        let d = &*this.doc();
        let mut e = Some(this.id);
        while let Some(id) = e {
            if d.type_(id) == Some(NodeType::Element) {
                let mut a = d.attrs(id);
                while let Some(at) = a {
                    if let Some((_, u)) = xmlns_decl(d, at) {
                        let key = str_span(ruby, d, d.node(at).qname);
                        if h.get(key).is_none() {
                            h.aset(key, utf8(ruby, u))?;
                        }
                    }
                    a = d.next(at);
                }
            }
            e = d.parent(id);
        }
    }
    Ok(h)
}

/// `#collect_namespaces` - every declaration anywhere in the document, pre-order
/// through the tree (no recursion).
pub fn collect_namespaces(ruby: &Ruby, this: super::XmlSelf) -> Result<RHash, Error> {
    let h = ruby.hash_new();
    unsafe {
        let d = &*this.doc();
        let mut root = this.id;
        while let Some(p) = d.parent(root) {
            root = p;
        }
        let mut cur = Some(root);
        while let Some(id) = cur {
            if d.type_(id) == Some(NodeType::Element) {
                let mut a = d.attrs(id);
                while let Some(at) = a {
                    if let Some((_, u)) = xmlns_decl(d, at) {
                        h.aset(str_span(ruby, d, d.node(at).qname), utf8(ruby, u))?;
                    }
                    a = d.next(at);
                }
            }
            if d.first_child(id).is_some() {
                cur = d.first_child(id);
                continue;
            }
            /* Climb until a non-root node with a next sibling is found. */
            let mut climbed = None;
            let mut n = id;
            loop {
                if n == root {
                    climbed = Some(root);
                    break;
                }
                match d.next(n) {
                    Some(nx) => {
                        climbed = Some(nx);
                        break;
                    }
                    None => match d.parent(n) {
                        Some(p) => n = p,
                        None => break,
                    },
                }
            }
            match climbed {
                None => break,
                Some(n) if n == root => break,
                Some(nx) => cur = Some(nx),
            }
        }
    }
    Ok(h)
}
