// PSRAM-backed allocations for ESP32.
//
// Provides `PsramBox<T>` (fixed-capacity buffer) and `PsramString` (immutable
// string) that allocate from PSRAM via `heap_caps_malloc` when compiled with
// `features = ["esp", "psram"]`, falling back to standard `alloc` on host.
//
// All unsafe code is confined to this file.

extern crate alloc;

use alloc::string::String;
use core::alloc::Layout;
use core::fmt;
use core::ops::Deref;
use core::ptr;

// ---------------------------------------------------------------------------
// Low-level alloc / free
// ---------------------------------------------------------------------------

#[cfg(all(feature = "esp", feature = "psram"))]
unsafe fn psram_alloc(layout: Layout) -> *mut u8 {
    // SPIRAM allocation — ignores layout.align() because heap_caps_malloc
    // returns naturally-aligned pointers (8-byte on ESP32-S2).
    let ptr = esp_idf_svc::sys::heap_caps_malloc(layout.size(), esp_idf_svc::sys::MALLOC_CAP_SPIRAM);
    ptr as *mut u8
}

#[cfg(all(feature = "esp", feature = "psram"))]
unsafe fn psram_free(ptr: *mut u8, _layout: Layout) {
    esp_idf_svc::sys::heap_caps_free(ptr as *mut core::ffi::c_void);
}

#[cfg(not(all(feature = "esp", feature = "psram")))]
unsafe fn psram_alloc(layout: Layout) -> *mut u8 {
    alloc::alloc::alloc(layout)
}

#[cfg(not(all(feature = "esp", feature = "psram")))]
unsafe fn psram_free(ptr: *mut u8, layout: Layout) {
    alloc::alloc::dealloc(ptr, layout);
}

// ---------------------------------------------------------------------------
// PsramBox<T> — fixed-capacity buffer in PSRAM
// ---------------------------------------------------------------------------

/// Fixed-capacity buffer allocated in PSRAM (ESP) or heap (host).
///
/// Elements are stored contiguously. `len` tracks how many slots are
/// initialized. The buffer never grows — panics if you push beyond capacity.
pub struct PsramBox<T> {
    ptr: *mut T,
    len: usize,
    capacity: usize,
}

// SAFETY: PsramBox owns its allocation exclusively — safe to send across threads.
unsafe impl<T: Send> Send for PsramBox<T> {}

impl<T> PsramBox<T> {
    /// Allocate a buffer with the given capacity. Panics on zero capacity or
    /// allocation failure (unrecoverable on ESP).
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "PsramBox capacity must be > 0");
        let layout = Layout::array::<T>(capacity).expect("layout overflow");
        let ptr = unsafe { psram_alloc(layout) } as *mut T;
        assert!(!ptr.is_null(), "PsramBox allocation failed");
        Self {
            ptr,
            len: 0,
            capacity,
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Append a value. Panics if at capacity.
    pub fn push(&mut self, val: T) {
        assert!(self.len < self.capacity, "PsramBox full");
        unsafe {
            self.ptr.add(self.len).write(val);
        }
        self.len += 1;
    }

    /// Read element at physical index. Panics if out of bounds.
    #[inline]
    pub fn get(&self, i: usize) -> &T {
        assert!(i < self.len, "PsramBox index {} out of bounds (len {})", i, self.len);
        unsafe { &*self.ptr.add(i) }
    }

    /// Mutable reference at physical index. Panics if out of bounds.
    #[inline]
    pub fn get_mut(&mut self, i: usize) -> &mut T {
        assert!(i < self.len, "PsramBox index {} out of bounds (len {})", i, self.len);
        unsafe { &mut *self.ptr.add(i) }
    }

    /// Overwrite element at index, dropping the old value.
    pub fn set(&mut self, i: usize, val: T) {
        assert!(i < self.len, "PsramBox set index {} out of bounds (len {})", i, self.len);
        unsafe {
            self.ptr.add(i).drop_in_place();
            self.ptr.add(i).write(val);
        }
    }

    /// Drop all elements and reset length to zero. Does not free the allocation.
    pub fn clear(&mut self) {
        for i in 0..self.len {
            unsafe { self.ptr.add(i).drop_in_place(); }
        }
        self.len = 0;
    }
}

