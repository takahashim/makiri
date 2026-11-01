//! What a String's object header says of it, read straight from the header:
//! its cached coderange, its encoding index and whether its class is String
//! itself. The one place the layer reads `RBasic`'s fields for a String, so
//! what rests on Ruby's object layout is read here and nowhere else; what a
//! reader makes of the answers (`string::read`, `encoding`) is theirs.

#![allow(unsafe_code)]

use core::ffi::c_int;

use nupur_sys::VALUE;

use super::{AsRawValue, RString};

/// A String's coderange, as Ruby caches it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Coderange {
    Unknown,
    SevenBit,
    Valid,
    Broken,
}

impl RString {
    /// The header's flags word.
    #[inline]
    fn flags(self) -> VALUE {
        // SAFETY: a String is a heap object, whose header starts with
        // `RBasic`; reading its flags runs no Ruby.
        unsafe { (*(self.as_raw() as *const nupur_sys::RBasic)).flags }
    }

    /// The String's cached coderange - a read of its header flags, which
    /// never scans and never allocates.
    #[inline]
    pub fn enc_coderange(self) -> Coderange {
        const MASK: VALUE = nupur_sys::ruby_coderange_type::RUBY_ENC_CODERANGE_MASK as VALUE;
        const SEVEN: VALUE = nupur_sys::ruby_coderange_type::RUBY_ENC_CODERANGE_7BIT as VALUE;
        const VALID: VALUE = nupur_sys::ruby_coderange_type::RUBY_ENC_CODERANGE_VALID as VALUE;
        const BROKEN: VALUE = nupur_sys::ruby_coderange_type::RUBY_ENC_CODERANGE_BROKEN as VALUE;
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
    #[cfg_attr(
        dylint_lib = "makiri_lints",
        allow(
            raising_call_outside_protect,
            reason = "rb_enc_get_index raises for a File or a Symbol, as rb_enc_get; `s` is a String, and a String's index is an ivar read when it is not inline"
        )
    )]
    pub(crate) fn encoding_index(self) -> c_int {
        use nupur_sys::ruby_encoding_consts::{
            RUBY_ENCODING_INLINE_MAX, RUBY_ENCODING_MASK, RUBY_ENCODING_SHIFT,
        };
        /* Seven bits, which a `c_int` holds. */
        let bits = (crate::int::word(self.flags()) & RUBY_ENCODING_MASK as usize)
            >> RUBY_ENCODING_SHIFT as u32;
        let inline = c_int::try_from(bits).unwrap_or(c_int::MAX);
        if inline == RUBY_ENCODING_INLINE_MAX as c_int {
            // SAFETY: a live String; the lookup reads an instance variable
            // and raises nothing.
            unsafe { nupur_sys::rb_enc_get_index(self.as_raw()) }
        } else {
            inline
        }
    }

    /// Whether the String's class is String itself, not a subclass: the
    /// header's `klass`, compared with `rb_cString`.
    #[inline]
    pub(crate) fn has_string_class(self) -> bool {
        // SAFETY: as `flags`; `rb_cString` is a C global Ruby sets once at
        // boot.
        unsafe { (*(self.as_raw() as *const nupur_sys::RBasic)).klass == nupur_sys::rb_cString }
    }
}
