#[macro_export]
macro_rules! cfg_if {
    (
        if #[cfg($first:meta)] { $($first_items:tt)* }
        $(else if #[cfg($condition:meta)] { $($items:tt)* })*
        $(else { $($otherwise:tt)* })?
    ) => {
        $crate::cfg_if! {
            @chain []
            [$first] { $($first_items)* }
            $([$condition] { $($items)* })*
            $([] { $($otherwise)* })?
        }
    };
    (@chain [$($previous:meta,)*]) => {};
    (@chain [$($previous:meta,)*] [] { $($items:tt)* }) => {
        $crate::cfg_if! { @emit #[cfg(not(any($($previous),*)))] $($items)* }
    };
    (@chain [$($previous:meta,)*] [$condition:meta] { $($items:tt)* } $($rest:tt)*) => {
        $crate::cfg_if! { @emit #[cfg(all($condition, not(any($($previous),*))))] $($items)* }
        $crate::cfg_if! { @chain [$($previous,)* $condition,] $($rest)* }
    };
    (@emit #[cfg($predicate:meta)] $($items:item)*) => {
        $(#[cfg($predicate)] $items)*
    };
}
