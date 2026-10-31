//! [`CName`]: a short name - a class or method to define, an encoding to
//! look up - as the NUL-terminated C string Ruby's C API takes, held on the
//! stack. Ruby-free.

#![forbid(unsafe_code)]

use core::ffi::CStr;

/// The longest name a [`CName`] holds, its NUL included. Every name the
/// layer hands Ruby this way is a short identifier.
const NAME_MAX: usize = 64;

/// A name as a C string, on the stack.
pub(crate) struct CName([u8; NAME_MAX]);

impl core::ops::Deref for CName {
    type Target = CStr;
    fn deref(&self) -> &CStr {
        /* `new` wrote a NUL and checked there is no other. */
        CStr::from_bytes_until_nul(&self.0).unwrap_or(c"")
    }
}

impl CName {
    /// `name` NUL-terminated on the stack, or `None` for one with a NUL of
    /// its own or too long to leave room for the terminator.
    pub(crate) fn new(name: &[u8]) -> Option<CName> {
        let mut buf = [0u8; NAME_MAX];
        match buf.get_mut(..name.len()) {
            Some(dst) if name.len() < NAME_MAX && !name.contains(&0) => {
                dst.copy_from_slice(name);
                Some(CName(buf))
            }
            _ => None,
        }
    }
}
