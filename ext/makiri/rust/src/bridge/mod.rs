//! The Ruby boundary's text layer, ported from ext/makiri/bridge/.
//!
//! This is the ONLY layer allowed raw Ruby String access and verified-string
//! minting; everything else receives an already-checked view. See
//! docs/string_types.md for the type lattice these functions move values
//! through, and CLAUDE.md's "Text-input contract" for the rules they enforce.
//!
//! Like `glue::node` and unlike `glue::serialize`, nothing here defines a Ruby
//! method - these are C-ABI functions the rest of the extension calls, several
//! of which raise as part of their contract. So the port keeps the C ABI
//! exactly. magnus appears only where it owns a read that has no plain C
//! function behind it (the cached coderange, the byte slice); rb-sys does the
//! rest.

#[cfg(feature = "bridge-string")]
pub mod string;

#[cfg(feature = "bridge-xml-decode")]
pub mod xml_decode;
