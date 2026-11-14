#[doc(hidden)]
#[macro_export]
macro_rules! __trace_names {
    (@ [$($out:expr),*]) => {
        &[$($out),*]
    };
    (@ [$($out:expr),*] , $($rest:tt)*) => {
        $crate::__trace_names!(@ [$($out),*] $($rest)*)
    };
    (@ [$($out:expr),*] $($k:ident).+ = ?$val:expr, $($rest:tt)*) => {
        $crate::__trace_names!(@ [$($out,)* ::core::stringify!($($k).+)] $($rest)*)
    };
    (@ [$($out:expr),*] $($k:ident).+ = ?$val:expr) => {
        $crate::__trace_names!(@ [$($out,)* ::core::stringify!($($k).+)])
    };
    (@ [$($out:expr),*] $($k:ident).+ = %$val:expr, $($rest:tt)*) => {
        $crate::__trace_names!(@ [$($out,)* ::core::stringify!($($k).+)] $($rest)*)
    };
    (@ [$($out:expr),*] $($k:ident).+ = %$val:expr) => {
        $crate::__trace_names!(@ [$($out,)* ::core::stringify!($($k).+)])
    };
    (@ [$($out:expr),*] $($k:ident).+ = $val:expr, $($rest:tt)*) => {
        $crate::__trace_names!(@ [$($out,)* ::core::stringify!($($k).+)] $($rest)*)
    };
    (@ [$($out:expr),*] $($k:ident).+ = $val:expr) => {
        $crate::__trace_names!(@ [$($out,)* ::core::stringify!($($k).+)])
    };
    (@ [$($out:expr),*] $($k:ident).+, $($rest:tt)*) => {
        $crate::__trace_names!(@ [$($out,)* ::core::stringify!($($k).+)] $($rest)*)
    };
    (@ [$($out:expr),*] $($k:ident).+) => {
        $crate::__trace_names!(@ [$($out,)* ::core::stringify!($($k).+)])
    };
    (@ [$($out:expr),*] ?$($k:ident).+, $($rest:tt)*) => {
        $crate::__trace_names!(@ [$($out,)* ::core::stringify!($($k).+)] $($rest)*)
    };
    (@ [$($out:expr),*] ?$($k:ident).+) => {
        $crate::__trace_names!(@ [$($out,)* ::core::stringify!($($k).+)])
    };
    (@ [$($out:expr),*] %$($k:ident).+, $($rest:tt)*) => {
        $crate::__trace_names!(@ [$($out,)* ::core::stringify!($($k).+)] $($rest)*)
    };
    (@ [$($out:expr),*] %$($k:ident).+) => {
        $crate::__trace_names!(@ [$($out,)* ::core::stringify!($($k).+)])
    };
    (@ [$($out:expr),*] $k:literal = ?$val:expr, $($rest:tt)*) => {
        $crate::__trace_names!(@ [$($out,)* $k] $($rest)*)
    };
    (@ [$($out:expr),*] $k:literal = ?$val:expr) => {
        $crate::__trace_names!(@ [$($out,)* $k])
    };
    (@ [$($out:expr),*] $k:literal = %$val:expr, $($rest:tt)*) => {
        $crate::__trace_names!(@ [$($out,)* $k] $($rest)*)
    };
    (@ [$($out:expr),*] $k:literal = %$val:expr) => {
        $crate::__trace_names!(@ [$($out,)* $k])
    };
    (@ [$($out:expr),*] $k:literal = $val:expr, $($rest:tt)*) => {
        $crate::__trace_names!(@ [$($out,)* $k] $($rest)*)
    };
    (@ [$($out:expr),*] $k:literal = $val:expr) => {
        $crate::__trace_names!(@ [$($out,)* $k])
    };
    (@ [$($out:expr),*] { $k:expr } = ?$val:expr, $($rest:tt)*) => {
        $crate::__trace_names!(@ [$($out,)* $k] $($rest)*)
    };
    (@ [$($out:expr),*] { $k:expr } = ?$val:expr) => {
        $crate::__trace_names!(@ [$($out,)* $k])
    };
    (@ [$($out:expr),*] { $k:expr } = %$val:expr, $($rest:tt)*) => {
        $crate::__trace_names!(@ [$($out,)* $k] $($rest)*)
    };
    (@ [$($out:expr),*] { $k:expr } = %$val:expr) => {
        $crate::__trace_names!(@ [$($out,)* $k])
    };
    (@ [$($out:expr),*] { $k:expr } = $val:expr, $($rest:tt)*) => {
        $crate::__trace_names!(@ [$($out,)* $k] $($rest)*)
    };
    (@ [$($out:expr),*] { $k:expr } = $val:expr) => {
        $crate::__trace_names!(@ [$($out,)* $k])
    };
    (@ [$($out:expr),*] $($rest:tt)+) => {
        &["message" $(, $out)*]
    };
    ($($fields:tt)*) => {
        $crate::__trace_names!(@ [] $($fields)*)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __trace_values {
    (@ [$($out:expr),*]) => {
        &[$($out),*]
    };
    (@ [$($out:expr),*] , $($rest:tt)*) => {
        $crate::__trace_values!(@ [$($out),*] $($rest)*)
    };
    (@ [$($out:expr),*] $($k:ident).+ = ?$val:expr, $($rest:tt)*) => {
        $crate::__trace_values!(@ [$($out,)* ::core::option::Option::Some(&$crate::field::debug(&$val) as &dyn $crate::field::Value)] $($rest)*)
    };
    (@ [$($out:expr),*] $($k:ident).+ = ?$val:expr) => {
        $crate::__trace_values!(@ [$($out,)* ::core::option::Option::Some(&$crate::field::debug(&$val) as &dyn $crate::field::Value)])
    };
    (@ [$($out:expr),*] $($k:ident).+ = %$val:expr, $($rest:tt)*) => {
        $crate::__trace_values!(@ [$($out,)* ::core::option::Option::Some(&$crate::field::display(&$val) as &dyn $crate::field::Value)] $($rest)*)
    };
    (@ [$($out:expr),*] $($k:ident).+ = %$val:expr) => {
        $crate::__trace_values!(@ [$($out,)* ::core::option::Option::Some(&$crate::field::display(&$val) as &dyn $crate::field::Value)])
    };
    (@ [$($out:expr),*] $($k:ident).+ = $val:expr, $($rest:tt)*) => {
        $crate::__trace_values!(@ [$($out,)* ::core::option::Option::Some(&$val as &dyn $crate::field::Value)] $($rest)*)
    };
    (@ [$($out:expr),*] $($k:ident).+ = $val:expr) => {
        $crate::__trace_values!(@ [$($out,)* ::core::option::Option::Some(&$val as &dyn $crate::field::Value)])
    };
    (@ [$($out:expr),*] $($k:ident).+, $($rest:tt)*) => {
        $crate::__trace_values!(@ [$($out,)* ::core::option::Option::Some(&$($k).+ as &dyn $crate::field::Value)] $($rest)*)
    };
    (@ [$($out:expr),*] $($k:ident).+) => {
        $crate::__trace_values!(@ [$($out,)* ::core::option::Option::Some(&$($k).+ as &dyn $crate::field::Value)])
    };
    (@ [$($out:expr),*] ?$($k:ident).+, $($rest:tt)*) => {
        $crate::__trace_values!(@ [$($out,)* ::core::option::Option::Some(&$crate::field::debug(&$($k).+) as &dyn $crate::field::Value)] $($rest)*)
    };
    (@ [$($out:expr),*] ?$($k:ident).+) => {
        $crate::__trace_values!(@ [$($out,)* ::core::option::Option::Some(&$crate::field::debug(&$($k).+) as &dyn $crate::field::Value)])
    };
    (@ [$($out:expr),*] %$($k:ident).+, $($rest:tt)*) => {
        $crate::__trace_values!(@ [$($out,)* ::core::option::Option::Some(&$crate::field::display(&$($k).+) as &dyn $crate::field::Value)] $($rest)*)
    };
    (@ [$($out:expr),*] %$($k:ident).+) => {
        $crate::__trace_values!(@ [$($out,)* ::core::option::Option::Some(&$crate::field::display(&$($k).+) as &dyn $crate::field::Value)])
    };
    (@ [$($out:expr),*] $k:literal = ?$val:expr, $($rest:tt)*) => {
        $crate::__trace_values!(@ [$($out,)* ::core::option::Option::Some(&$crate::field::debug(&$val) as &dyn $crate::field::Value)] $($rest)*)
    };
    (@ [$($out:expr),*] $k:literal = ?$val:expr) => {
        $crate::__trace_values!(@ [$($out,)* ::core::option::Option::Some(&$crate::field::debug(&$val) as &dyn $crate::field::Value)])
    };
    (@ [$($out:expr),*] $k:literal = %$val:expr, $($rest:tt)*) => {
        $crate::__trace_values!(@ [$($out,)* ::core::option::Option::Some(&$crate::field::display(&$val) as &dyn $crate::field::Value)] $($rest)*)
    };
    (@ [$($out:expr),*] $k:literal = %$val:expr) => {
        $crate::__trace_values!(@ [$($out,)* ::core::option::Option::Some(&$crate::field::display(&$val) as &dyn $crate::field::Value)])
    };
    (@ [$($out:expr),*] $k:literal = $val:expr, $($rest:tt)*) => {
        $crate::__trace_values!(@ [$($out,)* ::core::option::Option::Some(&$val as &dyn $crate::field::Value)] $($rest)*)
    };
    (@ [$($out:expr),*] $k:literal = $val:expr) => {
        $crate::__trace_values!(@ [$($out,)* ::core::option::Option::Some(&$val as &dyn $crate::field::Value)])
    };
    (@ [$($out:expr),*] { $k:expr } = ?$val:expr, $($rest:tt)*) => {
        $crate::__trace_values!(@ [$($out,)* ::core::option::Option::Some(&$crate::field::debug(&$val) as &dyn $crate::field::Value)] $($rest)*)
    };
    (@ [$($out:expr),*] { $k:expr } = ?$val:expr) => {
        $crate::__trace_values!(@ [$($out,)* ::core::option::Option::Some(&$crate::field::debug(&$val) as &dyn $crate::field::Value)])
    };
    (@ [$($out:expr),*] { $k:expr } = %$val:expr, $($rest:tt)*) => {
        $crate::__trace_values!(@ [$($out,)* ::core::option::Option::Some(&$crate::field::display(&$val) as &dyn $crate::field::Value)] $($rest)*)
    };
    (@ [$($out:expr),*] { $k:expr } = %$val:expr) => {
        $crate::__trace_values!(@ [$($out,)* ::core::option::Option::Some(&$crate::field::display(&$val) as &dyn $crate::field::Value)])
    };
    (@ [$($out:expr),*] { $k:expr } = $val:expr, $($rest:tt)*) => {
        $crate::__trace_values!(@ [$($out,)* ::core::option::Option::Some(&$val as &dyn $crate::field::Value)] $($rest)*)
    };
    (@ [$($out:expr),*] { $k:expr } = $val:expr) => {
        $crate::__trace_values!(@ [$($out,)* ::core::option::Option::Some(&$val as &dyn $crate::field::Value)])
    };
    (@ [$($out:expr),*] $($rest:tt)+) => {
        &[::core::option::Option::Some(&::core::format_args!($($rest)+) as &dyn $crate::field::Value) $(, $out)*]
    };
    ($fields:expr, $($values:tt)*) => {
        $fields.value_set_all($crate::__trace_values!(@ [] $($values)*))
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __trace_callsite {
    (name: $name:expr, kind: $kind:expr, target: $target:expr, level: $level:expr, fields: $($fields:tt)*) => {{
        static METADATA: $crate::Metadata<'static> = $crate::Metadata::new(
            $name,
            $target,
            $level,
            ::core::option::Option::Some(::core::file!()),
            ::core::option::Option::Some(::core::line!()),
            ::core::option::Option::Some(::core::module_path!()),
            $crate::field::FieldSet::new(
                $crate::__trace_names!($($fields)*),
                $crate::callsite::Identifier::new(&__CALLSITE),
            ),
            $kind,
        );
        $crate::callsite::DefaultCallsite::new(&METADATA)
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __trace_enabled {
    ($level:expr) => {
        $level <= $crate::LevelFilter::current() && {
            let interest = __CALLSITE.interest();
            !interest.is_never() && $crate::__private::is_enabled(__CALLSITE.metadata(), &interest)
        }
    };
}

#[macro_export]
macro_rules! span {
    (target: $target:expr, parent: $parent:expr, $level:expr, $name:expr) => {
        $crate::span!(target: $target, parent: $parent, $level, $name,)
    };
    (target: $target:expr, parent: $parent:expr, $level:expr, $name:expr, $($fields:tt)*) => {{
        #[allow(unused_imports)]
        use $crate::field::{Value, debug, display};
        static __CALLSITE: $crate::callsite::DefaultCallsite = $crate::__trace_callsite! {
            name: $name,
            kind: $crate::metadata::Kind::SPAN,
            target: $target,
            level: $level,
            fields: $($fields)*
        };
        if $crate::__trace_enabled!($level) {
            let metadata = __CALLSITE.metadata();
            $crate::Span::child_of($parent, metadata, &$crate::__trace_values!(metadata.fields(), $($fields)*))
        } else {
            $crate::Span::none()
        }
    }};
    (target: $target:expr, $level:expr, $name:expr) => {
        $crate::span!(target: $target, $level, $name,)
    };
    (target: $target:expr, $level:expr, $name:expr, $($fields:tt)*) => {{
        #[allow(unused_imports)]
        use $crate::field::{Value, debug, display};
        static __CALLSITE: $crate::callsite::DefaultCallsite = $crate::__trace_callsite! {
            name: $name,
            kind: $crate::metadata::Kind::SPAN,
            target: $target,
            level: $level,
            fields: $($fields)*
        };
        if $crate::__trace_enabled!($level) {
            let metadata = __CALLSITE.metadata();
            $crate::Span::new(metadata, &$crate::__trace_values!(metadata.fields(), $($fields)*))
        } else {
            $crate::Span::none()
        }
    }};
    (parent: $parent:expr, $level:expr, $name:expr) => {
        $crate::span!(target: ::core::module_path!(), parent: $parent, $level, $name,)
    };
    (parent: $parent:expr, $level:expr, $name:expr, $($fields:tt)*) => {
        $crate::span!(target: ::core::module_path!(), parent: $parent, $level, $name, $($fields)*)
    };
    ($level:expr, $name:expr) => {
        $crate::span!(target: ::core::module_path!(), $level, $name,)
    };
    ($level:expr, $name:expr, $($fields:tt)*) => {
        $crate::span!(target: ::core::module_path!(), $level, $name, $($fields)*)
    };
}

#[macro_export]
macro_rules! event {
    (target: $target:expr, parent: $parent:expr, $level:expr, { $($fields:tt)* } $(,)?) => {{
        #[allow(unused_imports)]
        use $crate::field::{Value, debug, display};
        static __CALLSITE: $crate::callsite::DefaultCallsite = $crate::__trace_callsite! {
            name: ::core::concat!("event ", ::core::file!(), ":", ::core::line!()),
            kind: $crate::metadata::Kind::EVENT,
            target: $target,
            level: $level,
            fields: $($fields)*
        };
        if $crate::__trace_enabled!($level) {
            $crate::Event::child_of(
                $parent,
                __CALLSITE.metadata(),
                &$crate::__trace_values!(__CALLSITE.metadata().fields(), $($fields)*),
            );
        }
    }};
    (target: $target:expr, parent: $parent:expr, $level:expr, { $($fields:tt)* }, $($arguments:tt)+) => {
        $crate::event!(
            target: $target,
            parent: $parent,
            $level,
            { message = ::core::format_args!($($arguments)+), $($fields)* }
        )
    };
    (target: $target:expr, parent: $parent:expr, $level:expr, $($arguments:tt)+) => {
        $crate::event!(target: $target, parent: $parent, $level, { $($arguments)+ })
    };
    (target: $target:expr, $level:expr, { $($fields:tt)* } $(,)?) => {{
        #[allow(unused_imports)]
        use $crate::field::{Value, debug, display};
        static __CALLSITE: $crate::callsite::DefaultCallsite = $crate::__trace_callsite! {
            name: ::core::concat!("event ", ::core::file!(), ":", ::core::line!()),
            kind: $crate::metadata::Kind::EVENT,
            target: $target,
            level: $level,
            fields: $($fields)*
        };
        if $crate::__trace_enabled!($level) {
            $crate::Event::dispatch(
                __CALLSITE.metadata(),
                &$crate::__trace_values!(__CALLSITE.metadata().fields(), $($fields)*),
            );
        }
    }};
    (target: $target:expr, $level:expr, { $($fields:tt)* }, $($arguments:tt)+) => {
        $crate::event!(
            target: $target,
            $level,
            { message = ::core::format_args!($($arguments)+), $($fields)* }
        )
    };
    (target: $target:expr, $level:expr, $($arguments:tt)+) => {
        $crate::event!(target: $target, $level, { $($arguments)+ })
    };
    (parent: $parent:expr, $level:expr, $($arguments:tt)+) => {
        $crate::event!(target: ::core::module_path!(), parent: $parent, $level, $($arguments)+)
    };
    ($level:expr, $($arguments:tt)+) => {
        $crate::event!(target: ::core::module_path!(), $level, $($arguments)+)
    };
}

#[macro_export]
macro_rules! enabled {
    (target: $target:expr, $level:expr) => {{
        if $level <= $crate::LevelFilter::current() {
            static __CALLSITE: $crate::callsite::DefaultCallsite = $crate::__trace_callsite! {
                name: ::core::concat!("enabled ", ::core::file!(), ":", ::core::line!()),
                kind: $crate::metadata::Kind::HINT,
                target: $target,
                level: $level,
                fields:
            };
            let interest = __CALLSITE.interest();
            if !interest.is_never() && $crate::__private::is_enabled(__CALLSITE.metadata(), &interest) {
                let metadata = __CALLSITE.metadata();
                $crate::dispatcher::get_default(|current| current.enabled(metadata))
            } else {
                false
            }
        } else {
            false
        }
    }};
    ($level:expr) => {
        $crate::enabled!(target: ::core::module_path!(), $level)
    };
}

#[macro_export]
macro_rules! trace {
    (target: $target:expr, parent: $parent:expr, $($arguments:tt)+) => {
        $crate::event!(target: $target, parent: $parent, $crate::Level::TRACE, $($arguments)+)
    };
    (target: $target:expr, $($arguments:tt)+) => {
        $crate::event!(target: $target, $crate::Level::TRACE, $($arguments)+)
    };
    (parent: $parent:expr, $($arguments:tt)+) => {
        $crate::event!(target: ::core::module_path!(), parent: $parent, $crate::Level::TRACE, $($arguments)+)
    };
    ($($arguments:tt)+) => {
        $crate::event!(target: ::core::module_path!(), $crate::Level::TRACE, $($arguments)+)
    };
}

#[macro_export]
macro_rules! trace_span {
    (target: $target:expr, parent: $parent:expr, $($arguments:tt)+) => {
        $crate::span!(target: $target, parent: $parent, $crate::Level::TRACE, $($arguments)+)
    };
    (target: $target:expr, $($arguments:tt)+) => {
        $crate::span!(target: $target, $crate::Level::TRACE, $($arguments)+)
    };
    (parent: $parent:expr, $($arguments:tt)+) => {
        $crate::span!(target: ::core::module_path!(), parent: $parent, $crate::Level::TRACE, $($arguments)+)
    };
    ($($arguments:tt)+) => {
        $crate::span!(target: ::core::module_path!(), $crate::Level::TRACE, $($arguments)+)
    };
}

#[macro_export]
macro_rules! debug {
    (target: $target:expr, parent: $parent:expr, $($arguments:tt)+) => {
        $crate::event!(target: $target, parent: $parent, $crate::Level::DEBUG, $($arguments)+)
    };
    (target: $target:expr, $($arguments:tt)+) => {
        $crate::event!(target: $target, $crate::Level::DEBUG, $($arguments)+)
    };
    (parent: $parent:expr, $($arguments:tt)+) => {
        $crate::event!(target: ::core::module_path!(), parent: $parent, $crate::Level::DEBUG, $($arguments)+)
    };
    ($($arguments:tt)+) => {
        $crate::event!(target: ::core::module_path!(), $crate::Level::DEBUG, $($arguments)+)
    };
}

#[macro_export]
macro_rules! debug_span {
    (target: $target:expr, parent: $parent:expr, $($arguments:tt)+) => {
        $crate::span!(target: $target, parent: $parent, $crate::Level::DEBUG, $($arguments)+)
    };
    (target: $target:expr, $($arguments:tt)+) => {
        $crate::span!(target: $target, $crate::Level::DEBUG, $($arguments)+)
    };
    (parent: $parent:expr, $($arguments:tt)+) => {
        $crate::span!(target: ::core::module_path!(), parent: $parent, $crate::Level::DEBUG, $($arguments)+)
    };
    ($($arguments:tt)+) => {
        $crate::span!(target: ::core::module_path!(), $crate::Level::DEBUG, $($arguments)+)
    };
}

#[macro_export]
macro_rules! info {
    (target: $target:expr, parent: $parent:expr, $($arguments:tt)+) => {
        $crate::event!(target: $target, parent: $parent, $crate::Level::INFO, $($arguments)+)
    };
    (target: $target:expr, $($arguments:tt)+) => {
        $crate::event!(target: $target, $crate::Level::INFO, $($arguments)+)
    };
    (parent: $parent:expr, $($arguments:tt)+) => {
        $crate::event!(target: ::core::module_path!(), parent: $parent, $crate::Level::INFO, $($arguments)+)
    };
    ($($arguments:tt)+) => {
        $crate::event!(target: ::core::module_path!(), $crate::Level::INFO, $($arguments)+)
    };
}

#[macro_export]
macro_rules! info_span {
    (target: $target:expr, parent: $parent:expr, $($arguments:tt)+) => {
        $crate::span!(target: $target, parent: $parent, $crate::Level::INFO, $($arguments)+)
    };
    (target: $target:expr, $($arguments:tt)+) => {
        $crate::span!(target: $target, $crate::Level::INFO, $($arguments)+)
    };
    (parent: $parent:expr, $($arguments:tt)+) => {
        $crate::span!(target: ::core::module_path!(), parent: $parent, $crate::Level::INFO, $($arguments)+)
    };
    ($($arguments:tt)+) => {
        $crate::span!(target: ::core::module_path!(), $crate::Level::INFO, $($arguments)+)
    };
}

#[macro_export]
macro_rules! warn {
    (target: $target:expr, parent: $parent:expr, $($arguments:tt)+) => {
        $crate::event!(target: $target, parent: $parent, $crate::Level::WARN, $($arguments)+)
    };
    (target: $target:expr, $($arguments:tt)+) => {
        $crate::event!(target: $target, $crate::Level::WARN, $($arguments)+)
    };
    (parent: $parent:expr, $($arguments:tt)+) => {
        $crate::event!(target: ::core::module_path!(), parent: $parent, $crate::Level::WARN, $($arguments)+)
    };
    ($($arguments:tt)+) => {
        $crate::event!(target: ::core::module_path!(), $crate::Level::WARN, $($arguments)+)
    };
}

#[macro_export]
macro_rules! warn_span {
    (target: $target:expr, parent: $parent:expr, $($arguments:tt)+) => {
        $crate::span!(target: $target, parent: $parent, $crate::Level::WARN, $($arguments)+)
    };
    (target: $target:expr, $($arguments:tt)+) => {
        $crate::span!(target: $target, $crate::Level::WARN, $($arguments)+)
    };
    (parent: $parent:expr, $($arguments:tt)+) => {
        $crate::span!(target: ::core::module_path!(), parent: $parent, $crate::Level::WARN, $($arguments)+)
    };
    ($($arguments:tt)+) => {
        $crate::span!(target: ::core::module_path!(), $crate::Level::WARN, $($arguments)+)
    };
}

#[macro_export]
macro_rules! error {
    (target: $target:expr, parent: $parent:expr, $($arguments:tt)+) => {
        $crate::event!(target: $target, parent: $parent, $crate::Level::ERROR, $($arguments)+)
    };
    (target: $target:expr, $($arguments:tt)+) => {
        $crate::event!(target: $target, $crate::Level::ERROR, $($arguments)+)
    };
    (parent: $parent:expr, $($arguments:tt)+) => {
        $crate::event!(target: ::core::module_path!(), parent: $parent, $crate::Level::ERROR, $($arguments)+)
    };
    ($($arguments:tt)+) => {
        $crate::event!(target: ::core::module_path!(), $crate::Level::ERROR, $($arguments)+)
    };
}

#[macro_export]
macro_rules! error_span {
    (target: $target:expr, parent: $parent:expr, $($arguments:tt)+) => {
        $crate::span!(target: $target, parent: $parent, $crate::Level::ERROR, $($arguments)+)
    };
    (target: $target:expr, $($arguments:tt)+) => {
        $crate::span!(target: $target, $crate::Level::ERROR, $($arguments)+)
    };
    (parent: $parent:expr, $($arguments:tt)+) => {
        $crate::span!(target: ::core::module_path!(), parent: $parent, $crate::Level::ERROR, $($arguments)+)
    };
    ($($arguments:tt)+) => {
        $crate::span!(target: ::core::module_path!(), $crate::Level::ERROR, $($arguments)+)
    };
}
