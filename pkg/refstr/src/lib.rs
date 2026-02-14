use std::alloc::{alloc, dealloc, realloc, Layout};
use std::cell::Cell;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::mem;
use std::ops::Deref;
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicUsize, Ordering};

// --- Public Types -----------------------------------------------------------

/// Single-threaded reference-counted string. Cheap clone (refcount bump).
/// Use when all access is on one thread (agent loop, session manager, etc.).
pub type LocalStr = Str<Local>;

/// Thread-safe reference-counted string. Cheap clone (atomic refcount bump).
/// Use when the string crosses thread boundaries (rare in this codebase).
pub type AtomicStr = Str<Atomic>;

// --- The Core Type ----------------------------------------------------------

/// A reference-counted string with inline storage.
///
/// Layout: `[Header { rc, cap }][...utf8 bytes...]`
///
/// - Clone is O(1) — increments refcount.
/// - Deref to `&str` is O(1) — pointer + length.
/// - Equality checks pointer first (O(1) fast path for clones).
/// - Implements `Borrow<str>` so `HashMap<Str<M>, V>` supports `&str` lookup.
/// - Writable via `fmt::Write` when refcount == 1.
pub struct Str<M: Mode> {
    ptr: NonNull<u8>,
    len: usize,
    _marker: PhantomData<M>,
}

unsafe impl Send for Str<Atomic> {}
unsafe impl Sync for Str<Atomic> {}

// --- The Modes (Local vs Atomic) --------------------------------------------

pub struct Local(Cell<usize>);
pub struct Atomic(AtomicUsize);

pub trait Mode {
    fn new(count: usize) -> Self;
    fn count(&self) -> usize;
    fn inc(&self);
    /// Decrements and returns `true` if the count reached zero.
    fn dec_check(&self) -> bool;
}

impl Mode for Local {
    #[inline(always)]
    fn new(c: usize) -> Self {
        Local(Cell::new(c))
    }
    #[inline(always)]
    fn count(&self) -> usize {
        self.0.get()
    }
    #[inline(always)]
    fn inc(&self) {
        self.0.set(self.0.get() + 1);
    }
    #[inline(always)]
    fn dec_check(&self) -> bool {
        let v = self.0.get() - 1;
        self.0.set(v);
        v == 0
    }
}

impl Mode for Atomic {
    #[inline(always)]
    fn new(c: usize) -> Self {
        Atomic(AtomicUsize::new(c))
    }
    #[inline(always)]
    fn count(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }
    #[inline(always)]
    fn inc(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
    #[inline(always)]
    fn dec_check(&self) -> bool {
        self.0.fetch_sub(1, Ordering::Release) == 1 && {
            std::sync::atomic::fence(Ordering::Acquire);
            true
        }
    }
}

// --- Layout / Header --------------------------------------------------------

#[repr(C)]
struct Header<M> {
    rc: M,
    cap: usize,
}

impl<M: Mode> Str<M> {
    pub fn new() -> Self {
        Self::with_capacity(0)
    }

    pub fn with_capacity(cap: usize) -> Self {
        let cap = cap.max(1);
        let layout = Self::layout(cap);
        unsafe {
            let ptr = alloc(layout);
            if ptr.is_null() {
                std::alloc::handle_alloc_error(layout);
            }

            let header = ptr as *mut Header<M>;
            ptr::write(header, Header { rc: M::new(1), cap });

            Str {
                ptr: NonNull::new_unchecked(ptr.add(mem::size_of::<Header<M>>())),
                len: 0,
                _marker: PhantomData,
            }
        }
    }

    /// O(1) pointer equality — true for clones of the same allocation.
    pub fn ptr_eq(&self, other: &Self) -> bool {
        self.ptr == other.ptr
    }

    /// Current reference count (for debugging).
    pub fn ref_count(&self) -> usize {
        unsafe { self.header().rc.count() }
    }

