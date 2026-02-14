use refstr::{Atomic, Str};
use std::collections::HashMap;

#[test]
fn basic_local() {
    let s: Str = "hello".into();
    assert_eq!(&*s, "hello");
    assert_eq!(s.len(), 5);
    assert_eq!(s.ref_count(), 1);
}

#[test]
fn clone_shares() {
    let a: Str = "world".into();
    let b = a.clone();
    assert!(a.ptr_eq(&b));
    assert_eq!(a.ref_count(), 2);
    assert_eq!(&*a, &*b);
}

#[test]
fn drop_frees() {
    let a: Str = "test".into();
    let b = a.clone();
    assert_eq!(a.ref_count(), 2);
    drop(b);
    assert_eq!(a.ref_count(), 1);
}

#[test]
fn push_str_works() {
    let mut s: Str = Str::with_capacity(16);
    s.make_mut().push_str("hello");
    s.make_mut().push_str(" world");
    assert_eq!(&*s, "hello world");
}

#[test]
fn make_mut_cow_prevents_shared_write() {
    // With the old API this would panic. Now it COWs instead.
    let mut a: Str = "hello".into();
    let b = a.clone();
    a.make_mut().push_str(" world");
    assert_eq!(&*a, "hello world");
    assert_eq!(&*b, "hello");
}

#[test]
fn hashmap_lookup_with_str() {
    let mut map: HashMap<Str, u32> = HashMap::new();
    let key: Str = "hello".into();
    map.insert(key, 42);
    assert_eq!(map.get("hello"), Some(&42));
}

#[test]
fn equality() {
    let a: Str = "hello".into();
    let b: Str = "hello".into();
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
    let a: Str<Atomic> = "threadsafe".into();
    let b = a.clone();
    assert!(a.ptr_eq(&b));
    assert_eq!(&*a, "threadsafe");
}

#[test]
fn grow_realloc() {
    let mut s: Str = Str::with_capacity(2);
    s.make_mut().push_str("this is a much longer string that forces reallocation");
    assert_eq!(&*s, "this is a much longer string that forces reallocation");
}

#[test]
fn make_mut_unique() {
    use std::fmt::Write;
    let mut s: Str = "hello".into();
    assert_eq!(s.ref_count(), 1);
    write!(s.make_mut(), " world").unwrap();
    assert_eq!(&*s, "hello world");
}

#[test]
fn make_mut_cow() {
    use std::fmt::Write;
    let mut s: Str = "hello".into();
    let s2 = s.clone();
    assert_eq!(s.ref_count(), 2);

    // COW: detaches, original clone untouched
    write!(s.make_mut(), " world").unwrap();
    assert_eq!(&*s, "hello world");
    assert_eq!(&*s2, "hello");
    assert_eq!(s.ref_count(), 1);
    assert_eq!(s2.ref_count(), 1);
    assert!(!s.ptr_eq(&s2));
}

#[test]
fn make_mut_clear_reuse() {
    use std::fmt::Write;
    let mut s: Str = "old content".into();
    {
        let mut m = s.make_mut();
        m.clear();
        write!(m, "new {}", 42).unwrap();
    }
    assert_eq!(&*s, "new 42");
}

#[test]
fn str_mut_deref() {
    let mut s: Str = "hello".into();
    let m = s.make_mut();
    assert_eq!(&*m, "hello");
    assert_eq!(m.len(), 5);
    assert!(!m.is_empty());
}

#[test]
fn str_mut_prevents_clone() {
    // This is a compile-time guarantee — StrMut borrows &mut Str,
    // so s.clone() is impossible while StrMut exists.
    // Just verify make_mut returns and we can use it.
    let mut s: Str = "test".into();
    let m = s.make_mut();
    assert_eq!(&*m, "test");
    drop(m);
    let _ = s.clone(); // only works after StrMut is dropped
}

#[cfg(feature = "serde")]
#[test]
fn serde_roundtrip() {
    let original: Str = "serde test".into();
    let json = serde_json::to_string(&original).unwrap();
    assert_eq!(json, "\"serde test\"");
    let deserialized: Str = serde_json::from_str(&json).unwrap();
    assert_eq!(original, deserialized);
}
