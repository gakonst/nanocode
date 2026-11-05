#![allow(unsafe_code, non_upper_case_globals)]

use base64::{Engine as _, engine::general_purpose::STANDARD};
use core_foundation::{
    base::{CFType, TCFType},
    boolean::CFBoolean,
    dictionary::CFDictionary,
    number::CFNumber,
    string::CFString,
};
use core_foundation_sys::{
    array::{CFArrayGetCount, CFArrayGetTypeID, CFArrayGetValueAtIndex, CFArrayRef},
    base::{
        CFEqual, CFGetTypeID, CFIndex, CFRange, CFRelease, CFRetain, CFTypeID, CFTypeRef,
        kCFAllocatorDefault,
    },
    data::{CFDataCreateMutable, CFDataGetBytePtr, CFDataGetLength, CFMutableDataRef},
    dictionary::{CFDictionaryGetValue, CFDictionaryRef},
    string::CFStringRef,
};
use std::{ffi::c_void, ptr, slice, thread, time::Duration};

pub type AXError = i32;
pub const AX_SUCCESS: AXError = 0;
pub const AX_INVALID: AXError = -25202;
pub const AX_CANNOT_COMPLETE: AXError = -25204;
pub const AX_ATTRIBUTE_UNSUPPORTED: AXError = -25205;
pub const AX_ACTION_UNSUPPORTED: AXError = -25206;
pub const AX_NOT_IMPLEMENTED: AXError = -25208;
pub const AX_API_DISABLED: AXError = -25211;
const AX_NO_VALUE: AXError = -25212;
const AX_RANGE: u32 = 4;
const MESSAGING_TIMEOUT: f32 = 1.0;
const ACTION_TIMEOUT: f32 = 10.0;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXUIElementGetTypeID() -> CFTypeID;
    fn AXUIElementCreateApplication(pid: i32) -> CFTypeRef;
    fn AXUIElementCopyAttributeValue(
        element: CFTypeRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    fn AXUIElementSetAttributeValue(
        element: CFTypeRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> AXError;
    fn AXUIElementCopyActionNames(element: CFTypeRef, names: *mut CFArrayRef) -> AXError;
    fn AXUIElementPerformAction(element: CFTypeRef, action: CFStringRef) -> AXError;
    fn AXUIElementSetMessagingTimeout(element: CFTypeRef, seconds: f32) -> AXError;
    fn AXValueCreate(kind: u32, value: *const c_void) -> CFTypeRef;
    fn _AXUIElementGetWindow(element: CFTypeRef, window: *mut u32) -> AXError;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventCreateKeyboardEvent(source: CFTypeRef, key: u16, down: bool) -> CFTypeRef;
    fn CGEventSetFlags(event: CFTypeRef, flags: u64);
    fn CGEventKeyboardSetUnicodeString(event: CFTypeRef, length: usize, text: *const u16);
    fn CGEventPostToPid(pid: i32, event: CFTypeRef);
    fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> CFArrayRef;
    fn CGWindowListCreateImage(
        bounds: CGRect,
        option: u32,
        window: u32,
        image_option: u32,
    ) -> CFTypeRef;
    fn CGPreflightScreenCaptureAccess() -> bool;
}

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    static kCGImageDestinationLossyCompressionQuality: CFStringRef;
    fn CGImageDestinationCreateWithData(
        data: CFMutableDataRef,
        kind: CFStringRef,
        count: usize,
        options: CFDictionaryRef,
    ) -> CFTypeRef;
    fn CGImageDestinationAddImage(
        destination: CFTypeRef,
        image: CFTypeRef,
        properties: CFDictionaryRef,
    );
    fn CGImageDestinationFinalize(destination: CFTypeRef) -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFBundleGetValueForInfoDictionaryKey(bundle: CFTypeRef, key: CFStringRef) -> CFTypeRef;
    fn CFBundleGetIdentifier(bundle: CFTypeRef) -> CFStringRef;
    fn CFBundleCreate(allocator: CFTypeRef, url: CFTypeRef) -> CFTypeRef;
    fn CFURLCreateFromFileSystemRepresentation(
        allocator: CFTypeRef,
        path: *const u8,
        length: CFIndex,
        directory: bool,
    ) -> CFTypeRef;
}

pub fn trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

pub struct Element(CFTypeRef);

unsafe impl Send for Element {}

impl Element {
    pub fn application(pid: i32) -> Option<Self> {
        let object = unsafe { AXUIElementCreateApplication(pid) };
        (!object.is_null()).then(|| Self::bounded(object))
    }