impl<T: Clone> Clone for PsramBox<T> {
    fn clone(&self) -> Self {
        let mut new = PsramBox::new(self.capacity);
        for i in 0..self.len {
            new.push(unsafe { &*self.ptr.add(i) }.clone());
        }
        new
    }
}

impl<T: PartialEq> PartialEq for PsramBox<T> {
    fn eq(&self, other: &Self) -> bool {
        if self.len != other.len {
            return false;
        }
        for i in 0..self.len {
            if self.get(i) != other.get(i) {
                return false;
            }
        }
        true
    }
}

impl<T: fmt::Debug> fmt::Debug for PsramBox<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut list = f.debug_list();
        for i in 0..self.len {
            list.entry(self.get(i));
        }
        list.finish()
    }
}

impl<T> Drop for PsramBox<T> {
    fn drop(&mut self) {
        // Drop all initialized elements
        for i in 0..self.len {
            unsafe { self.ptr.add(i).drop_in_place(); }
        }
        // Free the allocation
        if self.capacity > 0 {
            let layout = Layout::array::<T>(self.capacity).expect("layout overflow");
            unsafe { psram_free(self.ptr as *mut u8, layout); }
        }
    }
}

// ---------------------------------------------------------------------------
// PsramString — immutable string in PSRAM
// ---------------------------------------------------------------------------

/// Immutable UTF-8 string allocated in PSRAM (ESP) or heap (host).
///
/// Created only from `&str`, preserving the UTF-8 invariant. Cannot be mutated
/// after creation.
pub struct PsramString {
    ptr: *mut u8,
    len: usize,
}

// SAFETY: PsramString owns its allocation and is immutable after creation.
unsafe impl Send for PsramString {}
unsafe impl Sync for PsramString {}

impl PsramString {
    /// Create a new PSRAM-backed string from a `&str`.
    pub fn from_str(s: &str) -> Self {
        if s.is_empty() {
            return Self {
                ptr: ptr::NonNull::dangling().as_ptr(),
                len: 0,
            };
        }
        let layout = Layout::array::<u8>(s.len()).expect("layout overflow");
        let ptr = unsafe { psram_alloc(layout) };
        assert!(!ptr.is_null(), "PsramString allocation failed");
        unsafe {
            ptr::copy_nonoverlapping(s.as_ptr(), ptr, s.len());
        }
        Self { ptr, len: s.len() }
    }

    /// View as a `&str`.
    #[inline]
    pub fn as_str(&self) -> &str {
        if self.len == 0 {
            return "";
        }
        unsafe {
            let bytes = core::slice::from_raw_parts(self.ptr, self.len);
            core::str::from_utf8_unchecked(bytes)
        }
    }
}

