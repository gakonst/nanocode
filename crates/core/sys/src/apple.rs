use std::ffi::c_void;

const UTF8: u32 = 0x0800_0100;
const UTF8_BYTES_PER_UNIT_MAX: isize = 3;
const NAME_BYTES_MAX: usize = 64;

#[repr(C)]
struct Range {
    location: isize,
    length: isize,
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFTimeZoneResetSystem();
    fn CFTimeZoneCopySystem() -> *const c_void;
    fn CFTimeZoneGetName(zone: *const c_void) -> *const c_void;
    fn CFStringGetLength(string: *const c_void) -> isize;
    fn CFStringGetBytes(
        string: *const c_void,
        range: Range,
        encoding: u32,
        loss: u8,
        external: u8,
        buffer: *mut u8,
        capacity: isize,
        used: *mut isize,
    ) -> isize;
    fn CFRelease(object: *const c_void);
}

pub fn time_zone_name() -> Option<String> {
    let zone = unsafe {
        CFTimeZoneResetSystem();
        CFTimeZoneCopySystem()
    };
    if zone.is_null() {
        return None;
    }
    let name = unsafe { CFTimeZoneGetName(zone) };
    let text = (!name.is_null()).then(|| {
        let length = unsafe { CFStringGetLength(name) };
        let mut buffer = vec![0_u8; usize::try_from(length * UTF8_BYTES_PER_UNIT_MAX).ok()?];
        let mut used = 0;
        let converted = unsafe {
            CFStringGetBytes(
                name,
                Range {
                    location: 0,
                    length,
                },
                UTF8,
                0,
                0,
                buffer.as_mut_ptr(),
                length * UTF8_BYTES_PER_UNIT_MAX,
                &mut used,
            )
        };
        buffer.truncate(usize::try_from(used).ok()?);
        (converted == length)
            .then(|| String::from_utf8(buffer).ok())
            .flatten()
    });
    unsafe { CFRelease(zone) };
    text.flatten()
        .filter(|name| !name.is_empty() && name.len() <= NAME_BYTES_MAX)
}