    /// Length in bytes.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if the string is empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    unsafe fn header(&self) -> &Header<M> {
        unsafe { &*(self.ptr.as_ptr().sub(mem::size_of::<Header<M>>()) as *const Header<M>) }
    }

    fn layout(cap: usize) -> Layout {
        Layout::new::<Header<M>>()
            .extend(Layout::array::<u8>(cap).unwrap())
            .unwrap()
            .0
    }

    fn grow(&mut self, required: usize) {
        unsafe {
            let header = self.header();
            let old_cap = header.cap;
            let new_cap = old_cap.checked_mul(2).unwrap().max(required);

            let old_layout = Self::layout(old_cap);
            let new_layout = Self::layout(new_cap);
            let alloc_ptr = self.ptr.as_ptr().sub(mem::size_of::<Header<M>>());

            let new_ptr = realloc(alloc_ptr, old_layout, new_layout.size());
            if new_ptr.is_null() {
                std::alloc::handle_alloc_error(new_layout);
            }

            (*(new_ptr as *mut Header<M>)).cap = new_cap;
            self.ptr = NonNull::new_unchecked(new_ptr.add(mem::size_of::<Header<M>>()));
        }
    }

    /// Push a `&str` onto this string. Panics if refcount > 1.
    pub fn push_str(&mut self, s: &str) {
        use std::fmt::Write;
        self.write_str(s).unwrap();
    }
}

// --- From conversions -------------------------------------------------------

impl<M: Mode> From<&str> for Str<M> {
    fn from(s: &str) -> Self {
        let mut out = Self::with_capacity(s.len());
        out.push_str(s);
        out
    }
}

impl<M: Mode> From<String> for Str<M> {
    fn from(s: String) -> Self {
        Self::from(s.as_str())
    }
}

impl<M: Mode> From<&String> for Str<M> {
    fn from(s: &String) -> Self {
        Self::from(s.as_str())
    }
}

// --- Standard Integration ---------------------------------------------------

impl<M: Mode> Deref for Str<M> {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        unsafe {
            let slice = std::slice::from_raw_parts(self.ptr.as_ptr(), self.len);
            std::str::from_utf8_unchecked(slice)
        }
    }
}

impl<M: Mode> Clone for Str<M> {
    fn clone(&self) -> Self {
        unsafe {
            self.header().rc.inc();
        }
        Str {
            ptr: self.ptr,
            len: self.len,
            _marker: PhantomData,
        }
    }
}

impl<M: Mode> Drop for Str<M> {
    fn drop(&mut self) {
        unsafe {
            let header = self.header();
            if header.rc.dec_check() {
                dealloc(
                    self.ptr.as_ptr().sub(mem::size_of::<Header<M>>()),
                    Self::layout(header.cap),
                );
            }
        }
    }
}

impl<M: Mode> fmt::Write for Str<M> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        unsafe {
            let header = self.header();
            if header.rc.count() != 1 {
                panic!("Cannot mutate shared Str (refcount = {})", header.rc.count());
            }

            let req = self.len + s.len();
            if req > header.cap {
                self.grow(req);
            }

            ptr::copy_nonoverlapping(s.as_ptr(), self.ptr.as_ptr().add(self.len), s.len());
            self.len += s.len();
        }
        Ok(())
    }
}

impl<M: Mode> fmt::Display for Str<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

impl<M: Mode> fmt::Debug for Str<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<M: Mode> PartialEq for Str<M> {
    fn eq(&self, other: &Self) -> bool {
        if self.ptr_eq(other) {
            return true;
        }
        self.deref() == other.deref()
    }
}

impl<M: Mode> PartialEq<&str> for Str<M> {
    fn eq(&self, other: &&str) -> bool {
        self.deref() == *other
    }
}

impl<M: Mode> PartialEq<String> for Str<M> {
    fn eq(&self, other: &String) -> bool {
        self.deref() == other.as_str()
    }
}

