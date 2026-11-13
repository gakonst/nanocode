use std::{
    ffi::{CStr, c_char, c_void},
    mem,
    ptr::{self, NonNull},
    sync::OnceLock,
};

pub type Reference = *const c_void;
pub type TypeId = usize;
type Index = isize;
type Boolean = u8;
type NumberType = Index;
type StringEncoding = u32;

const CORE_FOUNDATION_PATH: &CStr =
    c"/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation";
const ENCODING_UTF8: StringEncoding = 0x0800_0100;
const NUMBER_SIGNED_64: NumberType = 4;
const NUMBER_FLOAT_64: NumberType = 6;
const FALSE: Boolean = 0;
const TRUE: Boolean = 1;
const RUN_LOOP_FINISHED: i32 = 1;

#[derive(Clone, Copy)]
struct Constant(Reference);

unsafe impl Send for Constant {}

unsafe impl Sync for Constant {}

#[repr(C)]
struct Range {
    location: Index,
    length: Index,
}

macro_rules! library {
    ($($name:ident: fn($($argument:ty),*) $(-> $result:ty)? = $symbol:literal;)*) => {
        struct Library {
            $($name: unsafe extern "C" fn($($argument),*) $(-> $result)?,)*
            null_allocator: Constant,
            boolean_true: Constant,
            array_callbacks: Constant,
            key_callbacks: Constant,
            value_callbacks: Constant,
        }

        impl Library {
            fn load() -> Result<Self, String> {
                let handle = open(CORE_FOUNDATION_PATH)?;
                let null_allocator = resolve(handle, c"kCFAllocatorNull")?;
                let boolean_true = resolve(handle, c"kCFBooleanTrue")?;
                Ok(Self {
                    $($name: {
                        let address = resolve(handle, $symbol)?;
                        unsafe {
                            mem::transmute::<*mut c_void, unsafe extern "C" fn($($argument),*) $(-> $result)?>(address)
                        }
                    },)*
                    null_allocator: Constant(unsafe { *null_allocator.cast::<Reference>() }),
                    boolean_true: Constant(unsafe { *boolean_true.cast::<Reference>() }),
                    array_callbacks: Constant(resolve(handle, c"kCFTypeArrayCallBacks")?.cast_const()),
                    key_callbacks: Constant(resolve(handle, c"kCFTypeDictionaryKeyCallBacks")?.cast_const()),
                    value_callbacks: Constant(resolve(handle, c"kCFTypeDictionaryValueCallBacks")?.cast_const()),
                })
            }
        }
    };
}

library! {
    retain: fn(Reference) -> Reference = c"CFRetain";
    release: fn(Reference) = c"CFRelease";
    equal: fn(Reference, Reference) -> Boolean = c"CFEqual";
    hash: fn(Reference) -> usize = c"CFHash";
    type_of: fn(Reference) -> TypeId = c"CFGetTypeID";
    string_type: fn() -> TypeId = c"CFStringGetTypeID";
    number_type: fn() -> TypeId = c"CFNumberGetTypeID";
    boolean_type: fn() -> TypeId = c"CFBooleanGetTypeID";
    array_type: fn() -> TypeId = c"CFArrayGetTypeID";
    dictionary_type: fn() -> TypeId = c"CFDictionaryGetTypeID";
    null_type: fn() -> TypeId = c"CFNullGetTypeID";
    string_create: fn(Reference, *const u8, Index, StringEncoding, Boolean) -> Reference = c"CFStringCreateWithBytes";
    string_create_no_copy: fn(Reference, *const u8, Index, StringEncoding, Boolean, Reference) -> Reference = c"CFStringCreateWithBytesNoCopy";
    string_pointer: fn(Reference, StringEncoding) -> *const c_char = c"CFStringGetCStringPtr";
    string_length: fn(Reference) -> Index = c"CFStringGetLength";
    string_bytes: fn(Reference, Range, StringEncoding, u8, Boolean, *mut u8, Index, *mut Index) -> Index = c"CFStringGetBytes";
    string_character: fn(Reference, Index) -> u16 = c"CFStringGetCharacterAtIndex";
    string_substring: fn(Reference, Reference, Range) -> Reference = c"CFStringCreateWithSubstring";
    number_create: fn(Reference, NumberType, *const c_void) -> Reference = c"CFNumberCreate";
    number_value: fn(Reference, NumberType, *mut c_void) -> Boolean = c"CFNumberGetValue";
    array_create: fn(Reference, *const Reference, Index, Reference) -> Reference = c"CFArrayCreate";
    array_count: fn(Reference) -> Index = c"CFArrayGetCount";
    array_value: fn(Reference, Index) -> Reference = c"CFArrayGetValueAtIndex";
    dictionary_create: fn(Reference, *const Reference, *const Reference, Index, Reference, Reference) -> Reference = c"CFDictionaryCreate";
    dictionary_find: fn(Reference, Reference, *mut Reference) -> Boolean = c"CFDictionaryGetValueIfPresent";
    dictionary_value: fn(Reference, Reference) -> Reference = c"CFDictionaryGetValue";
    data_create_mutable: fn(Reference, Index) -> Reference = c"CFDataCreateMutable";
    data_length: fn(Reference) -> Index = c"CFDataGetLength";
    data_bytes: fn(Reference) -> *const u8 = c"CFDataGetBytePtr";
    run_loop_current: fn() -> Reference = c"CFRunLoopGetCurrent";
    run_loop_add: fn(Reference, Reference, Reference) = c"CFRunLoopAddSource";
    run_loop_remove: fn(Reference, Reference, Reference) = c"CFRunLoopRemoveSource";
    run_loop_run: fn(Reference, f64, Boolean) -> i32 = c"CFRunLoopRunInMode";
}

