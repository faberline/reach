//! ImageIO PNG validation and CommonCrypto SHA-256 (no child process).
// A PNG signature alone is not evidence of an image. ImageIO must be able to
// parse a complete image with supported dimensions before it is hashed.
pub(crate) fn png_dimensions(data: &[u8]) -> Option<(usize, usize)> {
    use std::{ffi::c_void, ptr};
    type CFRef = *const c_void;
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFDataCreate(allocator: CFRef, bytes: *const u8, length: isize) -> CFRef;
        fn CFRelease(value: CFRef);
    }
    #[link(name = "ImageIO", kind = "framework")]
    extern "C" {
        fn CGImageSourceCreateWithData(data: CFRef, options: CFRef) -> CFRef;
        fn CGImageSourceGetStatus(source: CFRef) -> i32;
        fn CGImageSourceGetCount(source: CFRef) -> usize;
        fn CGImageSourceCreateImageAtIndex(source: CFRef, index: usize, options: CFRef) -> CFRef;
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGImageGetWidth(image: CFRef) -> usize;
        fn CGImageGetHeight(image: CFRef) -> usize;
    }
    unsafe {
        let cfdata = CFDataCreate(ptr::null(), data.as_ptr(), data.len() as isize);
        if cfdata.is_null() {
            return None;
        }
        let source = CGImageSourceCreateWithData(cfdata, ptr::null());
        CFRelease(cfdata);
        if source.is_null() {
            return None;
        }
        // kCGImageStatusComplete is 0. Do not accept a truncated source.
        let dimensions =
            if CGImageSourceGetStatus(source) == 0 && CGImageSourceGetCount(source) == 1 {
                let image = CGImageSourceCreateImageAtIndex(source, 0, ptr::null());
                if image.is_null() {
                    None
                } else {
                    let width = CGImageGetWidth(image);
                    let height = CGImageGetHeight(image);
                    CFRelease(image);
                    ((1..=8192).contains(&width) && (1..=8192).contains(&height))
                        .then_some((width, height))
                }
            } else {
                None
            };
        CFRelease(source);
        dimensions
    }
}

// CommonCrypto hashes the same bytes that ImageIO validated, without a child process.
pub(crate) fn sha256_bytes(data: &[u8]) -> Result<String, String> {
    #[link(name = "System")]
    extern "C" {
        fn CC_SHA256(data: *const std::ffi::c_void, len: u32, output: *mut u8) -> *mut u8;
    }
    let len = u32::try_from(data.len()).map_err(|_| "image too large to hash")?;
    let mut digest = [0u8; 32];
    let result = unsafe { CC_SHA256(data.as_ptr().cast(), len, digest.as_mut_ptr()) };
    if result.is_null() {
        return Err("SHA-256 failed".into());
    }
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}
