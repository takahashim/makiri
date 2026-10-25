//! [`IntTable`]: a Ruby Hash from small non-negative integers to values, kept
//! by one object as its private table - a wrapper cache, say, from a token to
//! the one wrapper made for it.
//!
//! A Ruby Hash rather than a Rust map, so the GC marks its values and moves
//! them itself, and the owner keeps one value (the table) where its mark
//! finds it. Its keys are Fixnums, which hash and compare in C, so no Ruby
//! code runs on a lookup; how an integer becomes one is the layer's
//! (`raw::table_get` / `table_set`), as is the hiding of the Hash.

#![forbid(unsafe_code)]

use crate::rb::{Error, RHash, ReprValue, Ruby, Value};

/// A Hash keyed by small non-negative integers ([module docs](self)).
#[derive(Clone, Copy)]
pub struct IntTable(RHash);

impl IntTable {
    /// An empty table, hidden from `ObjectSpace` from the moment it is made:
    /// no Ruby code can find it to clear, freeze or fill it. Hand it to no
    /// Ruby code - a hidden object has no class - only to an owner's slot.
    pub fn new_hidden(ruby: &Ruby) -> Result<IntTable, Error> {
        crate::raw::hidden_hash_new(ruby).map(IntTable)
    }

    /// The table `v` is, read back from where its owner keeps it: `None`
    /// for a value that is not a Hash.
    #[inline]
    pub fn from_value(v: Value) -> Option<IntTable> {
        RHash::from_value(v).map(IntTable)
    }

    /// The table as a value, for its owner to keep.
    #[inline]
    pub fn as_value(self) -> Value {
        self.0.as_value()
    }

    /// The value at `n`, or `None` - also for an `n` past a Fixnum, which
    /// no entry can have.
    #[inline]
    pub fn get(self, n: usize) -> Result<Option<Value>, Error> {
        crate::raw::table_get(self.0, n)
    }

    /// Set the value at `n`: raises `NoMemoryError`, and an `n` past a
    /// Fixnum is an internal error.
    #[inline]
    pub fn set(self, n: usize, v: Value) -> Result<(), Error> {
        crate::raw::table_set(self.0, n, v)
    }
}
