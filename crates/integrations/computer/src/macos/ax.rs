#![allow(unsafe_code, non_upper_case_globals)]

use crate::{Button, state::shown_units};
use nanocode_sys::base64::STANDARD;
use nanocode_sys::core_foundation::{Object, Reference, RunLoop, RunOutcome, TypeId};
use std::{
    ffi::c_void,
    hash::{Hash, Hasher},
    ptr,
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
struct CFRange {
    location: isize,
    length: isize,
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
    static kAXTrustedCheckOptionPrompt: Reference;
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: Reference) -> bool;
    fn AXValueGetValue(value: Reference, kind: u32, out: *mut c_void) -> bool;
    fn AXUIElementGetTypeID() -> TypeId;
    fn AXUIElementCreateApplication(pid: i32) -> Reference;
    fn AXUIElementCopyAttributeValue(
        element: Reference,
        attribute: Reference,
        value: *mut Reference,
    ) -> AXError;
    fn AXUIElementSetAttributeValue(
        element: Reference,
        attribute: Reference,
        value: Reference,
    ) -> AXError;
    fn AXUIElementCopyMultipleAttributeValues(
        element: Reference,
        attributes: Reference,
        options: u32,
        values: *mut Reference,
    ) -> AXError;
    fn AXValueGetTypeID() -> TypeId;
    fn AXValueGetType(value: Reference) -> u32;
    fn AXUIElementCopyActionNames(element: Reference, names: *mut Reference) -> AXError;
    fn AXUIElementPerformAction(element: Reference, action: Reference) -> AXError;
    fn AXUIElementSetMessagingTimeout(element: Reference, seconds: f32) -> AXError;
    fn AXValueCreate(kind: u32, value: *const c_void) -> Reference;
    fn AXObserverCreate(
        application: i32,
        callback: unsafe extern "C" fn(Reference, Reference, Reference, *mut c_void),
        observer: *mut Reference,
    ) -> AXError;
    fn AXObserverAddNotification(
        observer: Reference,
        element: Reference,
        notification: Reference,
        refcon: *mut c_void,
    ) -> AXError;
    fn AXObserverGetRunLoopSource(observer: Reference) -> Reference;
    fn _AXUIElementGetWindow(element: Reference, window: *mut u32) -> AXError;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventCreateKeyboardEvent(source: Reference, key: u16, down: bool) -> Reference;
    fn CGEventSetFlags(event: Reference, flags: u64);
    fn CGEventKeyboardSetUnicodeString(event: Reference, length: usize, text: *const u16);
    fn CGEventPostToPid(pid: i32, event: Reference);
    fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> Reference;
    fn CGWindowListCreateImage(
        bounds: CGRect,
        option: u32,
        window: u32,
        image_option: u32,
    ) -> Reference;
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
    fn CGEventCreateMouseEvent(source: Reference, kind: u32, at: CGPoint, button: u32)
    -> Reference;
    fn CGEventCreateScrollWheelEvent(
        source: Reference,
        units: u32,
        count: u32,
        wheel: i32,
        ...
    ) -> Reference;
    fn CGEventSetLocation(event: Reference, at: CGPoint);
    fn CGEventSourceCreate(state: i32) -> Reference;
    fn CGEventSetIntegerValueField(event: Reference, field: u32, value: i64);
    fn CGSessionCopyCurrentDictionary() -> Reference;
}

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    static kCGImageDestinationLossyCompressionQuality: Reference;
    fn CGImageDestinationCreateWithData(
        data: Reference,
        kind: Reference,
        count: usize,
        options: Reference,
    ) -> Reference;
    fn CGImageDestinationAddImage(destination: Reference, image: Reference, properties: Reference);
    fn CGImageDestinationFinalize(destination: Reference) -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFBundleGetValueForInfoDictionaryKey(bundle: Reference, key: Reference) -> Reference;
    fn CFBundleGetIdentifier(bundle: Reference) -> Reference;
    fn CFBundleCreate(allocator: Reference, url: Reference) -> Reference;
    fn CFURLCreateFromFileSystemRepresentation(
        allocator: Reference,
        path: *const u8,
        length: isize,
        directory: bool,
    ) -> Reference;
}

pub fn trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

