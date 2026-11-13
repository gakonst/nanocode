#![allow(unsafe_code, non_upper_case_globals)]

use crate::{Button, state::shown_units};
use nanocode_sys::base64::STANDARD;
use core_foundation::{
    array::CFArray,
    base::{CFType, TCFType},
    boolean::CFBoolean,
    dictionary::CFDictionary,
    number::CFNumber,
    string::CFString,
};
use core_foundation_sys::{
    array::{CFArrayGetCount, CFArrayGetTypeID, CFArrayGetValueAtIndex, CFArrayRef},
    base::{
        CFEqual, CFGetTypeID, CFHash, CFIndex, CFNullGetTypeID, CFRange, CFRelease, CFRetain,
        CFTypeID, CFTypeRef, kCFAllocatorDefault,
    },
    data::{CFDataCreateMutable, CFDataGetBytePtr, CFDataGetLength, CFMutableDataRef},
    dictionary::{CFDictionaryGetValue, CFDictionaryRef},
    runloop::{
        CFRunLoopAddSource, CFRunLoopGetCurrent, CFRunLoopRef, CFRunLoopRemoveSource,
        CFRunLoopRunInMode, CFRunLoopSourceRef, kCFRunLoopRunFinished,
    },
    string::{
        CFStringCreateWithSubstring, CFStringGetCharacterAtIndex, CFStringGetLength, CFStringRef,
    },
};
use std::{
    ffi::c_void,
    hash::{Hash, Hasher},
    ptr, slice,
    sync::atomic::{AtomicI64, AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};

pub type AXError = i32;
pub const AX_SUCCESS: AXError = 0;
pub const AX_INVALID: AXError = -25202;
pub const AX_CANNOT_COMPLETE: AXError = -25204;
pub const AX_ATTRIBUTE_UNSUPPORTED: AXError = -25205;
pub const AX_ACTION_UNSUPPORTED: AXError = -25206;
pub const AX_NOT_IMPLEMENTED: AXError = -25208;
pub const AX_API_DISABLED: AXError = -25211;
const AX_NO_VALUE: AXError = -25212;
const AX_POINT: u32 = 1;
const AX_SIZE: u32 = 2;
const AX_RANGE: u32 = 4;
const AX_ERROR_VALUE: u32 = 5;
const MESSAGING_TIMEOUT: f32 = 1.0;
const ACTION_TIMEOUT: f32 = 10.0;
const SETTLING: &str = "nanocode.computer.settle";
const CHANGES: [&str; 10] = [
    "AXValueChanged",
    "AXUIElementDestroyed",
    "AXCreated",
    "AXFocusedUIElementChanged",
    "AXFocusedWindowChanged",
    "AXWindowCreated",
    "AXTitleChanged",
    "AXSelectedChildrenChanged",
    "AXSelectedRowsChanged",
    "AXRowCountChanged",
];

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CGSize {
    width: f64,
    height: f64,
}

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
    static kAXTrustedCheckOptionPrompt: CFStringRef;
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
    fn AXValueGetValue(value: CFTypeRef, kind: u32, out: *mut c_void) -> bool;
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
    fn AXUIElementCopyMultipleAttributeValues(
        element: CFTypeRef,
        attributes: CFArrayRef,
        options: u32,
        values: *mut CFArrayRef,
    ) -> AXError;
    fn AXValueGetTypeID() -> CFTypeID;
    fn AXValueGetType(value: CFTypeRef) -> u32;
    fn AXUIElementCopyActionNames(element: CFTypeRef, names: *mut CFArrayRef) -> AXError;
    fn AXUIElementPerformAction(element: CFTypeRef, action: CFStringRef) -> AXError;
    fn AXUIElementSetMessagingTimeout(element: CFTypeRef, seconds: f32) -> AXError;
    fn AXValueCreate(kind: u32, value: *const c_void) -> CFTypeRef;
    fn AXObserverCreate(
        application: i32,
        callback: unsafe extern "C" fn(CFTypeRef, CFTypeRef, CFStringRef, *mut c_void),
        observer: *mut CFTypeRef,
    ) -> AXError;
    fn AXObserverAddNotification(
        observer: CFTypeRef,
        element: CFTypeRef,
        notification: CFStringRef,
        refcon: *mut c_void,
    ) -> AXError;
    fn AXObserverGetRunLoopSource(observer: CFTypeRef) -> CFRunLoopSourceRef;
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
    fn CGRequestScreenCaptureAccess() -> bool;
    fn CGEventCreateMouseEvent(source: CFTypeRef, kind: u32, at: CGPoint, button: u32)
    -> CFTypeRef;
    fn CGEventCreateScrollWheelEvent(
        source: CFTypeRef,
        units: u32,
        count: u32,
        wheel: i32,
        ...
    ) -> CFTypeRef;
    fn CGEventSetLocation(event: CFTypeRef, at: CGPoint);
    fn CGEventSourceCreate(state: i32) -> CFTypeRef;
    fn CGEventSetIntegerValueField(event: CFTypeRef, field: u32, value: i64);
    fn CGSessionCopyCurrentDictionary() -> CFDictionaryRef;
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

pub fn request_trust() -> bool {
    let prompt = unsafe { CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt) };
    let options = CFDictionary::from_CFType_pairs(&[(
        prompt.as_CFType(),
        CFBoolean::true_value().as_CFType(),
    )]);
    unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef()) }
}

