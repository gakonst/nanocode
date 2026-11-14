#[macro_export]
macro_rules! quote {
    () => {
        $crate::Tokens::new()
    };
    ($($tt:tt)*) => {
        $crate::quote_spanned!($crate::Span::call_site() => $($tt)*)
    };
}

#[macro_export]
macro_rules! quote_spanned {
    ($span:expr =>) => {{
        let _: $crate::Span = $span;
        $crate::Tokens::new()
    }};
    ($span:expr => $($tt:tt)*) => {{
        let mut _tokens = $crate::Tokens::new();
        let _span: $crate::Span = $span;
        $crate::quote_each_token!{_tokens _span $($tt)*}
        _tokens
    }};
}

#[macro_export]
macro_rules! parse_quote {
    ($($tt:tt)*) => {
        $crate::parse_quoted($crate::quote!($($tt)*))
    };
}

#[macro_export]
macro_rules! format_ident {
    ($format:expr) => {
        $crate::format_ident_with!([::core::option::Option::None, $format])
    };
    ($format:expr, $($rest:tt)*) => {
        $crate::format_ident_with!([::core::option::Option::None, $format] $($rest)*)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! format_ident_with {
    ([$span:expr, $($format:tt)*]) => {
        $crate::__private::make_ident(&::std::format!($($format)*), $span)
    };
    ([$old:expr, $($format:tt)*] span = $span:expr) => {
        $crate::format_ident_with!([$old, $($format)*] span = $span,)
    };
    ([$old:expr, $($format:tt)*] span = $span:expr, $($rest:tt)*) => {
        $crate::format_ident_with!([::core::option::Option::Some::<$crate::Span>($span), $($format)*] $($rest)*)
    };
    ([$span:expr, $($format:tt)*] $name:ident = $argument:expr) => {
        $crate::format_ident_with!([$span, $($format)*] $name = $argument,)
    };
    ([$span:expr, $($format:tt)*] $name:ident = $argument:expr, $($rest:tt)*) => {
        match $crate::__private::FragmentAdapter(&$argument) {
            argument => $crate::format_ident_with!([$span.or(argument.span()), $($format)*, $name = argument] $($rest)*),
        }
    };
    ([$span:expr, $($format:tt)*] $argument:expr) => {
        $crate::format_ident_with!([$span, $($format)*] $argument,)
    };
    ([$span:expr, $($format:tt)*] $argument:expr, $($rest:tt)*) => {
        match $crate::__private::FragmentAdapter(&$argument) {
            argument => $crate::format_ident_with!([$span.or(argument.span()), $($format)*, argument] $($rest)*),
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! quote_each_token {
    ($tokens:ident $span:ident $($tts:tt)*) => {
        $crate::quote_tokens_with_context!{$tokens $span
            (@ @ @ @ @ @ $($tts)*)
            (@ @ @ @ @ $($tts)* @)
            (@ @ @ @ $($tts)* @ @)
            (@ @ @ $(($tts))* @ @ @)
            (@ @ $($tts)* @ @ @ @)
            (@ $($tts)* @ @ @ @ @)
            ($($tts)* @ @ @ @ @ @)
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! quote_tokens_with_context {
    ($tokens:ident $span:ident
        ($($b3:tt)*) ($($b2:tt)*) ($($b1:tt)*)
        ($($curr:tt)*)
        ($($a1:tt)*) ($($a2:tt)*) ($($a3:tt)*)
    ) => {
        $(
            $crate::quote_token_with_context!{$tokens $span $b3 $b2 $b1 $curr $a1 $a2 $a3}
        )*
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! quote_token_with_context {
    ($tokens:ident $span:ident $b3:tt $b2:tt $b1:tt @ $a1:tt $a2:tt $a3:tt) => {};

    ($tokens:ident $span:ident $b3:tt $b2:tt $b1:tt (#) ( $($inner:tt)* ) * $a3:tt) => {{
        #[allow(unused_imports)]
        use $crate::__private::ext::*;
        let has_iter = $crate::__private::ThereIsNoIteratorInRepetition;
        $crate::pounded_var_names!{quote_bind_into_iter!(has_iter) () $($inner)*}
        let _: $crate::__private::HasIterator = has_iter;
        #[allow(while_true)]
        while true {
            $crate::pounded_var_names!{quote_bind_next_or_break!() () $($inner)*}
            $crate::quote_each_token!{$tokens $span $($inner)*}
        }
    }};
    ($tokens:ident $span:ident $b3:tt $b2:tt # (( $($inner:tt)* )) * $a2:tt $a3:tt) => {};
    ($tokens:ident $span:ident $b3:tt # ( $($inner:tt)* ) (*) $a1:tt $a2:tt $a3:tt) => {};

    ($tokens:ident $span:ident $b3:tt $b2:tt $b1:tt (#) ( $($inner:tt)* ) $sep:tt *) => {{
        #[allow(unused_imports)]
        use $crate::__private::ext::*;
        let mut _first = true;
        let has_iter = $crate::__private::ThereIsNoIteratorInRepetition;
        $crate::pounded_var_names!{quote_bind_into_iter!(has_iter) () $($inner)*}
        let _: $crate::__private::HasIterator = has_iter;
        #[allow(while_true)]
        while true {
            $crate::pounded_var_names!{quote_bind_next_or_break!() () $($inner)*}
            if !_first {
                $crate::quote_token!{$tokens $span $sep}
            }
            _first = false;
            $crate::quote_each_token!{$tokens $span $($inner)*}
        }
    }};
    ($tokens:ident $span:ident $b3:tt $b2:tt # (( $($inner:tt)* )) $sep:tt * $a3:tt) => {};
    ($tokens:ident $span:ident $b3:tt # ( $($inner:tt)* ) ($sep:tt) * $a2:tt $a3:tt) => {};
    ($tokens:ident $span:ident # ( $($inner:tt)* ) * (*) $a1:tt $a2:tt $a3:tt) => {
        $crate::quote_token!{$tokens $span *}
    };
    ($tokens:ident $span:ident # ( $($inner:tt)* ) $sep:tt (*) $a1:tt $a2:tt $a3:tt) => {};

    ($tokens:ident $span:ident $b3:tt $b2:tt $b1:tt (#) $var:ident $a2:tt $a3:tt) => {
        $crate::ToTokens::to_tokens(&$var, &mut $tokens);
    };
    ($tokens:ident $span:ident $b3:tt $b2:tt # ($var:ident) $a1:tt $a2:tt $a3:tt) => {};

    ($tokens:ident $span:ident $b3:tt $b2:tt $b1:tt ($curr:tt) $a1:tt $a2:tt $a3:tt) => {
        $crate::quote_token!{$tokens $span $curr}
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! quote_token {
    ($tokens:ident $span:ident ( $($inner:tt)* )) => {
        $crate::__private::push_group(&mut $tokens, $span, $crate::Delimiter::Parenthesis, {
            let mut _tokens = $crate::Tokens::new();
            $crate::quote_each_token!{_tokens $span $($inner)*}
            _tokens
        });
    };
    ($tokens:ident $span:ident [ $($inner:tt)* ]) => {
        $crate::__private::push_group(&mut $tokens, $span, $crate::Delimiter::Bracket, {
            let mut _tokens = $crate::Tokens::new();
            $crate::quote_each_token!{_tokens $span $($inner)*}
            _tokens
        });
    };
    ($tokens:ident $span:ident { $($inner:tt)* }) => {
        $crate::__private::push_group(&mut $tokens, $span, $crate::Delimiter::Brace, {
            let mut _tokens = $crate::Tokens::new();
            $crate::quote_each_token!{_tokens $span $($inner)*}
            _tokens
        });
    };
    ($tokens:ident $span:ident _) => {
        $crate::__private::push_ident(&mut $tokens, $span, "_");
    };
    ($tokens:ident $span:ident $ident:ident) => {
        $crate::__private::push_ident(&mut $tokens, $span, ::core::stringify!($ident));
    };
    ($tokens:ident $span:ident $lifetime:lifetime) => {
        $crate::__private::push_lifetime(&mut $tokens, $span, ::core::stringify!($lifetime));
    };
    ($tokens:ident $span:ident $other:tt) => {
        $crate::__private::push_other(&mut $tokens, $span, ::core::stringify!($other));
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! pounded_var_names {
    ($call:ident! $extra:tt $($tts:tt)*) => {
        $crate::pounded_var_names_with_context!{$call! $extra
            (@ $($tts)*)
            ($($tts)* @)
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! pounded_var_names_with_context {
    ($call:ident! $extra:tt ($($b1:tt)*) ($($curr:tt)*)) => {
        $(
            $crate::pounded_var_with_context!{$call! $extra $b1 $curr}
        )*
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! pounded_var_with_context {
    ($call:ident! $extra:tt $b1:tt ( $($inner:tt)* )) => {
        $crate::pounded_var_names!{$call! $extra $($inner)*}
    };
    ($call:ident! $extra:tt $b1:tt [ $($inner:tt)* ]) => {
        $crate::pounded_var_names!{$call! $extra $($inner)*}
    };
    ($call:ident! $extra:tt $b1:tt { $($inner:tt)* }) => {
        $crate::pounded_var_names!{$call! $extra $($inner)*}
    };
    ($call:ident!($($extra:tt)*) # $var:ident) => {
        $crate::$call!($($extra)* $var);
    };
    ($call:ident! $extra:tt $b1:tt $curr:tt) => {};
}

#[doc(hidden)]
#[macro_export]
macro_rules! quote_bind_into_iter {
    ($has_iter:ident $var:ident) => {
        #[allow(unused_mut)]
        let (mut $var, i) = $var.quote_into_iter();
        let $has_iter = $has_iter | i;
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! quote_bind_next_or_break {
    ($var:ident) => {
        let $var = match $var.next() {
            Some(_x) => $crate::__private::RepInterp(_x),
            None => break,
        };
    };
}