    fn retained(object: CFTypeRef) -> Self {
        unsafe { CFRetain(object) };
        Self::bounded(object)
    }

    fn bounded(object: CFTypeRef) -> Self {
        unsafe { AXUIElementSetMessagingTimeout(object, MESSAGING_TIMEOUT) };
        Self(object)
    }

    fn copy(&self, attribute: &str) -> Result<CFType, AXError> {
        let name = CFString::new(attribute);
        let mut value: CFTypeRef = ptr::null();
        let error = unsafe {
            AXUIElementCopyAttributeValue(self.0, name.as_concrete_TypeRef(), &mut value)
        };
        if error != AX_SUCCESS {
            return Err(error);
        }
        if value.is_null() {
            return Err(AX_NO_VALUE);
        }
        Ok(unsafe { CFType::wrap_under_create_rule(value) })
    }

    pub fn text(&self, attribute: &str) -> Option<String> {
        let value = self.copy(attribute).ok()?;
        let text = if let Some(text) = value.downcast::<CFString>() {
            text.to_string()
        } else if let Some(number) = value.downcast::<CFNumber>() {
            let number = number.to_f64()?;
            if number.fract() == 0.0 {
                format!("{number:.0}")
            } else {
                number.to_string()
            }
        } else if bool::from(value.downcast::<CFBoolean>()?) {
            "1".into()
        } else {
            "0".into()
        };
        (!text.is_empty()).then_some(text)
    }

    pub fn numeric(&self, attribute: &str) -> bool {
        self.copy(attribute)
            .is_ok_and(|value| value.downcast::<CFNumber>().is_some())
    }

    pub fn flag(&self, attribute: &str) -> Option<bool> {
        let value = self.copy(attribute).ok()?;
        value.downcast::<CFBoolean>().map(bool::from).or_else(|| {
            value
                .downcast::<CFNumber>()
                .and_then(|n| n.to_i64())
                .map(|n| n != 0)
        })
    }

    pub fn element(&self, attribute: &str) -> Option<Self> {
        let value = self.copy(attribute).ok()?;
        let is_element = unsafe { CFGetTypeID(value.as_CFTypeRef()) == AXUIElementGetTypeID() };
        is_element.then(|| Self::retained(value.as_CFTypeRef()))
    }

    pub fn elements(&self, attribute: &str, limit: usize) -> Vec<Self> {
        let Ok(value) = self.copy(attribute) else {
            return Vec::new();
        };
        array_items(value.as_CFTypeRef(), limit)
            .into_iter()
            .filter(|&item| unsafe { CFGetTypeID(item) == AXUIElementGetTypeID() })
            .map(Self::retained)
            .collect()
    }

    pub fn actions(&self) -> Vec<String> {
        let mut names: CFArrayRef = ptr::null();
        if unsafe { AXUIElementCopyActionNames(self.0, &mut names) } != AX_SUCCESS
            || names.is_null()
        {
            return Vec::new();
        }
        let names = unsafe { CFType::wrap_under_create_rule(names as CFTypeRef) };
        array_items(names.as_CFTypeRef(), 64)
            .into_iter()
            .map(|item| unsafe { CFString::wrap_under_get_rule(item as CFStringRef) }.to_string())
            .collect()
    }

    pub fn perform(&self, action: &str) -> AXError {
        let name = CFString::new(action);
        unsafe {
            AXUIElementSetMessagingTimeout(self.0, ACTION_TIMEOUT);
            let error = AXUIElementPerformAction(self.0, name.as_concrete_TypeRef());
            AXUIElementSetMessagingTimeout(self.0, MESSAGING_TIMEOUT);
            error
        }
    }

    fn set(&self, attribute: &str, value: CFTypeRef) -> AXError {
        let name = CFString::new(attribute);
        unsafe {
            AXUIElementSetMessagingTimeout(self.0, ACTION_TIMEOUT);
            let error = AXUIElementSetAttributeValue(self.0, name.as_concrete_TypeRef(), value);
            AXUIElementSetMessagingTimeout(self.0, MESSAGING_TIMEOUT);
            error
        }
    }

    pub fn set_true(&self, attribute: &str) -> AXError {
        self.set(attribute, CFBoolean::true_value().as_CFTypeRef())
    }

    pub fn set_number(&self, attribute: &str, number: f64) -> AXError {
        self.set(attribute, CFNumber::from(number).as_CFTypeRef())
    }