pub fn screen_recording() -> bool {
    unsafe { CGPreflightScreenCaptureAccess() }
}

pub fn request_screen_recording() -> bool {
    unsafe { CGRequestScreenCaptureAccess() }
}

pub fn locked() -> bool {
    let session = unsafe { CGSessionCopyCurrentDictionary() };
    if session.is_null() {
        return false;
    }
    let session: CFDictionary<CFString, CFType> =
        unsafe { CFDictionary::wrap_under_create_rule(session) };
    session
        .find(CFString::from_static_string("CGSSessionScreenIsLocked"))
        .and_then(|locked| locked.downcast::<CFBoolean>())
        .is_some_and(bool::from)
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

    pub fn many<const N: usize>(&self, attributes: &[&str; N]) -> [Option<CFType>; N] {
        let names: Vec<CFString> = attributes.iter().map(|name| CFString::new(name)).collect();
        let names = CFArray::from_CFTypes(&names);
        let mut values: CFArrayRef = ptr::null();
        let error = unsafe {
            AXUIElementCopyMultipleAttributeValues(
                self.0,
                names.as_concrete_TypeRef(),
                0,
                &mut values,
            )
        };
        if error != AX_SUCCESS || values.is_null() {
            return std::array::from_fn(|_| None);
        }
        let values = unsafe { CFType::wrap_under_create_rule(values as CFTypeRef) };
        let items = array_items(values.as_CFTypeRef(), N);
        std::array::from_fn(|index| {
            let item = *items.get(index)?;
            let failed = unsafe {
                CFGetTypeID(item) == CFNullGetTypeID()
                    || CFGetTypeID(item) == AXValueGetTypeID()
                        && AXValueGetType(item) == AX_ERROR_VALUE
            };
            (!failed).then(|| unsafe { CFType::wrap_under_get_rule(item) })
        })
    }

    pub fn text(&self, attribute: &str) -> Option<String> {
        text_of(&self.copy(attribute).ok()?)
    }

    pub fn numeric(&self, attribute: &str) -> bool {
        self.copy(attribute)
            .is_ok_and(|value| value.downcast::<CFNumber>().is_some())
    }

    pub fn flag(&self, attribute: &str) -> Option<bool> {
        flag_of(&self.copy(attribute).ok()?)
    }

    pub fn element(&self, attribute: &str) -> Option<Self> {
        let value = self.copy(attribute).ok()?;
        let is_element = unsafe { CFGetTypeID(value.as_CFTypeRef()) == AXUIElementGetTypeID() };
        is_element.then(|| Self::retained(value.as_CFTypeRef()))
    }

    pub fn elements(&self, attribute: &str, limit: usize) -> Vec<Self> {
        self.copy(attribute)
            .map(|value| elements_of(&value, limit))
            .unwrap_or_default()
    }
}

