//! CoreFoundation and IOKit objects for the macOS readings: owners that release
//! on every path, and a borrowed view with checked readers, so the readings
//! above it hold no raw reference and call nothing unsafe.

use core::{
    ffi::{CStr, c_char, c_void},
    marker::PhantomData,
};

use super::{ffi, kern_error};
use crate::Result;

/// A CoreFoundation object this code owns (one reference), released on drop.
pub(super) struct Cf(ffi::CfRef);

impl Cf {
    /// Takes the one reference a Create or Copy function returned; null is `None`.
    pub(super) fn new(object: ffi::CfRef) -> Option<Cf> {
        (!object.is_null()).then_some(Cf(object))
    }

    /// An immutable UTF-8 `CFString`.
    pub(super) fn string(what: &'static str, text: &str) -> Result<Cf> {
        let len = ffi::CfIndex::try_from(text.len()).unwrap_or(0);
        // SAFETY: `text` is `len` readable bytes; the null allocator is the
        // default one.
        let object = unsafe {
            ffi::CFStringCreateWithBytes(
                core::ptr::null(),
                text.as_ptr(),
                len,
                ffi::CF_STRING_ENCODING_UTF8,
                0,
            )
        };
        Cf::new(object).ok_or_else(|| kern_error(what, "CFStringCreateWithBytes", 0))
    }

    /// The reference, for a call that only reads it.
    pub(super) fn raw(&self) -> ffi::CfRef {
        self.0
    }

    /// The object as a borrowed value.
    pub(super) fn get(&self) -> Ref<'_> {
        Ref {
            object: self.0,
            owner: PhantomData,
        }
    }

    /// A reference the Get rule lends out of this object: CoreFoundation keeps
    /// it alive as long as `self`, and null is `None`.
    pub(super) fn lent(&self, object: ffi::CfRef) -> Option<Ref<'_>> {
        (!object.is_null()).then_some(Ref {
            object,
            owner: PhantomData,
        })
    }
}

impl Drop for Cf {
    fn drop(&mut self) {
        // SAFETY: this value owns one reference, released once.
        unsafe { ffi::CFRelease(self.0) };
    }
}

/// A live CoreFoundation object, valid while its owner is. The readers check
/// the object's type first: a registry property can be anything.
#[derive(Clone, Copy)]
pub(super) struct Ref<'a> {
    object: ffi::CfRef,
    owner: PhantomData<&'a Cf>,
}

impl<'a> Ref<'a> {
    fn is(self, type_id: ffi::CfTypeId) -> bool {
        // SAFETY: `self.object` is a live CoreFoundation object.
        let id = unsafe { ffi::CFGetTypeID(self.object) };
        id == type_id
    }

    /// The reference, for a call that only reads it.
    pub(super) fn raw(self) -> ffi::CfRef {
        self.object
    }

    pub(super) fn is_dictionary(self) -> bool {
        self.is(ffi::CFDictionaryGetTypeID())
    }

    /// The value of `key`, when this is a dictionary holding it.
    pub(super) fn value(self, key: &Cf) -> Option<Ref<'a>> {
        if !self.is_dictionary() {
            return None;
        }
        // SAFETY: `self.object` is a dictionary and `key` a live object. The
        // value is borrowed from the dictionary (Get rule).
        let object = unsafe { ffi::CFDictionaryGetValue(self.object, key.raw()) };
        (!object.is_null()).then_some(Ref {
            object,
            owner: PhantomData,
        })
    }

    /// The elements of an array, the first `max` of them.
    pub(super) fn items(self, max: usize) -> impl Iterator<Item = Ref<'a>> {
        let count = if self.is(ffi::CFArrayGetTypeID()) {
            // SAFETY: `self.object` is an array.
            let count = unsafe { ffi::CFArrayGetCount(self.object) };
            count.clamp(0, ffi::CfIndex::try_from(max).unwrap_or(ffi::CfIndex::MAX))
        } else {
            0
        };
        (0..count).filter_map(move |index| {
            // SAFETY: `self.object` is an array and `index` is within its
            // count. The element is borrowed from the array (Get rule).
            let object = unsafe { ffi::CFArrayGetValueAtIndex(self.object, index) };
            (!object.is_null()).then_some(Ref {
                object,
                owner: PhantomData,
            })
        })
    }

    /// The value of a `CFNumber` that fits 64 bits exactly.
    pub(super) fn int(self) -> Option<i64> {
        if !self.is(ffi::CFNumberGetTypeID()) {
            return None;
        }
        let mut n: i64 = 0;
        // SAFETY: `self.object` is a `CFNumber` and `n` a writable 64-bit
        // integer, the size of `kCFNumberSInt64Type`.
        let exact = unsafe {
            ffi::CFNumberGetValue(
                self.object,
                ffi::CF_NUMBER_SINT64_TYPE,
                (&raw mut n).cast::<c_void>(),
            )
        };
        (exact != 0).then_some(n)
    }

    /// The value of a `CFBoolean`.
    pub(super) fn flag(self) -> Option<bool> {
        if !self.is(ffi::CFBooleanGetTypeID()) {
            return None;
        }
        // SAFETY: `self.object` is a `CFBoolean`.
        let value = unsafe { ffi::CFBooleanGetValue(self.object) };
        Some(value != 0)
    }

    /// A `CFString` as UTF-8 in `buf`, up to its NUL; `None` when it is another
    /// type or does not fit. The string goes nowhere but `buf`.
    pub(super) fn text(self, buf: &mut [u8]) -> Option<&[u8]> {
        if !self.is(ffi::CFStringGetTypeID()) {
            return None;
        }
        let size = ffi::CfIndex::try_from(buf.len()).ok()?;
        // SAFETY: `self.object` is a `CFString` and `buf` is `size` writable
        // bytes; the call writes at most that many, the NUL included.
        let ok = unsafe {
            ffi::CFStringGetCString(
                self.object,
                buf.as_mut_ptr().cast::<c_char>(),
                size,
                ffi::CF_STRING_ENCODING_UTF8,
            )
        };
        if ok == 0 {
            return None;
        }
        let end = buf.iter().position(|&b| b == 0)?;
        Some(&buf[..end])
    }

    /// The bytes of a `CFData`.
    pub(super) fn data(self) -> Option<&'a [u8]> {
        if !self.is(ffi::CFDataGetTypeID()) {
            return None;
        }
        // SAFETY: `self.object` is a `CFData`.
        let len = usize::try_from(unsafe { ffi::CFDataGetLength(self.object) }).ok()?;
        // SAFETY: as above.
        let bytes = unsafe { ffi::CFDataGetBytePtr(self.object) };
        if bytes.is_null() {
            return None;
        }
        // SAFETY: an immutable `CFData` holds `len` bytes at `bytes` for as long
        // as it lives, which `'a` does not outlast.
        Some(unsafe { core::slice::from_raw_parts(bytes, len) })
    }
}

