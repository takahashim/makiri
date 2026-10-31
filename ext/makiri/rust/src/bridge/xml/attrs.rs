//! An element's attribute found by qualified name, or by namespace and local
//! name.

#![forbid(unsafe_code)]

use super::*;

/* ------------------------------------------------------------------ */
/* attribute lookup                                                   *
 * ------------------------------------------------------------------ */

/// The attribute of `el` whose qualified name is exactly the verified `name`
/// (`AttrKey::find_in`, which counts namespace declarations as the DOM does).
///
/// `None` for a non-element (the name is then not even verified, matching the
/// readers' nil-returning behaviour). The name is converted BEFORE the arena is
/// borrowed: a Symbol's name or a transcode is a Ruby allocation.
pub fn find_attribute(ruby: &Ruby, this: &XmlSelf, name: Value) -> Result<Option<NodeId>, Error> {
    let id = this.id;
    if this.read()?.type_(id) != Some(ArenaKind::Element) {
        return Ok(None);
    }
    let nv = ruby_verified_text(ruby, name, "attribute name")?;
    let view = this.read()?;
    Ok(AttrKey::QName(nv.as_bytes()).find_in(&view, id))
}

/// The attribute of `el` in namespace `ns` (nil or "" for none) with local
/// name `local` - DOM "get an attribute by namespace and local name", the key
/// `remove_attribute_ns` removes by. Converted before the arena is borrowed,
/// as [`find_attribute`].
pub fn find_attribute_ns(
    ruby: &Ruby,
    this: &XmlSelf,
    ns: Value,
    local: Value,
) -> Result<Option<NodeId>, Error> {
    let id = this.id;
    if this.read()?.type_(id) != Some(ArenaKind::Element) {
        return Ok(None);
    }
    let lv = ruby_verified_text(ruby, local, "attribute local name")?;
    let nv = crate::bridge::string::namespace_arg(ruby, ns, "namespace")?;
    let key = AttrKey::Ns {
        ns: nv.as_ref().map_or(&b""[..], |n| n.as_bytes()),
        local: lv.as_bytes(),
    };
    let view = this.read()?;
    Ok(key.find_in(&view, id))
}
