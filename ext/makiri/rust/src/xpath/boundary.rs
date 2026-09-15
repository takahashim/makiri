//! The raw boundary the glue calls: releasing the results it holds.
//!
//! The evaluator uses the typed APIs in the sibling modules. This stays raw
//! because the glue holds results as C-layout structs.

use crate::falloc::raw::free_and_null;
use crate::xpath_abi::XPathValue;
use core::ffi::c_void;

const MKR_XPATH_TYPE_NODESET: u32 = 0;
const MKR_XPATH_TYPE_STRING: u32 = 1;

/// Release memory owned by an XPath result.
///
/// # Safety
/// `v` is null or a live value whose `type_` identifies its active arm.
pub unsafe extern "C" fn xpath_value_clear(v: *mut XPathValue) {
    if v.is_null() {
        return;
    }
    let v = &mut *v;
    match v.type_ {
        MKR_XPATH_TYPE_NODESET => {
            free_and_null(v.u.nodeset.nodes as *mut c_void);
            v.u.nodeset.nodes = core::ptr::null_mut();
            v.u.nodeset.count = 0;
        }
        MKR_XPATH_TYPE_STRING => v.u.string.clear(),
        _ => {}
    }
}