pub fn request_trust() -> bool {
    let prompt = unsafe { Object::from_get_rule(kAXTrustedCheckOptionPrompt) }
        .expect("kAXTrustedCheckOptionPrompt is a constant string");
    let options = Object::dictionary(&[(&prompt, &Object::true_value())]);
    unsafe { AXIsProcessTrustedWithOptions(options.as_ptr()) }
}

pub fn screen_recording() -> bool {
    unsafe { CGPreflightScreenCaptureAccess() }
}

pub fn request_screen_recording() -> bool {
    unsafe { CGRequestScreenCaptureAccess() }
}

pub fn locked() -> bool {
    let Some(session) = (unsafe { Object::from_create_rule(CGSessionCopyCurrentDictionary()) })
    else {
        return false;
    };
    session
        .dictionary_find(&Object::static_string("CGSSessionScreenIsLocked"))
        .and_then(|locked| locked.boolean())
        .unwrap_or(false)
}

pub struct Element(Object);

unsafe impl Send for Element {}

impl Element {
    pub fn application(pid: i32) -> Option<Self> {
        let object = unsafe { Object::from_create_rule(AXUIElementCreateApplication(pid)) };
        object.map(Self::bounded)
    }

    fn bounded(object: Object) -> Self {
        unsafe { AXUIElementSetMessagingTimeout(object.as_ptr(), MESSAGING_TIMEOUT) };
        Self(object)
    }

    fn copy(&self, attribute: &str) -> Result<Object, AXError> {
        let name = Object::string(attribute);
        let mut value: Reference = ptr::null();
        let error =
            unsafe { AXUIElementCopyAttributeValue(self.0.as_ptr(), name.as_ptr(), &mut value) };
        if error != AX_SUCCESS {
            return Err(error);
        }
        unsafe { Object::from_create_rule(value) }.ok_or(AX_NO_VALUE)
    }

    pub fn many<const N: usize>(&self, attributes: &[&str; N]) -> [Option<Object>; N] {
        let names: Vec<Object> = attributes.iter().map(|name| Object::string(name)).collect();
        let names = Object::array(&names.iter().collect::<Vec<_>>());
        let mut values: Reference = ptr::null();
        let error = unsafe {
            AXUIElementCopyMultipleAttributeValues(self.0.as_ptr(), names.as_ptr(), 0, &mut values)
        };
        let values = (error == AX_SUCCESS)
            .then(|| unsafe { Object::from_create_rule(values) })
            .flatten();
        let Some(values) = values else {
            return std::array::from_fn(|_| None);
        };
        let mut items = values.array_values(N).into_iter();
        std::array::from_fn(|_| {
            let item = items.next()?;
            let failed = item.is_null()
                || item.type_id() == ax_value_type()
                    && unsafe { AXValueGetType(item.as_ptr()) } == AX_ERROR_VALUE;
            (!failed).then_some(item)
        })
    }

    pub fn text(&self, attribute: &str) -> Option<String> {
        text_of(&self.copy(attribute).ok()?)
    }

    pub fn numeric(&self, attribute: &str) -> bool {
        self.copy(attribute).is_ok_and(|value| value.is_number())
    }

    pub fn flag(&self, attribute: &str) -> Option<bool> {
        flag_of(&self.copy(attribute).ok()?)
    }

    pub fn element(&self, attribute: &str) -> Option<Self> {
        let value = self.copy(attribute).ok()?;
        (value.type_id() == element_type()).then(|| Self::bounded(value))
    }

    pub fn elements(&self, attribute: &str, limit: usize) -> Vec<Self> {
        self.copy(attribute)
            .map(|value| elements_of(&value, limit))
            .unwrap_or_default()
    }
}

fn element_type() -> TypeId {
    unsafe { AXUIElementGetTypeID() }
}

fn ax_value_type() -> TypeId {
    unsafe { AXValueGetTypeID() }
}

fn string_of(text: &Object) -> String {
    text.text().expect("a CFString reads as text")
}

pub fn text_of(value: &Object) -> Option<String> {
    read(value, string_of)
}

pub fn label_of(value: &Object) -> Option<String> {
    read(value, shown)
}

fn shown(text: &Object) -> String {
    let len = text.utf16_length();
    let end = shown_units(len, |index| text.utf16_unit(index));
    if end == len {
        return string_of(text);
    }
    match text.utf16_prefix(end) {
        Some(prefix) => string_of(&prefix),
        None => String::new(),
    }
}

