use std::task::{Context, Poll};

pub enum Branch<Value, Rest> {
    This(Value),
    Next(Rest),
}

pub struct Exhausted;

#[cfg(not(target_family = "wasm"))]
#[inline]
pub fn budget(context: &mut Context<'_>) -> Poll<()> {
    if nanocode_executor::coop::has_budget_remaining() {
        return Poll::Ready(());
    }
    nanocode_executor::coop::poll_proceed(context).map(drop)
}

#[cfg(target_family = "wasm")]
#[inline]
pub const fn budget(_context: &mut Context<'_>) -> Poll<()> {
    Poll::Ready(())
}

#[inline]
pub fn finished<Output>(slot: &mut Option<Output>) -> Output {
    match slot.take() {
        Some(output) => output,
        None => panic!("a joined future's output is taken once, after it finished"),
    }
}

#[macro_export]
macro_rules! select {
    (@wrap () $value:expr) => {
        $crate::select::Branch::This($value)
    };
    (@wrap (_ $($skip:tt)*) $value:expr) => {
        $crate::select::Branch::Next($crate::select!(@wrap ($($skip)*) $value))
    };
    (@pattern () $pattern:pat) => {
        $crate::select::Branch::This($pattern)
    };
    (@pattern (_ $($skip:tt)*) $pattern:pat) => {
        $crate::select::Branch::Next($crate::select!(@pattern ($($skip)*) $pattern))
    };
    (@nest $initial:ident) => {
        ()
    };
    (@nest $initial:ident ($($skip:tt)*) $($rest:tt)*) => {
        (
            $crate::select!(@nest $initial $($rest)*),
            ::core::pin::pin!({
                let ( $($skip,)* value, .. ) = $initial;
                value
            }),
        )
    };
    (@path ($($path:tt)*) () $last:tt) => {
        $($path)* . $last
    };
    (@path ($($path:tt)*) (_ $($skip:tt)*) $last:tt) => {
        $crate::select!(@path ($($path)* . 0) ($($skip)*) $last)
    };
    (@exhausted ()) => {
        $crate::select::Exhausted
    };
    (@exhausted (_ $($skip:tt)*)) => {
        $crate::select::Branch::Next($crate::select!(@exhausted ($($skip)*)))
    };
    (@ {
        ( $($count:tt)* )
        $( ( $($skip:tt)* ) $bind:pat = $future:expr, if $condition:expr => $handler:expr, )+
        ; $else:expr
    }) => {{
        let output = {
            let mut enabled = ( $( $condition, )+ );
            let initial = ( $( $future, )+ );
            let converted = ( $( ::core::future::IntoFuture::into_future({
                let ( $($skip,)* value, .. ) = initial;
                value
            }), )+ );
            let mut futures = $crate::select!(@nest converted $( ($($skip)*) )+);
            ::core::future::poll_fn(|context| {
                ::core::task::ready!($crate::select::budget(context));
                let mut pending = false;
                $(
                    let ( $($skip,)* branch_enabled, .. ) = &mut enabled;
                    if *branch_enabled {
                        let future = &mut $crate::select!(@path (futures) ($($skip)*) 1);
                        match ::core::future::Future::poll(future.as_mut(), context) {
                            ::core::task::Poll::Ready(output) => {
                                *branch_enabled = false;
                                #[allow(unused_variables, unreachable_patterns)]
                                let matched = match &output {
                                    $bind => true,
                                    _ => false,
                                };
                                if matched {
                                    return ::core::task::Poll::Ready($crate::select!(@wrap ($($skip)*) output));
                                }
                            }
                            ::core::task::Poll::Pending => pending = true,
                        }
                    }
                )+
                if pending {
                    ::core::task::Poll::Pending
                } else {
                    ::core::task::Poll::Ready($crate::select!(@exhausted ($($count)*)))
                }
            })
            .await
        };
        match output {
            $( $crate::select!(@pattern ($($skip)*) $bind) => $handler, )+
            $crate::select!(@exhausted ($($count)*)) => $else,
            #[allow(unreachable_patterns)]
            _ => ::core::unreachable!("a branch is selected only when its pattern matched"),
        }
    }};
    (@ { $($normalized:tt)* } ) => {
        $crate::select!(@{ $($normalized)*; ::core::panic!("all branches are disabled and there is no else branch") })
    };
    (@ { $($normalized:tt)* } else => $else:expr $(,)?) => {
        $crate::select!(@{ $($normalized)*; $else })
    };
    (@ { ( $($s:tt)* ) $($t:tt)* } $p:pat = $f:expr, if $c:expr => $h:block, $($r:tt)* ) => {
        $crate::select!(@{ ($($s)* _) $($t)* ($($s)*) $p = $f, if $c => $h, } $($r)*)
    };
    (@ { ( $($s:tt)* ) $($t:tt)* } $p:pat = $f:expr => $h:block, $($r:tt)* ) => {
        $crate::select!(@{ ($($s)* _) $($t)* ($($s)*) $p = $f, if true => $h, } $($r)*)
    };
    (@ { ( $($s:tt)* ) $($t:tt)* } $p:pat = $f:expr, if $c:expr => $h:block $($r:tt)* ) => {
        $crate::select!(@{ ($($s)* _) $($t)* ($($s)*) $p = $f, if $c => $h, } $($r)*)
    };
    (@ { ( $($s:tt)* ) $($t:tt)* } $p:pat = $f:expr => $h:block $($r:tt)* ) => {
        $crate::select!(@{ ($($s)* _) $($t)* ($($s)*) $p = $f, if true => $h, } $($r)*)
    };
    (@ { ( $($s:tt)* ) $($t:tt)* } $p:pat = $f:expr, if $c:expr => $h:expr ) => {
        $crate::select!(@{ ($($s)* _) $($t)* ($($s)*) $p = $f, if $c => $h, })
    };
    (@ { ( $($s:tt)* ) $($t:tt)* } $p:pat = $f:expr => $h:expr ) => {
        $crate::select!(@{ ($($s)* _) $($t)* ($($s)*) $p = $f, if true => $h, })
    };
    (@ { ( $($s:tt)* ) $($t:tt)* } $p:pat = $f:expr, if $c:expr => $h:expr, $($r:tt)* ) => {
        $crate::select!(@{ ($($s)* _) $($t)* ($($s)*) $p = $f, if $c => $h, } $($r)*)
    };
    (@ { ( $($s:tt)* ) $($t:tt)* } $p:pat = $f:expr => $h:expr, $($r:tt)* ) => {
        $crate::select!(@{ ($($s)* _) $($t)* ($($s)*) $p = $f, if true => $h, } $($r)*)
    };
    (else => $else:expr $(,)?) => {{
        $else
    }};
    ($p:pat = $($t:tt)*) => {
        $crate::select!(@{ () } $p = $($t)*)
    };
    () => {
        ::core::compile_error!("select! needs at least one branch")
    };
}