static LIBRARY: OnceLock<Result<Library, String>> = OnceLock::new();

fn library() -> &'static Library {
    let library = LIBRARY.get_or_init(Library::load).as_ref();
    library.unwrap_or_else(|error| panic!("CoreFoundation is part of every macOS: {error}"))
}

fn dynamic_error() -> String {
    let message = unsafe { libc::dlerror() };
    if message.is_null() {
        return "no dlerror message".to_owned();
    }
    let message = unsafe { CStr::from_ptr(message) };
    message.to_string_lossy().into_owned()
}

fn open(path: &CStr) -> Result<NonNull<c_void>, String> {
    let handle = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
    NonNull::new(handle)
        .ok_or_else(|| format!("loading {}: {}", path.to_string_lossy(), dynamic_error()))
}

fn resolve(handle: NonNull<c_void>, name: &CStr) -> Result<*mut c_void, String> {
    let address = unsafe { libc::dlsym(handle.as_ptr(), name.as_ptr()) };
    if address.is_null() {
        return Err(format!(
            "resolving {}: {}",
            name.to_string_lossy(),
            dynamic_error()
        ));
    }
    Ok(address)
}

fn index(length: usize) -> Index {
    Index::try_from(length).expect("a Rust slice length fits a CFIndex")
}

pub struct Object(NonNull<c_void>);

impl Object {
    #[allow(clippy::missing_safety_doc)]
    pub unsafe fn from_create_rule(reference: Reference) -> Option<Self> {
        NonNull::new(reference.cast_mut()).map(Self)
    }

    #[allow(clippy::missing_safety_doc)]
    pub unsafe fn from_get_rule(reference: Reference) -> Option<Self> {
        let object = NonNull::new(reference.cast_mut())?;
        unsafe { (library().retain)(reference) };
        Some(Self(object))
    }

    fn created(reference: Reference, what: &str) -> Self {
        let object = NonNull::new(reference.cast_mut());
        Self(object.unwrap_or_else(|| panic!("{what} returned NULL")))
    }

    pub const fn as_ptr(&self) -> Reference {
        self.0.as_ptr().cast_const()
    }

    pub fn string(text: &str) -> Self {
        let string = unsafe {
            (library().string_create)(
                ptr::null(),
                text.as_ptr(),
                index(text.len()),
                ENCODING_UTF8,
                FALSE,
            )
        };
        Self::created(string, "CFStringCreateWithBytes")
    }

    pub fn static_string(text: &'static str) -> Self {
        Self::borrowed_string(text)
    }

    fn borrowed_string(text: &str) -> Self {
        let library = library();
        let string = unsafe {
            (library.string_create_no_copy)(
                ptr::null(),
                text.as_ptr(),
                index(text.len()),
                ENCODING_UTF8,
                FALSE,
                library.null_allocator.0,
            )
        };
        Self::created(string, "CFStringCreateWithBytesNoCopy")
    }

    pub fn number(value: f64) -> Self {
        let number = unsafe {
            (library().number_create)(ptr::null(), NUMBER_FLOAT_64, (&raw const value).cast())
        };
        Self::created(number, "CFNumberCreate")
    }