fn read(value: &Object, string: impl Fn(&Object) -> String) -> Option<String> {
    let text = if value.is_string() {
        string(value)
    } else if value.is_number() {
        let number = value.float()?;
        if number.fract() == 0.0 {
            format!("{number:.0}")
        } else {
            number.to_string()
        }
    } else if value.boolean()? {
        "1".into()
    } else {
        "0".into()
    };
    (!text.is_empty()).then_some(text)
}

pub fn flag_of(value: &Object) -> Option<bool> {
    value.boolean().or_else(|| value.integer().map(|n| n != 0))
}

pub fn elements_of(value: &Object, limit: usize) -> Vec<Element> {
    value
        .array_values(limit)
        .into_iter()
        .filter(|item| item.type_id() == element_type())
        .map(Element::bounded)
        .collect()
}

impl Element {
    pub fn actions(&self) -> Vec<String> {
        let mut names: Reference = ptr::null();
        if unsafe { AXUIElementCopyActionNames(self.0.as_ptr(), &mut names) } != AX_SUCCESS {
            return Vec::new();
        }
        let Some(names) = (unsafe { Object::from_create_rule(names) }) else {
            return Vec::new();
        };
        names
            .array_values(64)
            .iter()
            .map(|item| {
                item.text()
                    .expect("AXUIElementCopyActionNames returns CFStrings")
            })
            .collect()
    }

    pub fn perform(&self, action: &str) -> AXError {
        let name = Object::string(action);
        unsafe {
            AXUIElementSetMessagingTimeout(self.0.as_ptr(), ACTION_TIMEOUT);
            let error = AXUIElementPerformAction(self.0.as_ptr(), name.as_ptr());
            AXUIElementSetMessagingTimeout(self.0.as_ptr(), MESSAGING_TIMEOUT);
            error
        }
    }

    fn set(&self, attribute: &str, value: &Object) -> AXError {
        let name = Object::string(attribute);
        unsafe {
            AXUIElementSetMessagingTimeout(self.0.as_ptr(), ACTION_TIMEOUT);
            let error =
                AXUIElementSetAttributeValue(self.0.as_ptr(), name.as_ptr(), value.as_ptr());
            AXUIElementSetMessagingTimeout(self.0.as_ptr(), MESSAGING_TIMEOUT);
            error
        }
    }

    pub fn set_true(&self, attribute: &str) -> AXError {
        self.set(attribute, &Object::true_value())
    }

    pub fn set_number(&self, attribute: &str, number: f64) -> AXError {
        self.set(attribute, &Object::number(number))
    }

    pub fn set_text(&self, attribute: &str, text: &str) -> AXError {
        self.set(attribute, &Object::string(text))
    }

    pub fn set_range(&self, attribute: &str, location: usize, length: usize) -> AXError {
        let range = CFRange {
            location: location as isize,
            length: length as isize,
        };
        let value =
            unsafe { Object::from_create_rule(AXValueCreate(AX_RANGE, (&raw const range).cast())) }
                .expect("AXValueCreate makes a range value");
        self.set(attribute, &value)
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
        (unsafe { _AXUIElementGetWindow(self.0.as_ptr(), &mut id) } == AX_SUCCESS && id != 0)
            .then_some(id)
    }
}

fn geometry<T: Default>(value: &Object, kind: u32) -> Option<T> {
    let mut out = T::default();
    let read = value.type_id() == ax_value_type()
        && unsafe { AXValueGetValue(value.as_ptr(), kind, (&raw mut out).cast()) };
    read.then_some(out)
}

impl Clone for Element {
    fn clone(&self) -> Self {
        Self::bounded(self.0.clone())
    }
}

