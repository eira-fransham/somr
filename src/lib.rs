#![feature(shared, alloc, allocator_api)]

extern crate alloc;

use std::ops::Deref;
use std::ptr::{self, Shared};
use std::cell::Cell;
use std::usize;
use std::mem;

use alloc::heap::Heap;
use alloc::allocator::{Layout, Alloc};

// TODO: Multithreaded form of this

const COUNT_MASK: usize = usize::MAX >> 2;

// The value has been deallocated (i.e. only the `Cell` is allocated)
const VALUE_DEALLOCATED_FLAG: usize = usize::MAX ^ usize::MAX >> 1;

// The value has been dropped and therefore reading it is invalid
const VALUE_DROPPED_FLAG: usize = usize::MAX ^ usize::MAX >> 2;

#[repr(C)]
struct SomrInner<T: ?Sized> {
    weak_count: Cell<usize>,
    value: T,
}

pub struct Somr<T: ?Sized> {
    ptr: Shared<SomrInner<T>>,
}

pub struct Weak<T: ?Sized> {
    ptr: Shared<SomrInner<T>>,
}

#[inline(always)]
unsafe fn get_count<T: ?Sized>(inner: *mut SomrInner<T>) -> usize {
    let out = (&*inner).weak_count.get() & COUNT_MASK;
    out
}

struct StillReferenced;

#[inline(always)]
unsafe fn try_dealloc<T: ?Sized>(inner: *mut SomrInner<T>) -> Result<(), StillReferenced> {
    // We make sure to only dealloc after the inner value has been dropped (this means that the
    // last `weak` being dropped before the owner is dropped won't cause a double-free)
    if value_dropped(inner) && get_count(inner) == 0 {
        let layout = if (&*inner).weak_count.get() == VALUE_DEALLOCATED_FLAG {
            Layout::new::<Cell<usize>>()
        } else {
            Layout::for_value(&*inner)
        };
        Heap.dealloc(inner as *mut u8, layout);
        Ok(())
    } else {
        Err(StillReferenced)
    }
}

#[inline(always)]
unsafe fn value_dropped<T: ?Sized>(inner: *mut SomrInner<T>) -> bool {
    (&*inner).weak_count.get() & VALUE_DROPPED_FLAG != 0
}

impl<T: ?Sized> Somr<T> {
    pub fn new(val: T) -> Self
    where
        T: Sized,
    {
        let dst: Shared<SomrInner<T>> = {
            let alloc_result = Heap.alloc_one::<SomrInner<T>>();
            match alloc_result {
                Ok(ptr) => ptr.into(),
                Err(alloc_error) => Heap.oom(alloc_error),
            }
        };

        unsafe {
            ptr::write(
                dst.as_ptr(),
                SomrInner {
                    weak_count: Cell::new(0),
                    value: val,
                },
            );
        }

        Somr { ptr: dst }
    }

    pub fn unwrap(this: Self) -> T
    where
        T: Sized,
    {
        unsafe {
            let inner = &mut *this.ptr.as_ptr();
            let val = ptr::read(&inner.value);

            inner.weak_count.set(
                inner.weak_count.get() | VALUE_DROPPED_FLAG,
            );

            mem::forget(this);

            if try_dealloc(inner).is_err() {
                if Heap.shrink_in_place(
                    inner as *mut SomrInner<T> as *mut u8,
                    Layout::for_value(inner),
                    Layout::new::<Cell<usize>>(),
                ).is_ok()
                {
                    inner.weak_count.set(
                        inner.weak_count.get() |
                            VALUE_DEALLOCATED_FLAG,
                    );
                }
            }

            val
        }
    }

    pub fn to_weak(this: &Self) -> Weak<T> {
        unsafe {
            let cur_count = this.ptr.as_ref().weak_count.get();
            let flags = cur_count & !COUNT_MASK;
            let count_with_flags = cur_count | !COUNT_MASK;
            let new_count = count_with_flags.checked_add(1).expect(
                "Weak pointer overflowed",
            );

            this.ptr.as_ref().weak_count.set(
                flags | (new_count & COUNT_MASK),
            )
        };

        Weak { ptr: this.ptr }
    }
}

impl<T: ?Sized> Weak<T> {
    // TODO: Does this need to be optimised?
    pub fn is_dropped(this: &Self) -> bool {
        Self::try_get(this, |_| ()).is_none()
    }

    // `Fn` so that we can't mutate or drop the owning reference by moving it into this function.
    pub fn try_get<Out, F: Fn(&T) -> Out>(this: &Self, func: F) -> Option<Out> {
        unsafe {
            if !value_dropped(this.ptr.as_ptr()) {
                let inner = &*this.ptr.as_ptr();
                Some(func(&inner.value))
            } else {
                None
            }
        }
    }
}

impl<T: ?Sized> Deref for Somr<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &unsafe { self.ptr.as_ref() }.value
    }
}

impl<T: ?Sized> Drop for Weak<T> {
    fn drop(&mut self) {
        unsafe {
            let inner = &*self.ptr.as_ptr();
            inner.weak_count.set(inner.weak_count.get() - 1);
            let _ = try_dealloc(self.ptr.as_ptr());
        }
    }
}

impl<T: ?Sized> Drop for Somr<T> {
    fn drop(&mut self) {
        unsafe {
            let inner = &mut *self.ptr.as_ptr();

            inner.weak_count.set(
                inner.weak_count.get() | VALUE_DROPPED_FLAG,
            );

            ptr::drop_in_place(&mut inner.value);

            if try_dealloc(inner).is_err() {
                if Heap.shrink_in_place(
                    inner as *mut SomrInner<T> as *mut u8,
                    Layout::for_value(inner),
                    Layout::new::<Cell<usize>>(),
                ).is_ok()
                {
                    inner.weak_count.set(
                        inner.weak_count.get() |
                            VALUE_DEALLOCATED_FLAG,
                    )
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Weak, Somr};

    #[test]
    fn can_unwrap() {
        let somr = Somr::new("Hello!".to_owned());

        assert_eq!(Somr::unwrap(somr), "Hello!");
    }

    #[test]
    fn can_make_weak() {
        let somr = Somr::new("Hello!".to_owned());

        let weak = Somr::to_weak(&somr);

        assert_eq!(
            Weak::try_get(&weak, |a| a.to_owned()),
            Some(format!("Hello!"))
        );
    }

    #[test]
    fn owner_dropped_before_weak() {
        let weak = {
            let owner = Somr::new("Hello!".to_owned());
            Somr::to_weak(&owner)
        };

        assert!(Weak::is_dropped(&weak));
    }

    #[test]
    fn destructor_run_when_owner_dropped() {
        use std::cell::Cell;

        #[derive(Debug)]
        struct NoisyDrop<'a>(&'a Cell<bool>);
        impl<'a> Drop for NoisyDrop<'a> {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }

        let been_dropped = Cell::new(false);
        let noisy_drop = NoisyDrop(&been_dropped);
        let weak = {
            let owner = Somr::new(noisy_drop);
            Somr::to_weak(&owner)
        };

        assert!(been_dropped.get());
        assert!(Weak::is_dropped(&weak));
    }
}