    pub fn true_value() -> Self {
        let value = unsafe { Self::from_get_rule(library().boolean_true.0) };
        value.expect("kCFBooleanTrue is a constant object")
    }

    pub fn mutable_data() -> Option<Self> {
        unsafe { Self::from_create_rule((library().data_create_mutable)(ptr::null(), 0)) }
    }

    pub fn array(items: &[&Self]) -> Self {
        let library = library();
        let values: Vec<Reference> = items.iter().map(|item| item.as_ptr()).collect();
        let array = unsafe {
            (library.array_create)(
                ptr::null(),
                values.as_ptr(),
                index(values.len()),
                library.array_callbacks.0,
            )
        };
        Self::created(array, "CFArrayCreate")
    }

    pub fn dictionary(pairs: &[(&Self, &Self)]) -> Self {
        let library = library();
        let (keys, values): (Vec<Reference>, Vec<Reference>) = pairs
            .iter()
            .map(|(key, value)| (key.as_ptr(), value.as_ptr()))
            .unzip();
        let dictionary = unsafe {
            (library.dictionary_create)(
                ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                index(keys.len()),
                library.key_callbacks.0,
                library.value_callbacks.0,
            )
        };
        Self::created(dictionary, "CFDictionaryCreate")
    }

    pub fn type_id(&self) -> TypeId {
        unsafe { (library().type_of)(self.as_ptr()) }
    }

    pub fn is_string(&self) -> bool {
        self.type_id() == unsafe { (library().string_type)() }
    }

    pub fn is_number(&self) -> bool {
        self.type_id() == unsafe { (library().number_type)() }
    }

    pub fn is_boolean(&self) -> bool {
        self.type_id() == unsafe { (library().boolean_type)() }
    }

    pub fn is_array(&self) -> bool {
        self.type_id() == unsafe { (library().array_type)() }
    }

    pub fn is_dictionary(&self) -> bool {
        self.type_id() == unsafe { (library().dictionary_type)() }
    }

    pub fn is_null(&self) -> bool {
        self.type_id() == unsafe { (library().null_type)() }
    }

    pub fn text(&self) -> Option<String> {
        self.is_string().then(|| self.string_text())
    }

    fn string_text(&self) -> String {
        let library = library();
        let pointer = unsafe { (library.string_pointer)(self.as_ptr(), ENCODING_UTF8) };
        if !pointer.is_null() {
            let text = unsafe { CStr::from_ptr(pointer) };
            return String::from_utf8(text.to_bytes().to_vec())
                .expect("CFStringGetCStringPtr with kCFStringEncodingUTF8 points at UTF-8");
        }
        let length = unsafe { (library.string_length)(self.as_ptr()) };
        let range = || Range {
            location: 0,
            length,
        };
        let mut required: Index = 0;
        unsafe {
            (library.string_bytes)(
                self.as_ptr(),
                range(),
                ENCODING_UTF8,
                0,
                FALSE,
                ptr::null_mut(),
                0,
                &mut required,
            )
        };
        let capacity =
            usize::try_from(required).expect("CFStringGetBytes never needs a negative length");
        let mut buffer = vec![0_u8; capacity];
        let mut used: Index = 0;
        let written = unsafe {
            (library.string_bytes)(
                self.as_ptr(),
                range(),
                ENCODING_UTF8,
                0,
                FALSE,
                buffer.as_mut_ptr(),
                required,
                &mut used,
            )
        };
        assert_eq!(written, length, "CFStringGetBytes converts every character");
        assert_eq!(
            used, required,
            "CFStringGetBytes fills the length it asked for"
        );
        String::from_utf8(buffer).expect("CFStringGetBytes with kCFStringEncodingUTF8 writes UTF-8")
    }

    pub fn text_equals(&self, text: &str) -> bool {
        self == &Self::borrowed_string(text)
    }

    pub fn utf16_length(&self) -> usize {
        assert!(self.is_string(), "a UTF-16 length is a CFString's");
        let length = unsafe { (library().string_length)(self.as_ptr()) };
        usize::try_from(length).expect("CFStringGetLength is never negative")
    }

    pub fn utf16_unit(&self, position: usize) -> u16 {
        assert!(
            position < self.utf16_length(),
            "a UTF-16 unit lies inside the string"
        );
        unsafe { (library().string_character)(self.as_ptr(), index(position)) }
    }

