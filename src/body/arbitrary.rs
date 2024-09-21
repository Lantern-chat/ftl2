use core::{mem, ptr};

const MAX_ARBITRARY_SIZE: usize = 32;

/// A small arbitrary data type that can be stored in a [`Body`].
///
/// Because this is read-only, we can skip `Box<dyn Trait>` and store the data directly
/// as unaligned bytes. This is safe because the data is never accessed as a reference.
pub(crate) struct SmallArbitraryData {
    data: [u8; MAX_ARBITRARY_SIZE],
    drop_fn: fn(*mut u8),
}

impl Drop for SmallArbitraryData {
    fn drop(&mut self) {
        (self.drop_fn)(self.data.as_mut_ptr());
    }
}

#[inline(never)]
fn drop_fn<T>(ptr: *mut u8) {
    unsafe {
        let mut value = mem::MaybeUninit::<T>::uninit();
        ptr::copy_nonoverlapping(ptr, value.as_mut_ptr() as *mut u8, size_of::<T>());
        value.assume_init_drop();
    }
}

impl SmallArbitraryData {
    pub fn same_ty<T>(&self) -> bool {
        (drop_fn::<T> as fn(*mut u8)) == self.drop_fn
    }

    pub unsafe fn new<T: 'static>(value: T) -> Self {
        const {
            if size_of::<T>() > MAX_ARBITRARY_SIZE {
                panic!("arbitrary body size is too large");
            }
        }

        let mut data = [0; MAX_ARBITRARY_SIZE];
        ptr::copy_nonoverlapping(&value as *const _ as *const u8, data.as_mut_ptr(), size_of::<T>());
        mem::forget(value); // moved into `data`

        Self {
            data,
            drop_fn: drop_fn::<T>,
        }
    }

    /// # Safety:
    /// * This instance must be forgotten before the resulting value is assumed initialized.
    pub unsafe fn read<T: 'static>(&self) -> mem::MaybeUninit<T> {
        let mut value = mem::MaybeUninit::<T>::uninit();
        ptr::copy_nonoverlapping(self.data.as_ptr(), value.as_mut_ptr() as *mut u8, size_of::<T>());
        value
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn arbitrary_data() {
        unsafe {
            let data = SmallArbitraryData::new(42u32);
            assert!(data.same_ty::<u32>());
            assert!(!data.same_ty::<u64>());

            let value = data.read::<u32>();
            assert_eq!(value.assume_init(), 42);
        }
    }
}