pub fn text_of(value: &CFType) -> Option<String> {
    read(value, |text| text.to_string())
}

pub fn label_of(value: &CFType) -> Option<String> {
    read(value, shown)
}

fn shown(text: &CFString) -> String {
    let string = text.as_concrete_TypeRef();
    let len = usize::try_from(unsafe { CFStringGetLength(string) }).unwrap_or(0);
    let end = shown_units(len, |index| unsafe {
        CFStringGetCharacterAtIndex(string, index as CFIndex)
    });
    if end == len {
        return text.to_string();
    }
    let range = CFRange {
        location: 0,
        length: end as CFIndex,
    };
    let prefix = unsafe { CFStringCreateWithSubstring(kCFAllocatorDefault, string, range) };
    if prefix.is_null() {
        return String::new();
    }
    unsafe { CFString::wrap_under_create_rule(prefix) }.to_string()
}

fn read(value: &CFType, string: impl Fn(&CFString) -> String) -> Option<String> {
    let text = if let Some(text) = value.downcast::<CFString>() {
        string(&text)
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

pub fn flag_of(value: &CFType) -> Option<bool> {
    value.downcast::<CFBoolean>().map(bool::from).or_else(|| {
        value
            .downcast::<CFNumber>()
            .and_then(|n| n.to_i64())
            .map(|n| n != 0)
    })
}

pub fn elements_of(value: &CFType, limit: usize) -> Vec<Element> {
    array_items(value.as_CFTypeRef(), limit)
        .into_iter()
        .filter(|&item| unsafe { CFGetTypeID(item) == AXUIElementGetTypeID() })
        .map(Element::retained)
        .collect()
}

impl Element {
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

    pub fn is_window(&self) -> bool {
        self.text("AXRole").as_deref() == Some("AXWindow")
    }

    pub fn alive(&self) -> bool {
        !matches!(self.copy("AXRole"), Err(AX_INVALID))
    }

    pub fn frame(&self) -> Option<(f64, f64, f64, f64)> {
        let [position, size] = self.many(&["AXPosition", "AXSize"]);
        let position: CGPoint = geometry(&position?, AX_POINT)?;
        let size: CGSize = geometry(&size?, AX_SIZE)?;
        Some((position.x, position.y, size.width, size.height))
    }

    pub fn window_id(&self) -> Option<u32> {
        let mut id = 0u32;
        (unsafe { _AXUIElementGetWindow(self.0, &mut id) } == AX_SUCCESS && id != 0).then_some(id)
    }
}

fn geometry<T: Default>(value: &CFType, kind: u32) -> Option<T> {
    let mut out = T::default();
    let read = unsafe {
        CFGetTypeID(value.as_CFTypeRef()) == AXValueGetTypeID()
            && AXValueGetValue(value.as_CFTypeRef(), kind, (&raw mut out).cast())
    };
    read.then_some(out)
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

impl Eq for Element {}

impl Hash for Element {
    fn hash<H: Hasher>(&self, state: &mut H) {
        unsafe { CFHash(self.0) }.hash(state);
    }
}

impl Drop for Element {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) };
    }
}

pub struct Observer {
    observer: CFTypeRef,
    run_loop: CFRunLoopRef,
    seen: Box<AtomicUsize>,
    handled: usize,
}

unsafe impl Send for Observer {}

unsafe extern "C" fn noticed(_: CFTypeRef, _: CFTypeRef, _: CFStringRef, seen: *mut c_void) {
    unsafe { &*seen.cast::<AtomicUsize>() }.fetch_add(1, Ordering::Relaxed);
}