#[macro_export]
macro_rules! join {
    (@nest $initial:ident) => {
        ()
    };
    (@nest $initial:ident ($($skip:tt)*) $($rest:tt)*) => {
        (
            $crate::join!(@nest $initial $($rest)*),
            ::core::pin::pin!(::core::option::Option::Some({
                let ( $($skip,)* value, .. ) = $initial;
                value
            })),
            &mut ::core::option::Option::None,
        )
    };
    (@ { ( $($s:tt)* ) $( ( $($skip:tt)* ) $future:expr, )+ }) => {{
        let initial = ( $( ::core::future::IntoFuture::into_future($future), )+ );
        let mut futures = $crate::join!(@nest initial $( ($($skip)*) )+);
        ::core::future::poll_fn(|context| {
            let mut pending = false;
            $(
                let future = &mut $crate::select!(@path (futures) ($($skip)*) 1);
                if let ::core::option::Option::Some(running) = future.as_mut().as_pin_mut() {
                    match ::core::future::Future::poll(running, context) {
                        ::core::task::Poll::Ready(output) => {
                            future.set(::core::option::Option::None);
                            *$crate::select!(@path (futures) ($($skip)*) 2) = ::core::option::Option::Some(output);
                        }
                        ::core::task::Poll::Pending => pending = true,
                    }
                }
            )+
            if pending {
                return ::core::task::Poll::Pending;
            }
            ::core::task::Poll::Ready(( $( $crate::select::finished($crate::select!(@path (futures) ($($skip)*) 2)), )+ ))
        })
        .await
    }};
    (@ { ( $($s:tt)* ) $($t:tt)* } $future:expr, $($r:tt)*) => {
        $crate::join!(@{ ($($s)* _) $($t)* ($($s)*) $future, } $($r)*)
    };
    ($($future:expr),+ $(,)?) => {
        $crate::join!(@{ () } $($future,)+)
    };
}

#[macro_export]
macro_rules! try_join {
    (@ { ( $($s:tt)* ) $( ( $($skip:tt)* ) $future:expr, )+ }) => {{
        let initial = ( $( ::core::future::IntoFuture::into_future($future), )+ );
        let mut futures = $crate::join!(@nest initial $( ($($skip)*) )+);
        ::core::future::poll_fn(|context| {
            let mut pending = false;
            $(
                let future = &mut $crate::select!(@path (futures) ($($skip)*) 1);
                if let ::core::option::Option::Some(running) = future.as_mut().as_pin_mut() {
                    match ::core::future::Future::poll(running, context) {
                        ::core::task::Poll::Ready(::core::result::Result::Ok(output)) => {
                            future.set(::core::option::Option::None);
                            *$crate::select!(@path (futures) ($($skip)*) 2) = ::core::option::Option::Some(output);
                        }
                        ::core::task::Poll::Ready(::core::result::Result::Err(error)) => {
                            future.set(::core::option::Option::None);
                            return ::core::task::Poll::Ready(::core::result::Result::Err(error));
                        }
                        ::core::task::Poll::Pending => pending = true,
                    }
                }
            )+
            if pending {
                return ::core::task::Poll::Pending;
            }
            ::core::task::Poll::Ready(::core::result::Result::Ok(( $( $crate::select::finished($crate::select!(@path (futures) ($($skip)*) 2)), )+ )))
        })
        .await
    }};
    (@ { ( $($s:tt)* ) $($t:tt)* } $future:expr, $($r:tt)*) => {
        $crate::try_join!(@{ ($($s)* _) $($t)* ($($s)*) $future, } $($r)*)
    };
    ($($future:expr),+ $(,)?) => {
        $crate::try_join!(@{ () } $($future,)+)
    };
}