/// An IOKit object this code owns, released on drop.
pub(super) struct Io(pub(super) ffi::IoObject);

impl Io {
    /// Takes an object an IOKit call returned; 0 is `None`.
    pub(super) fn new(object: ffi::IoObject) -> Option<Io> {
        (object != 0).then_some(Io(object))
    }

    /// The service a matching dictionary finds. The dictionary's one reference
    /// is consumed on every path, so `matching` must not be used again.
    pub(super) fn matching(matching: ffi::CfRef) -> Option<Io> {
        if matching.is_null() {
            return None;
        }
        // SAFETY: `matching` is a dictionary with one reference, which the call
        // consumes; the object returned, if any, is owned by the result.
        let service =
            unsafe { ffi::IOServiceGetMatchingService(ffi::IO_MAIN_PORT_DEFAULT, matching) };
        Io::new(service)
    }

    /// The first service of an IOKit class.
    pub(super) fn service(class: &CStr) -> Option<Io> {
        // SAFETY: the class name is NUL-terminated. The dictionary returned
        // carries one reference, which `matching` consumes.
        let matching = unsafe { ffi::IOServiceMatching(class.as_ptr()) };
        Io::matching(matching)
    }

    /// The registry entry at `path` (`IODeviceTree:/chosen`).
    pub(super) fn at_path(path: &CStr) -> Option<Io> {
        // SAFETY: the path is NUL-terminated; the object returned, if any, is
        // owned by the result.
        let entry =
            unsafe { ffi::IORegistryEntryFromPath(ffi::IO_MAIN_PORT_DEFAULT, path.as_ptr()) };
        Io::new(entry)
    }

    /// The BSD device's media (`disk3s1`).
    pub(super) fn bsd_device(name: &CStr) -> Option<Io> {
        // SAFETY: the name is NUL-terminated. The dictionary returned carries
        // one reference, which `matching` consumes.
        let matching =
            unsafe { ffi::IOBSDNameMatching(ffi::IO_MAIN_PORT_DEFAULT, 0, name.as_ptr()) };
        Io::matching(matching)
    }

    /// A copy of a property of this registry entry.
    pub(super) fn property(&self, key: &Cf) -> Option<Cf> {
        // SAFETY: `self.0` is a live registry entry and `key` a live `CFString`;
        // the null allocator is the default one. The result, if any, carries
        // one reference, owned by the returned value.
        Cf::new(unsafe {
            ffi::IORegistryEntryCreateCFProperty(self.0, key.raw(), core::ptr::null(), 0)
        })
    }

    /// This entry's first parent in the service plane.
    pub(super) fn parent(&self) -> Option<Io> {
        let mut parent: ffi::IoObject = 0;
        // SAFETY: `self.0` is a live registry entry, the plane name is
        // NUL-terminated, and `parent` is writable; a parent returned is owned
        // by the result.
        let kr = unsafe {
            ffi::IORegistryEntryGetParentEntry(self.0, c"IOService".as_ptr(), &raw mut parent)
        };
        let parent = Io::new(parent)?;
        (kr == ffi::KERN_SUCCESS).then_some(parent)
    }

    /// This entry's first child in the service plane.
    pub(super) fn child(&self) -> Option<Io> {
        let mut child: ffi::IoObject = 0;
        // SAFETY: as in `parent`.
        let kr = unsafe {
            ffi::IORegistryEntryGetChildEntry(self.0, c"IOService".as_ptr(), &raw mut child)
        };
        let child = Io::new(child)?;
        (kr == ffi::KERN_SUCCESS).then_some(child)
    }
}

impl Drop for Io {
    fn drop(&mut self) {
        // SAFETY: this value owns the object, released once.
        unsafe { ffi::IOObjectRelease(self.0) };
    }
}