impl PartialEq for Element {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for Element {}

impl Hash for Element {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

pub struct Observer {
    observer: Object,
    run_loop: RunLoop,
    seen: Box<AtomicUsize>,
    handled: usize,
}

unsafe impl Send for Observer {}

unsafe extern "C" fn noticed(_: Reference, _: Reference, _: Reference, seen: *mut c_void) {
    unsafe { &*seen.cast::<AtomicUsize>() }.fetch_add(1, Ordering::Relaxed);
}

impl Observer {
    pub fn new(pid: i32) -> Option<Self> {
        let mut observer: Reference = ptr::null();
        if unsafe { AXObserverCreate(pid, noticed, &mut observer) } != AX_SUCCESS {
            return None;
        }
        let observer = unsafe { Object::from_create_rule(observer) }?;
        let seen = Box::new(AtomicUsize::new(0));
        let app = Element::application(pid);
        let added = app.as_ref().is_some_and(|app| {
            CHANGES.iter().fold(false, |added, change| {
                let name = Object::string(change);
                let error = unsafe {
                    AXObserverAddNotification(
                        observer.as_ptr(),
                        app.0.as_ptr(),
                        name.as_ptr(),
                        (&raw const *seen).cast_mut().cast(),
                    )
                };
                added | (error == AX_SUCCESS)
            })
        });
        if !added {
            return None;
        }
        let run_loop = RunLoop::current();
        let mode = Object::static_string(SETTLING);
        run_loop.add_source(&run_loop_source(&observer), &mode);
        Some(Self {
            observer,
            run_loop,
            seen,
            handled: 0,
        })
    }