    pub fn set_text(&self, attribute: &str, text: &str) -> AXError {
        self.set(attribute, CFString::new(text).as_CFTypeRef())
    }

    pub fn set_range(&self, attribute: &str, location: usize, length: usize) -> AXError {
        let range = CFRange {
            location: location as CFIndex,
            length: length as CFIndex,
        };
        let value = unsafe {
            CFType::wrap_under_create_rule(AXValueCreate(AX_RANGE, (&raw const range).cast()))
        };
        self.set(attribute, value.as_CFTypeRef())
    }

    pub fn alive(&self) -> bool {
        !matches!(self.copy("AXRole"), Err(AX_INVALID))
    }

    pub fn window_id(&self) -> Option<u32> {
        let mut id = 0u32;
        (unsafe { _AXUIElementGetWindow(self.0, &mut id) } == AX_SUCCESS && id != 0).then_some(id)
    }
}

impl Clone for Element {
    fn clone(&self) -> Self {
        Self::retained(self.0)
    }
}

impl PartialEq for Element {
    fn eq(&self, other: &Self) -> bool {
        unsafe { CFEqual(self.0, other.0) != 0 }
    }
}

impl Drop for Element {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) };
    }
}

fn array_items(object: CFTypeRef, limit: usize) -> Vec<CFTypeRef> {
    unsafe {
        if object.is_null() || CFGetTypeID(object) != CFArrayGetTypeID() {
            return Vec::new();
        }
        let count = CFArrayGetCount(object as CFArrayRef).max(0) as usize;
        (0..count.min(limit))
            .map(|index| CFArrayGetValueAtIndex(object as CFArrayRef, index as CFIndex))
            .filter(|item| !item.is_null())
            .collect()
    }
}

pub fn post_key(pid: i32, code: u16, flags: u64, down: bool, text: Option<&[u16]>) {
    unsafe {
        let event = CGEventCreateKeyboardEvent(ptr::null(), code, down);
        if event.is_null() {
            return;
        }
        CGEventSetFlags(event, flags);
        if let Some(text) = text {
            CGEventKeyboardSetUnicodeString(event, text.len(), text.as_ptr());
        }
        CGEventPostToPid(pid, event);
        CFRelease(event);
    }
}

pub fn type_keys(pid: i32, text: &str) {
    let units: Vec<u16> = text.encode_utf16().collect();
    for chunk in units.chunks(16) {
        post_key(pid, 0, 0, true, Some(chunk));
        post_key(pid, 0, 0, false, Some(chunk));
        thread::sleep(Duration::from_millis(8));
    }
}

pub fn screenshot(window: &Element, max: usize) -> Result<Option<String>, String> {
    const INCLUDING_WINDOW: u32 = 1 << 3;
    const BOUNDS_IGNORE_FRAMING: u32 = 1 << 0;
    const NOMINAL_RESOLUTION: u32 = 1 << 4;
    if !unsafe { CGPreflightScreenCaptureAccess() } {
        return Err(
            "screenshots need Screen Recording permission; the state above stands without one"
                .into(),
        );
    }
    let id = window
        .window_id()
        .ok_or("the window could not be found on screen")?;
    let null = CGRect {
        x: f64::INFINITY,
        y: f64::INFINITY,
        width: 0.0,
        height: 0.0,
    };
    let image = unsafe {
        CGWindowListCreateImage(
            null,
            INCLUDING_WINDOW,
            id,
            BOUNDS_IGNORE_FRAMING | NOMINAL_RESOLUTION,
        )
    };
    if image.is_null() {
        return Err("the window could not be captured".into());
    }
    let image = unsafe { CFType::wrap_under_create_rule(image) };
    Ok([0.8, 0.5, 0.3]
        .into_iter()
        .filter_map(|quality| jpeg(image.as_CFTypeRef(), quality))
        .map(|jpeg| STANDARD.encode(jpeg))
        .find(|data| data.len() <= max))
}