    pub fn utf16_prefix(&self, length: usize) -> Option<Self> {
        assert!(
            length <= self.utf16_length(),
            "a UTF-16 prefix lies inside the string"
        );
        let range = Range {
            location: 0,
            length: index(length),
        };
        unsafe {
            Self::from_create_rule((library().string_substring)(
                ptr::null(),
                self.as_ptr(),
                range,
            ))
        }
    }

    pub fn float(&self) -> Option<f64> {
        if !self.is_number() {
            return None;
        }
        let mut value: f64 = 0.0;
        let converted = unsafe {
            (library().number_value)(self.as_ptr(), NUMBER_FLOAT_64, (&raw mut value).cast())
        };
        (converted != FALSE).then_some(value)
    }

    pub fn integer(&self) -> Option<i64> {
        if !self.is_number() {
            return None;
        }
        let mut value: i64 = 0;
        let converted = unsafe {
            (library().number_value)(self.as_ptr(), NUMBER_SIGNED_64, (&raw mut value).cast())
        };
        (converted != FALSE).then_some(value)
    }

    pub fn boolean(&self) -> Option<bool> {
        self.is_boolean()
            .then(|| self.as_ptr() == library().boolean_true.0)
    }

    pub fn array_values(&self, limit: usize) -> Vec<Self> {
        if !self.is_array() {
            return Vec::new();
        }
        let library = library();
        let count = unsafe { (library.array_count)(self.as_ptr()) };
        let count = usize::try_from(count).expect("CFArrayGetCount is never negative");
        (0..count.min(limit))
            .filter_map(|position| unsafe {
                Self::from_get_rule((library.array_value)(self.as_ptr(), index(position)))
            })
            .collect()
    }

    pub fn dictionary_find(&self, key: &Self) -> Option<Self> {
        assert!(self.is_dictionary(), "a lookup is in a CFDictionary");
        let mut value: Reference = ptr::null();
        let present =
            unsafe { (library().dictionary_find)(self.as_ptr(), key.as_ptr(), &mut value) };
        if present == FALSE {
            return None;
        }
        let value = unsafe { Self::from_get_rule(value) };
        Some(value.expect("a CFDictionary of CFTypes holds no NULL values"))
    }

    pub fn dictionary_value(&self, key: &Self) -> Option<Self> {
        assert!(self.is_dictionary(), "a lookup is in a CFDictionary");
        unsafe { Self::from_get_rule((library().dictionary_value)(self.as_ptr(), key.as_ptr())) }
    }

    pub fn data_bytes(&self) -> Vec<u8> {
        let library = library();
        let length = unsafe { (library.data_length)(self.as_ptr()) };
        let length = usize::try_from(length).expect("CFDataGetLength is never negative");
        let bytes = unsafe { (library.data_bytes)(self.as_ptr()) };
        if bytes.is_null() {
            return Vec::new();
        }
        unsafe { std::slice::from_raw_parts(bytes, length) }.to_vec()
    }
}

impl Clone for Object {
    fn clone(&self) -> Self {
        let object = unsafe { Self::from_get_rule(self.as_ptr()) };
        object.expect("a retained object is not NULL")
    }
}

impl PartialEq for Object {
    fn eq(&self, other: &Self) -> bool {
        unsafe { (library().equal)(self.as_ptr(), other.as_ptr()) != FALSE }
    }
}

impl Eq for Object {}

impl std::hash::Hash for Object {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        unsafe { (library().hash)(self.as_ptr()) }.hash(state);
    }
}

impl Drop for Object {
    fn drop(&mut self) {
        unsafe { (library().release)(self.as_ptr()) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunOutcome {
    Finished,
    Returned,
}

pub struct RunLoop(Object);

impl RunLoop {
    pub fn current() -> Self {
        let run_loop = unsafe { Object::from_get_rule((library().run_loop_current)()) };
        Self(run_loop.expect("CFRunLoopGetCurrent always returns the thread's run loop"))
    }

    pub fn add_source(&self, source: &Object, mode: &Object) {
        unsafe { (library().run_loop_add)(self.0.as_ptr(), source.as_ptr(), mode.as_ptr()) }
    }

    pub fn remove_source(&self, source: &Object, mode: &Object) {
        unsafe { (library().run_loop_remove)(self.0.as_ptr(), source.as_ptr(), mode.as_ptr()) }
    }

    pub fn run_in_mode(mode: &Object, seconds: f64) -> RunOutcome {
        let result = unsafe { (library().run_loop_run)(mode.as_ptr(), seconds, TRUE) };
        if result == RUN_LOOP_FINISHED {
            RunOutcome::Finished
        } else {
            RunOutcome::Returned
        }
    }
}
