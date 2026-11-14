macro_rules! crate_root {
    () => {
        mod lib {
            mod core {
                pub use std::*;
            }

            pub use self::core::{f32, f64};
            pub use self::core::{iter, num, str};

            pub use self::core::{cmp, mem};

            pub use self::core::cell::{Cell, RefCell};
            pub use self::core::cmp::Reverse;
            pub use self::core::fmt::{self, Debug, Display, Write as FmtWrite};
            pub use self::core::marker::PhantomData;
            pub use self::core::num::Wrapping;
            pub use self::core::ops::{Bound, Range, RangeFrom, RangeInclusive, RangeTo};
            pub use self::core::result;
            pub use self::core::time::Duration;

            pub use std::borrow::{Cow, ToOwned};

            pub use std::string::{String, ToString};

            pub use std::vec::Vec;

            pub use std::boxed::Box;

            #[cfg(feature = "rc")]
            pub use std::rc::{Rc, Weak as RcWeak};

            #[cfg(feature = "rc")]
            pub use std::sync::{Arc, Weak as ArcWeak};

            pub use std::collections::{BTreeMap, BTreeSet, BinaryHeap, LinkedList, VecDeque};

            pub use std::ffi::CStr;

            pub use std::ffi::CString;

            pub use std::net;

            pub use std::error;

            pub use std::collections::{HashMap, HashSet};
            #[cfg(any(unix, windows))]
            pub use std::ffi::{OsStr, OsString};
            pub use std::hash::{BuildHasher, Hash};
            pub use std::io::Write;
            pub use std::path::{Path, PathBuf};
            pub use std::sync::{Mutex, RwLock};
            pub use std::time::{SystemTime, UNIX_EPOCH};

            pub use std::sync::atomic::Ordering;
            #[cfg(target_has_atomic = "8")]
            pub use std::sync::atomic::{AtomicBool, AtomicI8, AtomicU8};
            #[cfg(target_has_atomic = "16")]
            pub use std::sync::atomic::{AtomicI16, AtomicU16};
            #[cfg(target_has_atomic = "32")]
            pub use std::sync::atomic::{AtomicI32, AtomicU32};
            #[cfg(target_has_atomic = "64")]
            pub use std::sync::atomic::{AtomicI64, AtomicU64};
            #[cfg(target_has_atomic = "ptr")]
            pub use std::sync::atomic::{AtomicIsize, AtomicUsize};

            pub use self::core::num::Saturating;
        }

        macro_rules! tri {
            ($expr:expr) => {
                match $expr {
                    Ok(val) => val,
                    Err(err) => return Err(err),
                }
            };
        }

        pub mod de;
        pub mod ser;

        mod format;

        #[doc(inline)]
        pub use crate::de::{Deserialize, Deserializer};
        #[doc(inline)]
        pub use crate::ser::{Serialize, Serializer};

        #[doc(hidden)]
        mod private;

        #[doc(hidden)]
        pub mod __private {
            #[doc(hidden)]
            pub use crate::private::doc;
            #[doc(hidden)]
            pub use core::result::Result;
        }

        #[doc(hidden)]
        pub mod __private228 {
            #[doc(hidden)]
            pub use crate::private::*;
        }

    };
}