impl<M: Mode> Eq for Str<M> {}

impl<M: Mode> Hash for Str<M> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.deref().hash(state);
    }
}

impl<M: Mode> std::borrow::Borrow<str> for Str<M> {
    fn borrow(&self) -> &str {
        &**self
    }
}

impl<M: Mode> Default for Str<M> {
    fn default() -> Self {
        Self::new()
    }
}

impl<M: Mode> AsRef<str> for Str<M> {
    fn as_ref(&self) -> &str {
        &**self
    }
}

// --- Serde support ----------------------------------------------------------

#[cfg(feature = "serde")]
mod serde_impl {
    use super::*;
    use serde::de::{Deserialize, Deserializer, Visitor};
    use serde::ser::{Serialize, Serializer};

    impl<M: Mode> Serialize for Str<M> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_str(self)
        }
    }

    struct StrVisitor<M: Mode>(PhantomData<M>);

    impl<M: Mode> Visitor<'_> for StrVisitor<M> {
        type Value = Str<M>;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a string")
        }

        fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok(Str::from(v))
        }

        fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Self::Value, E> {
            Ok(Str::from(v))
        }
    }

    impl<'de, M: Mode> Deserialize<'de> for Str<M> {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            deserializer.deserialize_str(StrVisitor(PhantomData))
        }
    }
}

// --- Tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn basic_local() {
        let s: LocalStr = "hello".into();
        assert_eq!(&*s, "hello");
        assert_eq!(s.len(), 5);
        assert_eq!(s.ref_count(), 1);
    }

    #[test]
    fn clone_shares() {
        let a: LocalStr = "world".into();
        let b = a.clone();
        assert!(a.ptr_eq(&b));
        assert_eq!(a.ref_count(), 2);
        assert_eq!(&*a, &*b);
    }

    #[test]
    fn drop_frees() {
        let a: LocalStr = "test".into();
        let b = a.clone();
        assert_eq!(a.ref_count(), 2);
        drop(b);
        assert_eq!(a.ref_count(), 1);
    }

    #[test]
    fn push_str_works() {
        let mut s: LocalStr = LocalStr::with_capacity(16);
        s.push_str("hello");
        s.push_str(" world");
        assert_eq!(&*s, "hello world");
    }

    #[test]
    #[should_panic(expected = "Cannot mutate shared Str")]
    fn push_str_panics_if_shared() {
        let mut a: LocalStr = "hello".into();
        let _b = a.clone();
        a.push_str(" world");
    }

    #[test]
    fn hashmap_lookup_with_str() {
        let mut map: HashMap<LocalStr, u32> = HashMap::new();
        let key: LocalStr = "hello".into();
        map.insert(key, 42);
        assert_eq!(map.get("hello"), Some(&42));
    }

    #[test]
    fn equality() {
        let a: LocalStr = "hello".into();
        let b: LocalStr = "hello".into();
        let c = a.clone();

        // Different allocations, same content
        assert_eq!(a, b);
        assert!(!a.ptr_eq(&b));

        // Same allocation
        assert_eq!(a, c);
        assert!(a.ptr_eq(&c));

        // Against &str
        assert_eq!(a, "hello");
    }

    #[test]
    fn atomic_basic() {
        let a: AtomicStr = "threadsafe".into();
        let b = a.clone();
        assert!(a.ptr_eq(&b));
        assert_eq!(&*a, "threadsafe");
    }

    #[test]
    fn grow_realloc() {
        let mut s: LocalStr = LocalStr::with_capacity(2);
        s.push_str("this is a much longer string that forces reallocation");
        assert_eq!(&*s, "this is a much longer string that forces reallocation");
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_roundtrip() {
        let original: LocalStr = "serde test".into();
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(json, "\"serde test\"");
        let deserialized: LocalStr = serde_json::from_str(&json).unwrap();
        assert_eq!(original, deserialized);
    }
}