impl Deref for PsramString {
    type Target = str;
    #[inline]
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl Clone for PsramString {
    fn clone(&self) -> Self {
        PsramString::from_str(self.as_str())
    }
}

impl PartialEq for PsramString {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for PsramString {}

impl PartialEq<str> for PsramString {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for PsramString {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl fmt::Debug for PsramString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl fmt::Display for PsramString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.as_str(), f)
    }
}

impl Drop for PsramString {
    fn drop(&mut self) {
        if self.len > 0 {
            let layout = Layout::array::<u8>(self.len).expect("layout overflow");
            unsafe { psram_free(self.ptr, layout); }
        }
    }
}

impl From<&str> for PsramString {
    fn from(s: &str) -> Self {
        PsramString::from_str(s)
    }
}

impl From<String> for PsramString {
    fn from(s: String) -> Self {
        PsramString::from_str(&s)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- PsramBox tests --

    #[test]
    fn box_push_and_get() {
        let mut b: PsramBox<i32> = PsramBox::new(4);
        b.push(10);
        b.push(20);
        b.push(30);
        assert_eq!(b.len(), 3);
        assert_eq!(*b.get(0), 10);
        assert_eq!(*b.get(1), 20);
        assert_eq!(*b.get(2), 30);
    }

    #[test]
    fn box_set_overwrites() {
        let mut b: PsramBox<i32> = PsramBox::new(3);
        b.push(1);
        b.push(2);
        b.push(3);
        b.set(1, 99);
        assert_eq!(*b.get(1), 99);
    }

    #[test]
    fn box_clear_resets() {
        let mut b: PsramBox<i32> = PsramBox::new(5);
        b.push(1);
        b.push(2);
        b.clear();
        assert!(b.is_empty());
        assert_eq!(b.len(), 0);
        // Can push again after clear
        b.push(10);
        assert_eq!(*b.get(0), 10);
    }

    #[test]
    #[should_panic(expected = "PsramBox full")]
    fn box_push_overflow_panics() {
        let mut b: PsramBox<i32> = PsramBox::new(2);
        b.push(1);
        b.push(2);
        b.push(3);
    }

    #[test]
    #[should_panic(expected = "out of bounds")]
    fn box_get_out_of_bounds() {
        let b: PsramBox<i32> = PsramBox::new(5);
        let _ = b.get(0);
    }

    #[test]
    fn box_clone() {
        let mut b: PsramBox<i32> = PsramBox::new(3);
        b.push(10);
        b.push(20);
        let c = b.clone();
        assert_eq!(b, c);
        assert_eq!(c.len(), 2);
        assert_eq!(*c.get(0), 10);
    }

    #[test]
    fn box_partial_eq() {
        let mut a: PsramBox<i32> = PsramBox::new(3);
        a.push(1);
        a.push(2);
        let mut b: PsramBox<i32> = PsramBox::new(5); // different capacity
        b.push(1);
        b.push(2);
        assert_eq!(a, b);

        b.push(3);
        assert_ne!(a, b);
    }

    #[test]
    fn box_debug() {
        let mut b: PsramBox<i32> = PsramBox::new(3);
        b.push(1);
        b.push(2);
        let s = format!("{:?}", b);
        assert_eq!(s, "[1, 2]");
    }

    #[test]
    fn box_with_strings() {
        let mut b: PsramBox<String> = PsramBox::new(3);
        b.push("hello".to_string());
        b.push("world".to_string());
        assert_eq!(b.get(0).as_str(), "hello");
        b.set(0, "replaced".to_string());
        assert_eq!(b.get(0).as_str(), "replaced");
        b.clear();
        assert!(b.is_empty());
    }

    #[test]
    fn box_get_mut() {
        let mut b: PsramBox<i32> = PsramBox::new(3);
        b.push(10);
        *b.get_mut(0) = 42;
        assert_eq!(*b.get(0), 42);
    }

    // -- PsramString tests --

    #[test]
    fn string_from_str() {
        let s = PsramString::from_str("hello");
        assert_eq!(s.as_str(), "hello");
        assert_eq!(s.len(), 5);
    }

    #[test]
    fn string_empty() {
        let s = PsramString::from_str("");
        assert_eq!(s.as_str(), "");
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn string_deref() {
        let s = PsramString::from_str("hello world");
        // Deref to &str — can call str methods
        assert!(s.contains("world"));
        assert_eq!(&s[0..5], "hello");
    }

    #[test]
    fn string_clone() {
        let a = PsramString::from_str("test");
        let b = a.clone();
        assert_eq!(a, b);
        assert_eq!(b.as_str(), "test");
    }

    #[test]
    fn string_partial_eq() {
        let a = PsramString::from_str("abc");
        let b = PsramString::from_str("abc");
        let c = PsramString::from_str("xyz");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn string_eq_str() {
        let s = PsramString::from_str("hello");
        assert!(s == *"hello");
        assert!(s == "hello");
    }

    #[test]
    fn string_debug() {
        let s = PsramString::from_str("hello");
        assert_eq!(format!("{:?}", s), "\"hello\"");
    }

    #[test]
    fn string_display() {
        let s = PsramString::from_str("hello");
        assert_eq!(format!("{}", s), "hello");
    }

    #[test]
    fn string_from_string() {
        let owned = String::from("from owned");
        let s = PsramString::from(owned);
        assert_eq!(s.as_str(), "from owned");
    }

    #[test]
    fn string_utf8_preserved() {
        let s = PsramString::from_str("héllo wörld 🌍");
        assert_eq!(s.as_str(), "héllo wörld 🌍");
    }
}