impl Observer {
    pub fn new(pid: i32) -> Option<Self> {
        let mut observer: CFTypeRef = ptr::null();
        if unsafe { AXObserverCreate(pid, noticed, &mut observer) } != AX_SUCCESS
            || observer.is_null()
        {
            return None;
        }
        let seen = Box::new(AtomicUsize::new(0));
        let app = Element::application(pid);
        let added = app.as_ref().is_some_and(|app| {
            CHANGES.iter().fold(false, |added, change| {
                let name = CFString::new(change);
                let error = unsafe {
                    AXObserverAddNotification(
                        observer,
                        app.0,
                        name.as_concrete_TypeRef(),
                        (&raw const *seen).cast_mut().cast(),
                    )
                };
                added | (error == AX_SUCCESS)
            })
        });
        if !added {
            unsafe { CFRelease(observer) };
            return None;
        }
        let run_loop = unsafe { CFRunLoopGetCurrent() };
        let mode = CFString::from_static_string(SETTLING);
        unsafe {
            CFRunLoopAddSource(
                run_loop,
                AXObserverGetRunLoopSource(observer),
                mode.as_concrete_TypeRef(),
            );
        }
        Some(Self {
            observer,
            run_loop,
            seen,
            handled: 0,
        })
    }

    pub fn settle(&mut self, first: Duration, quiet: Duration) -> bool {
        let mode = CFString::from_static_string(SETTLING);
        let start = Instant::now();
        let cap = start + first + quiet;
        let mut until = start + first;
        let mut changed = false;
        loop {
            let wait = until.saturating_duration_since(Instant::now());
            let ran =
                unsafe { CFRunLoopRunInMode(mode.as_concrete_TypeRef(), wait.as_secs_f64(), 1) };
            if ran == kCFRunLoopRunFinished {
                return changed;
            }
            let seen = self.seen.load(Ordering::Relaxed);
            if seen > self.handled {
                self.handled = seen;
                changed = true;
                until = (Instant::now() + quiet).min(cap);
            } else if Instant::now() >= until {
                return changed;
            }
        }
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        let mode = CFString::from_static_string(SETTLING);
        unsafe {
            CFRunLoopRemoveSource(
                self.run_loop,
                AXObserverGetRunLoopSource(self.observer),
                mode.as_concrete_TypeRef(),
            );
            CFRelease(self.observer);
        }
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

const HID_SYSTEM_STATE: i32 = 1;
const MOUSE_MOVED: u32 = 5;
const LEFT_MOUSE_DOWN: u32 = 1;
const LEFT_MOUSE_UP: u32 = 2;
const RIGHT_MOUSE_DOWN: u32 = 3;
const RIGHT_MOUSE_UP: u32 = 4;
const LEFT_MOUSE_DRAGGED: u32 = 6;
const OTHER_MOUSE_DOWN: u32 = 25;
const OTHER_MOUSE_UP: u32 = 26;
const MOUSE_BUTTON_LEFT: u32 = 0;
const MOUSE_BUTTON_RIGHT: u32 = 1;
const MOUSE_BUTTON_CENTER: u32 = 2;
const FIELD_CLICK_STATE: u32 = 1;
const FIELD_BUTTON_NUMBER: u32 = 3;
const FIELD_SUBTYPE: u32 = 7;
const FIELD_TARGET_PID: u32 = 40;
const FIELD_WINDOW_NUMBER: u32 = 51;
const FIELD_CLICK_GROUP: u32 = 58;
const FIELD_WINDOW_UNDER_POINTER: u32 = 91;
const FIELD_WINDOW_THAT_CAN_HANDLE: u32 = 92;
const SUBTYPE_TOUCH: i64 = 3;
const SCROLL_PIXELS: u32 = 0;
const SCROLL_AXES: u32 = 2;
const PRIMER_SETTLE: Duration = Duration::from_millis(12);
const PRESS_HOLD: Duration = Duration::from_millis(28);
const CLICK_GAP: Duration = Duration::from_millis(80);

static GESTURES: AtomicI64 = AtomicI64::new(0);

const fn mouse_kinds(button: Button) -> (u32, u32, u32) {
    match button {
        Button::Left => (LEFT_MOUSE_DOWN, LEFT_MOUSE_UP, MOUSE_BUTTON_LEFT),
        Button::Right => (RIGHT_MOUSE_DOWN, RIGHT_MOUSE_UP, MOUSE_BUTTON_RIGHT),
        Button::Middle => (OTHER_MOUSE_DOWN, OTHER_MOUSE_UP, MOUSE_BUTTON_CENTER),
    }
}

struct Gesture {
    pid: i32,
    window: Option<u32>,
    group: i64,
}

impl Gesture {
    fn new(pid: i32, window: Option<u32>) -> Self {
        Self {
            pid,
            window,
            group: GESTURES.fetch_add(1, Ordering::Relaxed) + 1,
        }
    }

    fn stamp(&self, event: CFTypeRef) {
        unsafe {
            CGEventSetIntegerValueField(event, FIELD_TARGET_PID, self.pid.into());
            if let Some(window) = self.window {
                CGEventSetIntegerValueField(event, FIELD_WINDOW_NUMBER, window.into());
                CGEventSetIntegerValueField(event, FIELD_CLICK_GROUP, self.group);
                CGEventSetIntegerValueField(event, FIELD_WINDOW_UNDER_POINTER, window.into());
                CGEventSetIntegerValueField(event, FIELD_WINDOW_THAT_CAN_HANDLE, window.into());
            }
        }
    }

    fn post(&self, event: CFTypeRef) {
        if event.is_null() {
            return;
        }
        self.stamp(event);
        unsafe {
            CGEventPostToPid(self.pid, event);
            CFRelease(event);
        }
    }

    fn mouse(&self, kind: u32, at: (f64, f64), button: u32, clicks: u32) {
        unsafe {
            let source = CGEventSourceCreate(HID_SYSTEM_STATE);
            let event = CGEventCreateMouseEvent(source, kind, CGPoint { x: at.0, y: at.1 }, button);
            if !source.is_null() {
                CFRelease(source);
            }
            if !event.is_null() {
                CGEventSetIntegerValueField(event, FIELD_CLICK_STATE, clicks.into());
                CGEventSetIntegerValueField(event, FIELD_BUTTON_NUMBER, button.into());
                CGEventSetIntegerValueField(event, FIELD_SUBTYPE, SUBTYPE_TOUCH);
            }
            self.post(event);
        }
    }

    fn prime(&self, at: (f64, f64), button: u32) {
        self.mouse(MOUSE_MOVED, at, button, 0);
        thread::sleep(PRIMER_SETTLE);
    }
}

pub fn click(pid: i32, window: Option<u32>, at: (f64, f64), button: Button, count: u32) {
    let (down, up, number) = mouse_kinds(button);
    let gesture = Gesture::new(pid, window);
    gesture.prime(at, number);
    for clicks in 1..=count {
        gesture.mouse(down, at, number, clicks);
        thread::sleep(PRESS_HOLD);
        gesture.mouse(up, at, number, clicks);
        thread::sleep(CLICK_GAP);
    }
}

pub fn drag(pid: i32, window: Option<u32>, from: (f64, f64), to: (f64, f64)) {
    let (down, up, number) = mouse_kinds(Button::Left);
    let gesture = Gesture::new(pid, window);
    gesture.prime(from, number);
    gesture.mouse(down, from, number, 1);
    thread::sleep(PRESS_HOLD);
    gesture.mouse(LEFT_MOUSE_DRAGGED, to, number, 1);
    thread::sleep(PRESS_HOLD);
    gesture.mouse(up, to, number, 1);
}

pub fn scroll(pid: i32, window: Option<u32>, at: (f64, f64), horizontal: i32, vertical: i32) {
    let gesture = Gesture::new(pid, window);
    gesture.prime(at, MOUSE_BUTTON_LEFT);
    unsafe {
        let source = CGEventSourceCreate(HID_SYSTEM_STATE);
        let event =
            CGEventCreateScrollWheelEvent(source, SCROLL_PIXELS, SCROLL_AXES, vertical, horizontal);
        if !source.is_null() {
            CFRelease(source);
        }
        if !event.is_null() {
            CGEventSetLocation(event, CGPoint { x: at.0, y: at.1 });
        }
        gesture.post(event);
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