    pub fn settle(&mut self, first: Duration, quiet: Duration) -> bool {
        let mode = Object::static_string(SETTLING);
        let start = Instant::now();
        let cap = start + first + quiet;
        let mut until = start + first;
        let mut changed = false;
        loop {
            let wait = until.saturating_duration_since(Instant::now());
            if RunLoop::run_in_mode(&mode, wait.as_secs_f64()) == RunOutcome::Finished {
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

fn run_loop_source(observer: &Object) -> Object {
    unsafe { Object::from_get_rule(AXObserverGetRunLoopSource(observer.as_ptr())) }
        .expect("an AXObserver has a run loop source")
}

impl Drop for Observer {
    fn drop(&mut self) {
        let mode = Object::static_string(SETTLING);
        self.run_loop
            .remove_source(&run_loop_source(&self.observer), &mode);
    }
}

pub fn post_key(pid: i32, code: u16, flags: u64, down: bool, text: Option<&[u16]>) {
    let event =
        unsafe { Object::from_create_rule(CGEventCreateKeyboardEvent(ptr::null(), code, down)) };
    let Some(event) = event else {
        return;
    };
    unsafe {
        CGEventSetFlags(event.as_ptr(), flags);
        if let Some(text) = text {
            CGEventKeyboardSetUnicodeString(event.as_ptr(), text.len(), text.as_ptr());
        }
        CGEventPostToPid(pid, event.as_ptr());
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

    fn stamp(&self, event: &Object) {
        unsafe {
            CGEventSetIntegerValueField(event.as_ptr(), FIELD_TARGET_PID, self.pid.into());
            if let Some(window) = self.window {
                CGEventSetIntegerValueField(event.as_ptr(), FIELD_WINDOW_NUMBER, window.into());
                CGEventSetIntegerValueField(event.as_ptr(), FIELD_CLICK_GROUP, self.group);
                CGEventSetIntegerValueField(
                    event.as_ptr(),
                    FIELD_WINDOW_UNDER_POINTER,
                    window.into(),
                );
                CGEventSetIntegerValueField(
                    event.as_ptr(),
                    FIELD_WINDOW_THAT_CAN_HANDLE,
                    window.into(),
                );
            }
        }
    }

    fn post(&self, event: Option<Object>) {
        let Some(event) = event else {
            return;
        };
        self.stamp(&event);
        unsafe { CGEventPostToPid(self.pid, event.as_ptr()) };
    }

    fn mouse(&self, kind: u32, at: (f64, f64), button: u32, clicks: u32) {
        let source = unsafe { Object::from_create_rule(CGEventSourceCreate(HID_SYSTEM_STATE)) };
        let source_pointer = source.as_ref().map_or(ptr::null(), Object::as_ptr);
        let event = unsafe {
            Object::from_create_rule(CGEventCreateMouseEvent(
                source_pointer,
                kind,
                CGPoint { x: at.0, y: at.1 },
                button,
            ))
        };
        drop(source);
        if let Some(event) = &event {
            unsafe {
                CGEventSetIntegerValueField(event.as_ptr(), FIELD_CLICK_STATE, clicks.into());
                CGEventSetIntegerValueField(event.as_ptr(), FIELD_BUTTON_NUMBER, button.into());
                CGEventSetIntegerValueField(event.as_ptr(), FIELD_SUBTYPE, SUBTYPE_TOUCH);
            }
        }
        self.post(event);
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
    let source = unsafe { Object::from_create_rule(CGEventSourceCreate(HID_SYSTEM_STATE)) };
    let source_pointer = source.as_ref().map_or(ptr::null(), Object::as_ptr);
    let event = unsafe {
        Object::from_create_rule(CGEventCreateScrollWheelEvent(
            source_pointer,
            SCROLL_PIXELS,
            SCROLL_AXES,
            vertical,
            horizontal,
        ))
    };
    drop(source);
    if let Some(event) = &event {
        unsafe { CGEventSetLocation(event.as_ptr(), CGPoint { x: at.0, y: at.1 }) };
    }
    gesture.post(event);
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
        Object::from_create_rule(CGWindowListCreateImage(
            null,
            INCLUDING_WINDOW,
            id,
            BOUNDS_IGNORE_FRAMING | NOMINAL_RESOLUTION,
        ))
    };
    let image = image.ok_or("the window could not be captured")?;
    Ok([0.8, 0.5, 0.3]
        .into_iter()
        .filter_map(|quality| jpeg(&image, quality))
        .map(|jpeg| STANDARD.encode(jpeg))
        .find(|data| data.len() <= max))
}

fn jpeg(image: &Object, quality: f64) -> Option<Vec<u8>> {
    let data = Object::mutable_data()?;
    let kind = Object::string("public.jpeg");
    let destination = unsafe {
        Object::from_create_rule(CGImageDestinationCreateWithData(
            data.as_ptr(),
            kind.as_ptr(),
            1,
            ptr::null(),
        ))
    }?;
    let key = unsafe { Object::from_get_rule(kCGImageDestinationLossyCompressionQuality) }
        .expect("kCGImageDestinationLossyCompressionQuality is a constant string");
    let options = Object::dictionary(&[(&key, &Object::number(quality))]);
    unsafe { CGImageDestinationAddImage(destination.as_ptr(), image.as_ptr(), options.as_ptr()) };
    if !unsafe { CGImageDestinationFinalize(destination.as_ptr()) } {
        return None;
    }
    Some(data.data_bytes())
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
    let url = unsafe {
        Object::from_create_rule(CFURLCreateFromFileSystemRepresentation(
            ptr::null(),
            path.as_ptr(),
            path.len() as isize,
            true,
        ))
    };
    let Some(url) = url else {
        return (String::new(), stem, false);
    };
    let Some(bundle) =
        (unsafe { Object::from_create_rule(CFBundleCreate(ptr::null(), url.as_ptr())) })
    else {
        return (String::new(), stem, false);
    };
    let value = |object: Reference| unsafe { Object::from_get_rule(object) };
    let string = |object: Reference| value(object).and_then(|value| value.text());
    let key = |name: &str| {
        let name = Object::string(name);
        unsafe { CFBundleGetValueForInfoDictionaryKey(bundle.as_ptr(), name.as_ptr()) }
    };
    let truthy = |object: Reference| {
        value(object).is_some_and(|value| {
            value
                .boolean()
                .or_else(|| value.is_string().then(|| value.text_equals("1")))
                .or_else(|| value.integer().map(|n| n != 0))
                .unwrap_or(false)
        })
    };
    let id = string(unsafe { CFBundleGetIdentifier(bundle.as_ptr()) }).unwrap_or_default();
    let name = string(key("CFBundleDisplayName"))
        .or_else(|| string(key("CFBundleName")))
        .unwrap_or(stem);
    let background = truthy(key("LSUIElement")) || truthy(key("LSBackgroundOnly"));
    (id, name, background)
}

pub fn on_screen() -> Vec<i32> {
    const ON_SCREEN_ONLY: u32 = 1 << 0;
    const EXCLUDE_DESKTOP: u32 = 1 << 4;
    let windows = unsafe {
        Object::from_create_rule(CGWindowListCopyWindowInfo(
            ON_SCREEN_ONLY | EXCLUDE_DESKTOP,
            0,
        ))
    };
    let Some(windows) = windows else {
        return Vec::new();
    };
    let number = |window: &Object, key: &str| {
        let key = Object::string(key);
        window
            .dictionary_value(&key)
            .and_then(|value| value.integer())
    };
    windows
        .array_values(4096)
        .into_iter()
        .filter(|window| number(window, "kCGWindowLayer") == Some(0))
        .filter_map(|window| number(&window, "kCGWindowOwnerPID").map(|pid| pid as i32))
        .collect()
}
