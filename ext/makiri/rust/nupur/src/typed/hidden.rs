//! [`TypedType::wrap_hidden`]: an object kept hidden from Ruby code while
//! its data is completed, and shown only once it is whole.

#![allow(unsafe_code)]

use super::{Birth, Payload, Root, Typed, TypedType};
use crate::rb::{Error, Value};

impl<T: Payload + 'static, const N: usize> TypedType<T, N> {
    /// [`wrap`](Self::wrap) an object whose data is not complete when it is
    /// made - one whose content arrives after it - and keep it HIDDEN
    /// (`rb_obj_hide`: no class, so `ObjectSpace` does not list it and no
    /// Ruby code can find it) while `fill` completes the data; then show it
    /// with its class and return it.
    ///
    /// The object is never handed out before that: a hidden object has no
    /// class, and Ruby code given one - a return value, an Array element -
    /// would crash on it. `fill` sees only the data, and may run anything
    /// (a GVL-released parse included), since nothing can reach the object.
    /// An `Err` from `fill`, or a failure to show the object, leaves it
    /// hidden for the GC to free, and is returned. The object comes back as
    /// the [`Typed`] handle that held it through `fill`, so a caller that
    /// goes on using it before handing it to Ruby still holds it.
    ///
    /// It is not hidden from birth. `ivars`, its hidden ivars (names without
    /// `@`), are written first, while it still has its class - generic ivar
    /// storage may read an object's class, so nothing is set on one that has
    /// none - and then it is hidden. What keeps Ruby code from finding it in
    /// between is that none runs there: the ivar writes may allocate, and so
    /// collect, but a collection runs no Ruby code. A step added between the
    /// wrap and the hide that can run Ruby code breaks that. A failed
    /// allocation of a non-embedded type's data leaves an object of the class
    /// with no data, visible, which every reader refuses (`typed_data`).
    ///
    /// Nothing but this call refers to the object until it is shown, so its
    /// data is lent to `fill` through the handle the wrap made, held as a
    /// `Hidden` - which lends the data and shows the object, and hands out
    /// nothing else.
    pub fn wrap_hidden<R: Root>(
        &'static self,
        ruby: &crate::rb::Ruby,
        birth: Birth<T, R, N>,
        ivars: &[(crate::rb::HiddenId, Value)],
        fill: impl FnOnce(&T) -> Result<(), Error>,
    ) -> Result<Typed<T, N>, Error> {
        let klass = birth.klass;
        let made = self.wrap(ruby, birth)?;
        let written = ivars
            .iter()
            .try_for_each(|&(id, v)| crate::raw::ivar_set(made.value(), id, v));
        // SAFETY: the object just made, which nothing but `made` refers to.
        let hidden = unsafe { Hidden::hide(made, klass) };
        written?;
        fill(hidden.data())?;
        hidden.show()
    }
}

/// An object [`TypedType::wrap_hidden`] has hidden: its data can be lent
/// and it can be shown, but its value cannot be had - a hidden object has no
/// class, and Ruby code given one would crash on it. Dropped unshown, it
/// stays hidden for the GC to free.
struct Hidden<T: Payload + 'static, const N: usize> {
    made: Typed<T, N>,
    klass: crate::rb::DataClass,
}

impl<T: Payload + 'static, const N: usize> Hidden<T, N> {
    /// Hide `made`, to be shown as an instance of `klass`.
    ///
    /// # Safety
    /// `made` is an object just made that nothing else refers to - no Ruby
    /// code holds it, and none will be handed it while it is hidden.
    unsafe fn hide(made: Typed<T, N>, klass: crate::rb::DataClass) -> Hidden<T, N> {
        // SAFETY: the caller's contract; hiding writes only the object's
        // class field, which `show` restores.
        unsafe { crate::rb::object::obj_hide(made.value()) };
        Hidden { made, klass }
    }

    /// The payload, lent while the object is hidden.
    fn data(&self) -> &T {
        self.made.data()
    }

    /// Show the object with its class again, and hand it out. A failure (out
    /// of memory) leaves it hidden, for the GC.
    fn show(self) -> Result<Typed<T, N>, Error> {
        crate::raw::obj_reveal(
            self.made.value(),
            crate::rb::ReprValue::as_value(self.klass),
        )?;
        Ok(self.made)
    }
}