fn jpeg(image: CFTypeRef, quality: f64) -> Option<Vec<u8>> {
    unsafe {
        let data = CFDataCreateMutable(kCFAllocatorDefault, 0);
        if data.is_null() {
            return None;
        }
        let owner = CFType::wrap_under_create_rule(data as CFTypeRef);
        let kind = CFString::new("public.jpeg");
        let destination =
            CGImageDestinationCreateWithData(data, kind.as_concrete_TypeRef(), 1, ptr::null());
        if destination.is_null() {
            return None;
        }
        let destination = CFType::wrap_under_create_rule(destination);
        let key = CFString::wrap_under_get_rule(kCGImageDestinationLossyCompressionQuality);
        let options = CFDictionary::from_CFType_pairs(&[(
            key.as_CFType(),
            CFNumber::from(quality).as_CFType(),
        )]);
        CGImageDestinationAddImage(
            destination.as_CFTypeRef(),
            image,
            options.as_concrete_TypeRef(),
        );
        if !CGImageDestinationFinalize(destination.as_CFTypeRef()) {
            return None;
        }
        let length = CFDataGetLength(data).max(0) as usize;
        let bytes = slice::from_raw_parts(CFDataGetBytePtr(data), length).to_vec();
        drop(owner);
        Some(bytes)
    }
}

pub fn processes() -> Vec<(i32, String)> {
    let mut pids = vec![0i32; 8192];
    let count =
        unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), (pids.len() * 4) as i32) };
    pids.truncate(count.max(0) as usize);
    pids.into_iter()
        .filter(|&pid| pid > 0)
        .filter_map(|pid| {
            let mut buffer = vec![0u8; 4096];
            let length =
                unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
            (length > 0).then(|| {
                (
                    pid,
                    String::from_utf8_lossy(&buffer[..length as usize]).into_owned(),
                )
            })
        })
        .collect()
}

pub fn bundle_info(path: &str) -> (String, String, bool) {
    let stem = path
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .trim_end_matches(".app")
        .to_owned();
    unsafe {
        let url = CFURLCreateFromFileSystemRepresentation(
            ptr::null(),
            path.as_ptr(),
            path.len() as CFIndex,
            true,
        );
        if url.is_null() {
            return (String::new(), stem, false);
        }
        let _url = CFType::wrap_under_create_rule(url);
        let bundle = CFBundleCreate(ptr::null(), url);
        if bundle.is_null() {
            return (String::new(), stem, false);
        }
        let bundle = CFType::wrap_under_create_rule(bundle);
        let value =
            |object: CFTypeRef| (!object.is_null()).then(|| CFType::wrap_under_get_rule(object));
        let string = |object: CFTypeRef| {
            value(object)
                .and_then(|v| v.downcast::<CFString>())
                .map(|s| s.to_string())
        };
        let key = |name: &str| {
            let name = CFString::new(name);
            CFBundleGetValueForInfoDictionaryKey(bundle.as_CFTypeRef(), name.as_concrete_TypeRef())
        };
        let truthy = |object: CFTypeRef| {
            value(object).is_some_and(|value| {
                value
                    .downcast::<CFBoolean>()
                    .map(bool::from)
                    .or_else(|| value.downcast::<CFString>().map(|s| s == "1"))
                    .or_else(|| {
                        value
                            .downcast::<CFNumber>()
                            .and_then(|n| n.to_i64())
                            .map(|n| n != 0)
                    })
                    .unwrap_or(false)
            })
        };
        let id =
            string(CFBundleGetIdentifier(bundle.as_CFTypeRef()) as CFTypeRef).unwrap_or_default();
        let name = string(key("CFBundleDisplayName"))
            .or_else(|| string(key("CFBundleName")))
            .unwrap_or(stem);
        let background = truthy(key("LSUIElement")) || truthy(key("LSBackgroundOnly"));
        (id, name, background)
    }
}

pub fn on_screen() -> Vec<i32> {
    const ON_SCREEN_ONLY: u32 = 1 << 0;
    const EXCLUDE_DESKTOP: u32 = 1 << 4;
    unsafe {
        let windows = CGWindowListCopyWindowInfo(ON_SCREEN_ONLY | EXCLUDE_DESKTOP, 0);
        if windows.is_null() {
            return Vec::new();
        }
        let windows = CFType::wrap_under_create_rule(windows as CFTypeRef);
        let number = |dict: CFTypeRef, key: &str| {
            let key = CFString::new(key);
            let value =
                CFDictionaryGetValue(dict as CFDictionaryRef, key.as_concrete_TypeRef().cast());
            (!value.is_null())
                .then(|| CFType::wrap_under_get_rule(value))
                .and_then(|v| v.downcast::<CFNumber>())
                .and_then(|n| n.to_i64())
        };
        array_items(windows.as_CFTypeRef(), 4096)
            .into_iter()
            .filter(|&window| number(window, "kCGWindowLayer") == Some(0))
            .filter_map(|window| number(window, "kCGWindowOwnerPID").map(|pid| pid as i32))
            .collect()
    }
}
