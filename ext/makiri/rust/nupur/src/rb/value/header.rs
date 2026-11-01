//! What a String's object header says of it, read straight from the header:
//! its cached coderange, its encoding index and whether its class is String
//! itself. The one place the layer reads `RBasic`'s fields for a String, so
//! what rests on Ruby's object layout is read here and nowhere else; what a
//! reader makes of the answers (`string::read`, `encoding`) is theirs.

#![allow(unsafe_code)]

use core::ffi::c_int;

use super::{AsSys, RString};

/// A String's coderange, as Ruby caches it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Coderange {
    Unknown,
    SevenBit,
    Valid,
    Broken,
}

impl RString {
    /// The header's flags word.
    #[inline]
    fn flags(self) -> usize {
        // SAFETY: a String is a heap object, whose header starts with
        // `RBasic`; reading its flags runs no Ruby.
        unsafe { nupur_sys::non_raising::rbasic_flags(self.as_sys()) }
    }

    /// The String's cached coderange - a read of its header flags, which
    /// never scans and never allocates.
    #[inline]
    pub(crate) fn enc_coderange(self) -> Coderange {
        const MASK: usize = nupur_sys::ruby_coderange_type::RUBY_ENC_CODERANGE_MASK as usize;
        const SEVEN: usize = nupur_sys::ruby_coderange_type::RUBY_ENC_CODERANGE_7BIT as usize;
        const VALID: usize = nupur_sys::ruby_coderange_type::RUBY_ENC_CODERANGE_VALID as usize;
        const BROKEN: usize = nupur_sys::ruby_coderange_type::RUBY_ENC_CODERANGE_BROKEN as usize;
        match self.flags() & MASK {
            SEVEN => Coderange::SevenBit,
            VALID => Coderange::Valid,
            BROKEN => Coderange::Broken,
            _ => Coderange::Unknown,
        }
    }

    /// The String's encoding index, read from its flags where Ruby keeps it
    /// inline (every encoding below `RUBY_ENCODING_INLINE_MAX`, the builtin
    /// three among them), and looked up only past that.
    #[inline]
    pub(crate) fn encoding_index(self) -> c_int {
        use nupur_sys::ruby_encoding_consts::{
            RUBY_ENCODING_INLINE_MAX, RUBY_ENCODING_MASK, RUBY_ENCODING_SHIFT,
        };
        /* Seven bits, which a `c_int` holds. */
        let bits = (self.flags() & RUBY_ENCODING_MASK as usize) >> RUBY_ENCODING_SHIFT as u32;
        let inline = c_int::try_from(bits).unwrap_or(c_int::MAX);
        if inline == RUBY_ENCODING_INLINE_MAX as c_int {
            // SAFETY: a live String, by type.
            unsafe { nupur_sys::non_raising::enc_get_index_of_string(self.as_sys()) }
        } else {
            inline
        }
    }

    /// Whether the String's class is String itself, not a subclass: the
    /// header's `klass`, compared with `rb_cString`.
    #[inline]
    pub(crate) fn has_string_class(self) -> bool {
        let string = nupur_sys::non_raising::rb_cString(self.as_sys().ruby());
        // SAFETY: as `flags`.
        unsafe { nupur_sys::non_raising::rbasic_klass(self.as_sys()) }.is(string)
    }
}
