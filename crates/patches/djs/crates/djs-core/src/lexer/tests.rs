//! Lexer unit tests.

extern crate std;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use std::format;

use super::*;
use TokenKind::*;

/// Lexes `src` with a small driver that handles template continuations (by tracking
/// braces, as the parser would) but never rescans `/` or `>`.
fn lex(src: &str) -> Result<Vec<Token>, LexError> {
    let mut lx = Lexer::new(src.as_bytes())?;
    let mut out = Vec::new();
    // Brace depth at each open template substitution.
    let mut templates: Vec<usize> = Vec::new();
    let mut depth = 0usize;
    loop {
        let mut tok = lx.next_token()?;
        match tok.kind {
            LBrace => depth += 1,
            RBrace if templates.last() == Some(&depth) => {
                tok = lx.rescan_template_continuation(tok)?;
                if tok.kind == TemplateTail {
                    templates.pop();
                }
            }
            RBrace => depth = depth.saturating_sub(1),
            TemplateHead => templates.push(depth),
            _ => {}
        }
        out.push(tok);
        if tok.kind == Eof {
            return Ok(out);
        }
    }
}

fn kinds(src: &str) -> Vec<TokenKind> {
    lex(src)
        .unwrap_or_else(|e| panic!("{src:?}: {e:?}"))
        .iter()
        .map(|t| t.kind)
        .filter(|&k| k != Eof)
        .collect()
}

/// The single token of `src` (followed by EOF).
fn one(src: &str) -> Token {
    let toks = lex(src).unwrap_or_else(|e| panic!("{src:?}: {e:?}"));
    assert_eq!(toks.len(), 2, "{src:?}: {toks:?}");
    assert_eq!(toks[1].kind, Eof);
    toks[0]
}

fn err(src: &str) -> LexError {
    match lex(src) {
        Ok(toks) => panic!("{src:?} lexed: {toks:?}"),
        Err(e) => e,
    }
}

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn cooked(src: &str) -> (Vec<u16>, bool) {
    let lx = Lexer::new(src.as_bytes()).unwrap();
    let tok = one(src);
    assert_eq!(tok.kind, String);
    let mut out = vec![0xAAAA]; // cooking appends
    let latin1 = lx.cook_string(&tok, &mut out);
    assert_eq!(out.remove(0), 0xAAAA);
    (out, latin1)
}

// ---------------------------------------------------------------------------------
// Punctuators

#[test]
fn punctuators() {
    let cases: &[(&str, TokenKind)] = &[
        ("{", LBrace),
        ("}", RBrace),
        ("(", LParen),
        (")", RParen),
        ("[", LBracket),
        ("]", RBracket),
        (".", Dot),
        ("...", Ellipsis),
        (";", Semicolon),
        (",", Comma),
        ("<", Lt),
        (">", Gt),
        ("<=", Le),
        ("==", Eq),
        ("!=", Ne),
        ("===", StrictEq),
        ("!==", StrictNe),
        ("+", Plus),
        ("-", Minus),
        ("*", Star),
        ("/", Slash),
        ("%", Percent),
        ("**", StarStar),
        ("++", PlusPlus),
        ("--", MinusMinus),
        ("<<", Shl),
        ("&", Amp),
        ("|", Pipe),
        ("^", Caret),
        ("!", Bang),
        ("~", Tilde),
        ("&&", AmpAmp),
        ("||", PipePipe),
        ("??", QuestionQuestion),
        ("?", Question),
        ("?.", QuestionDot),
        (":", Colon),
        ("=", Assign),
        ("+=", PlusAssign),
        ("-=", MinusAssign),
        ("*=", StarAssign),
        ("/=", SlashAssign),
        ("%=", PercentAssign),
        ("**=", StarStarAssign),
        ("<<=", ShlAssign),
        ("&=", AmpAssign),
        ("|=", PipeAssign),
        ("^=", CaretAssign),
        ("&&=", AmpAmpAssign),
        ("||=", PipePipeAssign),
        ("??=", QuestionQuestionAssign),
        ("=>", Arrow),
        ("@", At),
    ];
    for &(src, kind) in cases {
        let tok = one(src);
        assert_eq!(tok.kind, kind, "{src:?}");
        assert_eq!((tok.start, tok.end), (0, src.len() as u32), "{src:?}");
        assert_eq!(tok.kw, Kw::None);
    }
}

#[test]
fn punctuator_sequences() {
    assert_eq!(kinds("a++ + ++b"), [Ident, PlusPlus, Plus, PlusPlus, Ident]);
    assert_eq!(kinds("a+++b"), [Ident, PlusPlus, Plus, Ident]);
    assert_eq!(kinds("a---b"), [Ident, MinusMinus, Minus, Ident]);
    assert_eq!(kinds("x====y"), [Ident, StrictEq, Assign, Ident]);
    assert_eq!(kinds("x!===y"), [Ident, StrictNe, Assign, Ident]);
    assert_eq!(
        kinds("a**=b***c"),
        [Ident, StarStarAssign, Ident, StarStar, Star, Ident]
    );
    assert_eq!(kinds("...."), [Ellipsis, Dot]);
    assert_eq!(kinds(".."), [Dot, Dot]);
    assert_eq!(kinds("a<<=b<<c"), [Ident, ShlAssign, Ident, Shl, Ident]);
    assert_eq!(kinds("a=>b"), [Ident, Arrow, Ident]);
    assert_eq!(kinds("a? .b"), [Ident, Question, Dot, Ident]);
    // `?.` followed by a digit is `?` and a number.
    assert_eq!(kinds("a?.5:1"), [Ident, Question, Number, Colon, Number]);
    assert_eq!(kinds("a?.b"), [Ident, QuestionDot, Ident]);
    assert_eq!(
        kinds("a?.[0]"),
        [Ident, QuestionDot, LBracket, Number, RBracket]
    );
    assert_eq!(
        kinds("a ??= b ?? c"),
        [
            Ident,
            QuestionQuestionAssign,
            Ident,
            QuestionQuestion,
            Ident
        ]
    );
    assert_eq!(
        kinds("a&&=b||=c"),
        [Ident, AmpAmpAssign, Ident, PipePipeAssign, Ident]
    );
    assert_eq!(kinds("1..toString"), [Number, Dot, Ident]);
}

#[test]
fn greater_is_single_and_rescans() {
    // `>` sequences always lex as single `>` tokens.
    assert_eq!(kinds(">>>="), [Gt, Gt, Gt, Assign]);
    assert_eq!(kinds("a >= b"), [Ident, Gt, Assign, Ident]);
    assert_eq!(
        kinds("Array<Array<T>>"),
        [Ident, Lt, Ident, Lt, Ident, Gt, Gt]
    );

    let cases: &[(&str, TokenKind, u32)] = &[
        (">", Gt, 1),
        (">x", Gt, 1),
        ("> >", Gt, 1),
        (">=", Ge, 2),
        (">>", Shr, 2),
        (">>=", ShrAssign, 3),
        (">>>", UShr, 3),
        (">>>=", UShrAssign, 4),
        (">>>>", UShr, 3),
        (">>==", ShrAssign, 3),
        (">> >", Shr, 2),
    ];
    for &(src, kind, len) in cases {
        let mut lx = Lexer::new(src.as_bytes()).unwrap();
        let tok = lx.next_token().unwrap();
        assert_eq!(tok.kind, Gt);
        let tok = lx.rescan_greater(tok);
        assert_eq!((tok.kind, tok.end), (kind, len), "{src:?}");
        assert_eq!(lx.pos(), len);
        // Lexing continues after the combined token.
        let rest = lx.next_token().unwrap();
        assert!(rest.start >= len);
    }

    // The newline flag survives a rescan.
    let mut lx = Lexer::new(b"a\n>>b").unwrap();
    lx.next_token().unwrap();
    let gt = lx.next_token().unwrap();
    let shr = lx.rescan_greater(gt);
    assert_eq!(shr.kind, Shr);
    assert!(shr.newline_before());
}

// ---------------------------------------------------------------------------------
// Identifiers and keywords

fn ident_value(src: &str) -> String {
    let lx = Lexer::new(src.as_bytes()).unwrap();
    let tok = one(src);
    let mut buf = Vec::new();
    String::from_utf8(lx.ident_value(&tok, &mut buf).to_vec()).unwrap()
}

#[test]
fn identifiers() {
    for src in [
        "a",
        "abc",
        "_",
        "$",
        "$_$",
        "a1",
        "_0",
        "café",
        "ünïcödé",
        "𝓐",
        "a𝓐b",
        "中文",
        "x\u{200C}y",
        "a\u{200D}",
    ] {
        let tok = one(src);
        assert_eq!(tok.kind, Ident, "{src:?}");
        assert_eq!(tok.kw, Kw::None, "{src:?}");
        assert!(!tok.has_escape());
        assert_eq!(ident_value(src), src);
    }
    // Continue-only characters cannot start an identifier.
    assert_eq!(err("\u{0300}x").message, "unexpected character");
    assert_eq!(err("\u{200C}").message, "unexpected character");
    assert_eq!(err("・").message, "unexpected character");
    // But they can continue one (U+30FB is ID_Continue since Unicode 15.1).
    assert_eq!(kinds("a・"), [Ident]);
    // Non-identifier characters end an identifier.
    assert_eq!(kinds("a\u{A0}b"), [Ident, Ident]);
    assert_eq!(err("a€").message, "unexpected character");
    assert_eq!(err("a€").pos, 1);
}

#[test]
fn identifier_escapes() {
    let cases = [
        ("\\u0061", "a"),
        ("\\u{61}bc", "abc"),
        ("a\\u0062c", "abc"),
        ("\\u{0000000061}", "a"),
        ("\\u0024", "$"),
        ("_\\u200C", "_\u{200C}"),
        ("\\u{1d4d0}", "𝓐"),
    ];
    for (src, value) in cases {
        let tok = one(src);
        assert_eq!(tok.kind, Ident, "{src:?}");
        assert!(tok.has_escape(), "{src:?}");
        assert_eq!(ident_value(src), value, "{src:?}");
    }
    for src in [
        "\\u0031",        // digit cannot start
        "a\\u0020",       // space is not an identifier part
        "\\u{}",          // empty
        "\\u{110000}",    // out of range
        "\\u12",          // too short
        "\\x61",          // only \u is allowed
        "a\\",            // dangling
        "\\uD835\\uDC00", // surrogates are not identifier characters, not even paired
        "a\\u{2F}",       // `/`
    ] {
        let e = err(src);
        assert_eq!(
            e.message, "invalid Unicode escape sequence in identifier",
            "{src:?}"
        );
    }
    assert_eq!(err("ab\\u0020").pos, 2);
}

#[test]
fn keywords() {
    let reserved = [
        ("break", Break),
        ("case", Case),
        ("catch", Catch),
        ("class", Class),
        ("const", Const),
        ("continue", Continue),
        ("debugger", Debugger),
        ("default", Default),
        ("delete", Delete),
        ("do", Do),
        ("else", Else),
        ("enum", Enum),
        ("export", Export),
        ("extends", Extends),
        ("false", False),
        ("finally", Finally),
        ("for", For),
        ("function", Function),
        ("if", If),
        ("import", Import),
        ("in", In),
        ("instanceof", Instanceof),
        ("new", New),
        ("null", Null),
        ("return", Return),
        ("super", Super),
        ("switch", Switch),
        ("this", This),
        ("throw", Throw),
        ("true", True),
        ("try", Try),
        ("typeof", Typeof),
        ("var", Var),
        ("void", Void),
        ("while", While),
        ("with", With),
    ];
    for (src, kind) in reserved {
        let tok = one(src);
        assert_eq!(tok.kind, kind, "{src:?}");
        assert!(kind.is_reserved_word() && kind.is_identifier_name());
        assert_eq!(tok.kw.as_str(), src);
        assert_eq!(tok.kw.reserved_kind(), Some(kind));
        assert!(!tok.is_escaped_reserved());
    }
    let contextual = [
        "let",
        "static",
        "yield",
        "await",
        "async",
        "of",
        "get",
        "set",
        "target",
        "meta",
        "as",
        "from",
        "package",
        "eval",
        "arguments",
        "constructor",
        "using",
        "accessor",
        "type",
        "interface",
        "declare",
        "abstract",
        "readonly",
        "namespace",
        "module",
        "keyof",
        "infer",
        "is",
        "asserts",
        "satisfies",
        "override",
        "implements",
        "private",
        "protected",
        "public",
        "unique",
        "global",
        "out",
    ];
    for src in contextual {
        let tok = one(src);
        assert_eq!(tok.kind, Ident, "{src:?}");
        assert_eq!(tok.kw.as_str(), src);
        assert_eq!(tok.kw.reserved_kind(), None);
        assert!(tok.is_contextual(tok.kw));
    }
    for src in [
        "lets",
        "Let",
        "LET",
        "a",
        "i",
        "iff",
        "constructors",
        "instanceOf",
        "_if",
        "if_",
        "if1",
    ] {
        let tok = one(src);
        assert_eq!((tok.kind, tok.kw), (Ident, Kw::None), "{src:?}");
    }
    assert!(Kw::Let.is_strict_reserved() && Kw::Yield.is_strict_reserved());
    assert!(Kw::Package.is_strict_reserved() && !Kw::Await.is_strict_reserved());
    assert!(!Kw::Async.is_strict_reserved());
    assert!(Ident.is_identifier_name() && !Ident.is_reserved_word());
    assert!(!Number.is_identifier_name());
}

#[test]
fn keyword_table_round_trips() {
    // Every Kw maps back to itself through its text.
    let mut n = 0;
    for i in 1..=u8::MAX {
        // SAFETY-free enumeration: stop at the first id that does not round-trip.
        let Some(kw) = kw_from_u8(i) else { break };
        assert_eq!(Kw::lookup(kw.as_str().as_bytes()), kw, "{kw:?}");
        n += 1;
    }
    assert_eq!(n, 36 + 38);
}

/// All `Kw` values by discriminant (Kw is a fieldless `repr(u8)` enum).
fn kw_from_u8(i: u8) -> Option<Kw> {
    // Find by lookup of all lowercase ASCII words would be circular; instead walk a
    // list derived from the token kinds plus the contextual list.
    const ALL: &[Kw] = &[
        Kw::Break,
        Kw::Case,
        Kw::Catch,
        Kw::Class,
        Kw::Const,
        Kw::Continue,
        Kw::Debugger,
        Kw::Default,
        Kw::Delete,
        Kw::Do,
        Kw::Else,
        Kw::Enum,
        Kw::Export,
        Kw::Extends,
        Kw::False,
        Kw::Finally,
        Kw::For,
        Kw::Function,
        Kw::If,
        Kw::Import,
        Kw::In,
        Kw::Instanceof,
        Kw::New,
        Kw::Null,
        Kw::Return,
        Kw::Super,
        Kw::Switch,
        Kw::This,
        Kw::Throw,
        Kw::True,
        Kw::Try,
        Kw::Typeof,
        Kw::Var,
        Kw::Void,
        Kw::While,
        Kw::With,
        Kw::Let,
        Kw::Static,
        Kw::Yield,
        Kw::Await,
        Kw::Async,
        Kw::Of,
        Kw::Get,
        Kw::Set,
        Kw::Target,
        Kw::Meta,
        Kw::As,
        Kw::From,
        Kw::Package,
        Kw::Eval,
        Kw::Arguments,
        Kw::Constructor,
        Kw::Using,
        Kw::Accessor,
        Kw::Type,
        Kw::Interface,
        Kw::Declare,
        Kw::Abstract,
        Kw::Readonly,
        Kw::Namespace,
        Kw::Module,
        Kw::Keyof,
        Kw::Infer,
        Kw::Is,
        Kw::Asserts,
        Kw::Satisfies,
        Kw::Override,
        Kw::Implements,
        Kw::Private,
        Kw::Protected,
        Kw::Public,
        Kw::Unique,
        Kw::Global,
        Kw::Out,
    ];
    let kw = *ALL.get(usize::from(i) - 1)?;
    assert_eq!(kw as u8, i, "Kw order changed: {kw:?}");
    Some(kw)
}

#[test]
fn escaped_keywords() {
    // An escaped reserved word is an identifier with the reserved keyword id.
    let tok = one("\\u0069f");
    assert_eq!((tok.kind, tok.kw), (Ident, Kw::If));
    assert!(tok.has_escape() && tok.is_escaped_reserved());
    let tok = one("v\\u{61}r");
    assert_eq!((tok.kind, tok.kw), (Ident, Kw::Var));
    // An escaped contextual keyword has the id but is not usable as the keyword.
    let tok = one("l\\u0065t");
    assert_eq!((tok.kind, tok.kw), (Ident, Kw::Let));
    assert!(!tok.is_contextual(Kw::Let) && !tok.is_escaped_reserved());
    let tok = one("\\u{61}sync");
    assert_eq!(tok.kw, Kw::Async);
    // Long or non-ASCII escaped names are not keywords.
    assert_eq!(one("\\u0069fffffffffffffff").kw, Kw::None);
    assert_eq!(one("\\u00e9").kw, Kw::None);
}

#[test]
fn private_names() {
    let tok = one("#x");
    assert_eq!((tok.kind, tok.start, tok.end), (PrivateName, 0, 2));
    assert_eq!(ident_value("#x"), "#x");
    assert_eq!(one("#constructor").kw, Kw::Constructor);
    assert_eq!(one("#if").kind, PrivateName);
    assert_eq!(one("#if").kw, Kw::If);
    assert_eq!(ident_value("#\\u{61}b"), "#ab");
    assert_eq!(ident_value("#é"), "#é");
    assert_eq!(
        kinds("this.#a.#b"),
        [This, Dot, PrivateName, Dot, PrivateName]
    );
    assert_eq!(kinds("#a in b"), [PrivateName, In, Ident]);
    for src in ["#", "#1", "# a", "#\u{0300}", "a #"] {
        assert_eq!(
            err(src).message,
            "invalid first character of private name",
            "{src:?}"
        );
    }
    assert_eq!(
        err("#\\u0031").message,
        "invalid Unicode escape sequence in identifier"
    );
}

// ---------------------------------------------------------------------------------
// Numbers

fn number(src: &str) -> (TokenKind, NumberLiteral<'_>) {
    let lx = Lexer::new(src.as_bytes()).unwrap();
    let tok = one(src);
    let lit = lx.number_literal(&tok);
    assert_eq!(lit.has_separators, tok.has_flag(Token::HAS_SEPARATOR));
    (tok.kind, lit)
}

#[test]
fn numbers_valid() {
    use NumberBase::*;
    let cases: &[(&str, TokenKind, NumberBase, &str)] = &[
        ("0", Number, Decimal, "0"),
        ("7", Number, Decimal, "7"),
        ("123", Number, Decimal, "123"),
        ("1_000_000", Number, Decimal, "1_000_000"),
        ("0.5", Number, Decimal, "0.5"),
        (".5", Number, Decimal, ".5"),
        ("5.", Number, Decimal, "5."),
        ("5.e1", Number, Decimal, "5.e1"),
        ("1e10", Number, Decimal, "1e10"),
        ("1E+10", Number, Decimal, "1E+10"),
        ("1e-1_0", Number, Decimal, "1e-1_0"),
        ("0e0", Number, Decimal, "0e0"),
        ("0.0_1", Number, Decimal, "0.0_1"),
        (".5_5e5_5", Number, Decimal, ".5_5e5_5"),
        ("1_0.0_1e1_0", Number, Decimal, "1_0.0_1e1_0"),
        ("0x0", Number, Hex, "0"),
        ("0XdeadBEEF", Number, Hex, "deadBEEF"),
        ("0xF_F", Number, Hex, "F_F"),
        ("0o17", Number, Octal, "17"),
        ("0O7_7", Number, Octal, "7_7"),
        ("0b1010", Number, Binary, "1010"),
        ("0B1_0", Number, Binary, "1_0"),
        ("0n", BigInt, Decimal, "0"),
        ("123n", BigInt, Decimal, "123"),
        ("1_2n", BigInt, Decimal, "1_2"),
        ("0xFFn", BigInt, Hex, "FF"),
        ("0o7n", BigInt, Octal, "7"),
        ("0b1n", BigInt, Binary, "1"),
    ];
    for &(src, kind, base, digits) in cases {
        let (k, lit) = number(src);
        assert_eq!(k, kind, "{src:?}");
        assert_eq!(lit.base, base, "{src:?}");
        assert_eq!(lit.digits, digits.as_bytes(), "{src:?}");
        assert_eq!(lit.bigint, kind == BigInt);
        assert_eq!(lit.has_separators, digits.contains('_'), "{src:?}");
    }
    assert_eq!(NumberBase::Hex.radix(), 16);
    let mut out = Vec::new();
    strip_separators(b"1_000.0_1e1_0", &mut out);
    assert_eq!(out, b"1000.01e10");

    // Numbers followed by things that are not identifier starts.
    assert_eq!(kinds("1+2"), [Number, Plus, Number]);
    assert_eq!(kinds("1 .x"), [Number, Dot, Ident]);
    assert_eq!(kinds("1n.x"), [BigInt, Dot, Ident]);
    assert_eq!(kinds("1.5.x"), [Number, Dot, Ident]);
    assert_eq!(kinds("0.0.0"), [Number, Number]);
    assert_eq!(kinds("[1,2]"), [LBracket, Number, Comma, Number, RBracket]);
    assert_eq!(kinds("1\u{A0}"), [Number]);
}

#[test]
fn numbers_invalid() {
    let invalid = [
        "1__0", "1_", "0_1", "0_", "1._1", "1_.1", "1e_1", "1e+_1", "1e1_", "1e", "1e+", "1.e",
        "0x", "0x_1", "0x1_", "0x1__1", "0xg", "0x1g", "0b", "0b2", "0b12", "0o", "0o8", "0o78",
        "0b1_", "1n2", "0.5n", ".5n", "1e3n", "1.n", "3in x", "1a", "1$", "1_n", "0xn", "1\\u0061",
        "1é", "1𝓐", "0n1", "1__", "0_0",
    ];
    for src in invalid {
        let e = err(src);
        assert_eq!(e.message, "invalid number literal", "{src:?}");
        assert_eq!(e.pos, 0, "{src:?}");
    }
    for src in [
        "00", "01", "07", "08", "09", "0777", "08.5", "09e1", "01n", "00n",
    ] {
        let e = err(src);
        assert_eq!(
            e.message, "octal literals are deprecated in strict mode",
            "{src:?}"
        );
    }
    // The error points at the literal.
    assert_eq!(err("x = 1__0").pos, 4);
    // `._1` is a dot and an identifier.
    assert_eq!(kinds("._1"), [Dot, Ident]);
}

// ---------------------------------------------------------------------------------
// Strings

#[test]
fn strings_plain() {
    let lx = Lexer::new(b"'abc' \"d'e\" ''").unwrap();
    let toks = lex("'abc' \"d'e\" ''").unwrap();
    assert_eq!(
        toks.iter().map(|t| t.kind).collect::<Vec<_>>(),
        [String, String, String, Eof]
    );
    assert_eq!(lx.string_content(&toks[0]), b"abc");
    assert_eq!(lx.string_content(&toks[1]), b"d'e");
    assert_eq!(lx.string_content(&toks[2]), b"");
    assert!(toks.iter().all(|t| !t.has_escape()));
    assert_eq!(cooked("'abc'"), (utf16("abc"), true));
    assert_eq!(cooked("'café'"), (utf16("café"), true));
    assert_eq!(cooked("'ÿ'"), (utf16("ÿ"), true));
    assert_eq!(cooked("'Ā'"), (utf16("Ā"), false));
    assert_eq!(cooked("'😀'"), (utf16("😀"), false));
    assert_eq!(cooked("''"), (utf16(""), true));
    // LS and PS are allowed unescaped; other control characters too.
    assert_eq!(
        cooked("'a\u{2028}b\u{2029}'"),
        (utf16("a\u{2028}b\u{2029}"), false)
    );
    assert_eq!(
        cooked("'\u{0}\u{1}\t\u{7f}'"),
        (utf16("\u{0}\u{1}\t\u{7f}"), true)
    );
    assert!(!one("'\u{2028}'").has_escape());
}

#[test]
fn strings_escapes() {
    let cases: &[(&str, &[u16], bool)] = &[
        (r"'\n\t\r\b\f\v'", &[10, 9, 13, 8, 12, 11], true),
        (r#"'\'\"\\'"#, &[0x27, 0x22, 0x5C], true),
        (r"'\0'", &[0], true),
        (r"'\0a'", &[0, 0x61], true),
        (r"'\x41\xff\xFF'", &[0x41, 0xFF, 0xFF], true),
        (r"'\u0041\u00e9'", &[0x41, 0xE9], true),
        (r"'\u0100'", &[0x100], false),
        (
            r"'\u{41}\u{0000041}\u{10FFFF}'",
            &[0x41, 0x41, 0xDBFF, 0xDFFF],
            false,
        ),
        (r"'\u{1F600}'", &[0xD83D, 0xDE00], false),
        (r"'\uD83D\uDE00'", &[0xD83D, 0xDE00], false),
        // Lone surrogates are allowed.
        (r"'\uD800'", &[0xD800], false),
        (r"'\uDC00x'", &[0xDC00, 0x78], false),
        (r"'\u{D800}'", &[0xD800], false),
        // NonEscapeCharacter escapes itself.
        (r"'\a\q\$\ \é'", &[0x61, 0x71, 0x24, 0x20, 0xE9], true),
        ("'\\😀'", &[0xD83D, 0xDE00], false),
        // Line continuations.
        ("'a\\\nb'", &[0x61, 0x62], true),
        ("'a\\\r\nb'", &[0x61, 0x62], true),
        ("'a\\\rb'", &[0x61, 0x62], true),
        ("'a\\\u{2028}b\\\u{2029}c'", &[0x61, 0x62, 0x63], true),
    ];
    for &(src, value, latin1) in cases {
        let tok = one(src);
        assert!(tok.has_escape(), "{src:?}");
        assert_eq!(cooked(src), (value.to_vec(), latin1), "{src:?}");
    }
}

#[test]
fn strings_invalid() {
    let cases: &[(&str, &str, u32)] = &[
        ("'abc", "unexpected end of string", 0),
        ("x = 'abc", "unexpected end of string", 4),
        ("'a\nb'", "unexpected end of string", 0),
        ("'a\rb'", "unexpected end of string", 0),
        ("'\\", "unexpected end of string", 0),
        ("'\\'", "unexpected end of string", 0),
        ("\"a'", "unexpected end of string", 0),
        (
            r"'\01'",
            "octal escape sequences are not allowed in strict mode",
            1,
        ),
        (
            r"'\00'",
            "octal escape sequences are not allowed in strict mode",
            1,
        ),
        (
            r"'\1'",
            "octal escape sequences are not allowed in strict mode",
            1,
        ),
        (
            r"'a\7'",
            "octal escape sequences are not allowed in strict mode",
            2,
        ),
        (
            r"'\08'",
            "octal escape sequences are not allowed in strict mode",
            1,
        ),
        (r"'\8'", "malformed escape sequence in string literal", 1),
        (r"'\9'", "malformed escape sequence in string literal", 1),
        (r"'\x'", "malformed escape sequence in string literal", 1),
        (r"'\x4'", "malformed escape sequence in string literal", 1),
        (r"'\xg0'", "malformed escape sequence in string literal", 1),
        (r"'\u'", "malformed escape sequence in string literal", 1),
        (r"'\u12'", "malformed escape sequence in string literal", 1),
        (
            r"'\u123g'",
            "malformed escape sequence in string literal",
            1,
        ),
        (r"'\u{}'", "malformed escape sequence in string literal", 1),
        (
            r"'\u{110000}'",
            "malformed escape sequence in string literal",
            1,
        ),
        (
            r"'\u{FFFFFFFFFFFF}'",
            "malformed escape sequence in string literal",
            1,
        ),
        (r"'\u{12'", "malformed escape sequence in string literal", 1),
        (
            r"'\u{-1}'",
            "malformed escape sequence in string literal",
            1,
        ),
    ];
    for &(src, msg, pos) in cases {
        let e = err(src);
        assert_eq!((e.message, e.pos), (msg, pos), "{src:?}");
    }
}

// ---------------------------------------------------------------------------------
// Templates

fn template_values(src: &str) -> Vec<(TokenKind, Option<Vec<u16>>, Vec<u16>)> {
    let lx = Lexer::new(src.as_bytes()).unwrap();
    lex(src)
        .unwrap()
        .iter()
        .filter(|t| {
            matches!(
                t.kind,
                NoSubstTemplate | TemplateHead | TemplateMiddle | TemplateTail
            )
        })
        .map(|t| {
            let mut c = Vec::new();
            let cooked = lx.cook_template(t, &mut c).ok().map(|_| c);
            assert_eq!(cooked.is_none(), t.has_flag(Token::INVALID_ESCAPE));
            let mut r = Vec::new();
            lx.template_raw(t, &mut r);
            if !t.has_escape() {
                assert_eq!(
                    lx.template_content(t),
                    String::from_utf16(cooked.as_ref().unwrap())
                        .unwrap()
                        .as_bytes()
                );
            }
            if !t.has_flag(Token::HAS_CR) {
                assert_eq!(
                    lx.template_content(t),
                    String::from_utf16(&r).unwrap().as_bytes()
                );
            }
            (t.kind, cooked, r)
        })
        .collect()
}

#[test]
fn templates_structure() {
    assert_eq!(kinds("``"), [NoSubstTemplate]);
    assert_eq!(kinds("`a`"), [NoSubstTemplate]);
    assert_eq!(kinds("`a${b}c`"), [TemplateHead, Ident, TemplateTail]);
    assert_eq!(
        kinds("`${a}${b}`"),
        [TemplateHead, Ident, TemplateMiddle, Ident, TemplateTail]
    );
    assert_eq!(
        kinds("`a${ {x: `b${c}`} }d`"),
        [
            TemplateHead,
            LBrace,
            Ident,
            Colon,
            TemplateHead,
            Ident,
            TemplateTail,
            RBrace,
            TemplateTail
        ]
    );
    assert_eq!(kinds("`$`"), [NoSubstTemplate]);
    assert_eq!(kinds("`$$`"), [NoSubstTemplate]);
    assert_eq!(kinds("`{}`"), [NoSubstTemplate]);
    assert_eq!(kinds("`\\${x}`"), [NoSubstTemplate]);
    assert_eq!(kinds("`$\\{x}`"), [NoSubstTemplate]);
    assert_eq!(kinds("`$${x}`"), [TemplateHead, Ident, TemplateTail]);
    assert_eq!(kinds("tag`x`"), [Ident, NoSubstTemplate]);

    let src = "`ab${x}cd${y}ef`";
    let lx = Lexer::new(src.as_bytes()).unwrap();
    let toks = lex(src).unwrap();
    let parts: Vec<&[u8]> = [0, 2, 4]
        .iter()
        .map(|&i| lx.template_content(&toks[i]))
        .collect();
    assert_eq!(parts, [&b"ab"[..], b"cd", b"ef"]);
    assert_eq!((toks[2].start, toks[2].end), (6, 11));

    // The newline flag of the `}` is kept.
    let src = "`${x\n}`";
    assert!(lex(src).unwrap()[2].newline_before());

    for src in ["`abc", "`abc${x}def", "`\\`", "`$"] {
        assert_eq!(err(src).message, "unexpected end of string", "{src:?}");
    }
}

#[test]
fn templates_cooked_and_raw() {
    let v = template_values("`a\\nb`");
    assert_eq!(v, [(NoSubstTemplate, Some(utf16("a\nb")), utf16("a\\nb"))]);
    // CR and CRLF are normalized to LF in both values.
    let v = template_values("`a\r\nb\rc\nd`");
    assert_eq!(
        v,
        [(
            NoSubstTemplate,
            Some(utf16("a\nb\nc\nd")),
            utf16("a\nb\nc\nd")
        )]
    );
    // A line continuation is empty when cooked and normalized in raw.
    let v = template_values("`a\\\r\nb`");
    assert_eq!(v, [(NoSubstTemplate, Some(utf16("ab")), utf16("a\\\nb"))]);
    let v = template_values("`\\u{1F600}\\x41\\0`");
    assert_eq!(v[0].1, Some(utf16("😀A\0")));
    assert_eq!(v[0].2, utf16("\\u{1F600}\\x41\\0"));
    let v = template_values("`a${1}é${2}\\``");
    assert_eq!(
        v,
        [
            (TemplateHead, Some(utf16("a")), utf16("a")),
            (TemplateMiddle, Some(utf16("é")), utf16("é")),
            (TemplateTail, Some(utf16("`")), utf16("\\`")),
        ]
    );
    // Latin-1 reporting.
    let lx = Lexer::new("`é`".as_bytes()).unwrap();
    let tok = one("`é`");
    let mut out = Vec::new();
    assert_eq!(lx.cook_template(&tok, &mut out), Ok(true));
    assert!(lx.template_raw(&tok, &mut out));
    let lx = Lexer::new("`\\u0100`".as_bytes()).unwrap();
    let tok = one("`\\u0100`");
    assert_eq!(lx.cook_template(&tok, &mut out), Ok(false));
    assert!(lx.template_raw(&tok, &mut out));
}

#[test]
fn templates_invalid_escapes() {
    // Invalid escapes are not lexing errors; the cooked value is undefined.
    for src in [
        "`\\01`",
        "`\\1`",
        "`\\8`",
        "`\\9`",
        "`\\00`",
        "`\\x`",
        "`\\x4`",
        "`\\u`",
        "`\\u12`",
        "`\\u{`",
        "`\\u{}`",
        "`\\u{110000}`",
        "`\\u{12`",
        "`\\xg`",
        "`a\\u{d`",
    ] {
        let tok = one(src);
        assert_eq!(tok.kind, NoSubstTemplate, "{src:?}");
        assert!(tok.has_flag(Token::INVALID_ESCAPE), "{src:?}");
        let v = template_values(src);
        assert_eq!(v[0].1, None, "{src:?}");
        assert_eq!(v[0].2, utf16(&src[1..src.len() - 1]), "{src:?}");
    }
    // `\0` not followed by a digit is fine.
    assert!(!one("`\\0a`").has_flag(Token::INVALID_ESCAPE));
    // The error from cooking points at the escape.
    let src = "`ab\\x`";
    let lx = Lexer::new(src.as_bytes()).unwrap();
    let e = lx.cook_template(&one(src), &mut Vec::new()).unwrap_err();
    assert_eq!(
        (e.pos, e.message),
        (3, "malformed escape sequence in string literal")
    );
    // Scanning resumes right after the escape character, so `${` and `` ` `` still count.
    assert_eq!(kinds("`\\u${x}`"), [TemplateHead, Ident, TemplateTail]);
    assert_eq!(kinds("`\\u{`"), [NoSubstTemplate]);
    assert_eq!(kinds("`\\x${x}\\u{}`"), [TemplateHead, Ident, TemplateTail]);
    let v = template_values("`\\u${x}ok\\x`");
    assert_eq!(v[0].1, None);
    assert_eq!(v[1].1, None);
    let v = template_values("`\\u${x}ok`");
    assert_eq!(v[1].1, Some(utf16("ok")));
}

// ---------------------------------------------------------------------------------
// Regular expressions

fn regex(src: &str) -> Result<(String, String, u32), LexError> {
    let mut lx = Lexer::new(src.as_bytes()).unwrap();
    let tok = lx.next_token().unwrap();
    assert!(matches!(tok.kind, Slash | SlashAssign), "{src:?}");
    let tok = lx.rescan_as_regex(tok)?;
    assert_eq!(tok.kind, Regex);
    assert_eq!(lx.pos(), tok.end);
    let (body, flags) = lx.regex_parts(&tok);
    let s = |sp: Span| String::from_utf8(sp.slice(lx.source()).to_vec()).unwrap();
    Ok((s(body), s(flags), tok.end))
}

#[test]
fn regex_literals() {
    let cases: &[(&str, &str, &str)] = &[
        ("/a/", "a", ""),
        ("/a/g", "a", "g"),
        ("/a/dgimsuy", "a", "dgimsuy"),
        ("/a/v", "a", "v"),
        ("/=/", "=", ""),
        ("/=a/g", "=a", "g"),
        ("/[/]/", "[/]", ""),
        ("/[\\]/]/", "[\\]/]", ""),
        ("/\\//", "\\/", ""),
        ("/a\\/b/i", "a\\/b", "i"),
        ("/[]/", "[]", ""),
        ("/[[/]]/v", "[[/]]", "v"),
        ("/é😀/u", "é😀", "u"),
        ("/\\é/", "\\é", ""),
        ("/(?:)/", "(?:)", ""),
        ("/a/ /b/", "a", ""),
        ("/a/.test(x)", "a", ""),
        ("/a/g\\u0069", "a", "g"),
        ("/a/\u{2028}", "a", ""),
    ];
    for &(src, body, flags) in cases {
        let (b, f, _) = regex(src).unwrap_or_else(|e| panic!("{src:?}: {e:?}"));
        assert_eq!((b.as_str(), f.as_str()), (body, flags), "{src:?}");
    }
    // Lexing continues after the regex.
    let mut lx = Lexer::new(b"x = /a/g.source").unwrap();
    lx.next_token().unwrap();
    lx.next_token().unwrap();
    let slash = lx.next_token().unwrap();
    let re = lx.rescan_as_regex(slash).unwrap();
    assert_eq!((re.start, re.end), (4, 8));
    assert_eq!(lx.next_token().unwrap().kind, Dot);
    // Newline flag kept.
    let mut lx = Lexer::new(b"\n/a/").unwrap();
    let slash = lx.next_token().unwrap();
    assert!(lx.rescan_as_regex(slash).unwrap().newline_before());
}

#[test]
fn regex_invalid() {
    let cases: &[(&str, &str, u32)] = &[
        ("/abc", "unexpected end of regexp", 0),
        ("/a\\", "unexpected end of regexp", 0),
        ("/[/", "unexpected end of regexp", 0),
        ("/a\nb/", "unexpected line terminator in regexp", 2),
        ("/a\rb/", "unexpected line terminator in regexp", 2),
        ("/a\\\nb/", "unexpected line terminator in regexp", 3),
        ("/a\u{2028}/", "unexpected line terminator in regexp", 2),
        ("/a\\\u{2029}/", "unexpected line terminator in regexp", 3),
        ("/[\n]/", "unexpected line terminator in regexp", 2),
        ("/a/gg", "invalid regular expression flags", 0),
        ("/a/x", "invalid regular expression flags", 0),
        ("/a/G", "invalid regular expression flags", 0),
        ("/a/uv", "invalid regular expression flags", 0),
        ("/a/vgu", "invalid regular expression flags", 0),
        ("/a/gé", "invalid regular expression flags", 0),
        ("/a/i$", "invalid regular expression flags", 0),
        ("/a/i\u{200C}", "invalid regular expression flags", 0),
    ];
    for &(src, msg, pos) in cases {
        let e = regex(src).unwrap_err();
        assert_eq!((e.message, e.pos), (msg, pos), "{src:?}");
    }
}

// ---------------------------------------------------------------------------------
// White space, line terminators, comments

#[test]
fn whitespace() {
    for ws in [
        "\t", "\u{B}", "\u{C}", " ", "\u{A0}", "\u{FEFF}", "\u{1680}", "\u{2000}", "\u{2001}",
        "\u{2002}", "\u{2003}", "\u{2004}", "\u{2005}", "\u{2006}", "\u{2007}", "\u{2008}",
        "\u{2009}", "\u{200A}", "\u{202F}", "\u{205F}", "\u{3000}",
    ] {
        let src = format!("a{ws}b");
        let toks = lex(&src).unwrap();
        assert_eq!(toks.len(), 3, "{ws:?}");
        assert!(!toks[1].newline_before(), "{ws:?}");
        assert_eq!(toks[1].start as usize, 1 + ws.len());
    }
    // Not white space.
    for c in [
        "\u{180E}", "\u{200B}", "\u{85}", "\u{0}", "\u{1}", "\u{7F}", "\u{E000}",
    ] {
        assert_eq!(
            err(&format!("a {c}")).message,
            "unexpected character",
            "{c:?}"
        );
    }
    // A BOM at the start is just white space.
    assert_eq!(kinds("\u{FEFF}x"), [Ident]);
}

#[test]
fn newline_before() {
    let flags = |src: &str| -> Vec<bool> {
        lex(src)
            .unwrap()
            .iter()
            .map(|t| t.newline_before())
            .collect()
    };
    assert_eq!(flags("a b"), [false, false, false]);
    for nl in [
        "\n",
        "\r",
        "\r\n",
        "\u{2028}",
        "\u{2029}",
        "/*\n*/",
        "/*\u{2028}*/",
        "// c\n",
        "/* */\n",
        "\n/* */",
    ] {
        assert_eq!(flags(&format!("a{nl}b")), [false, true, false], "{nl:?}");
    }
    assert_eq!(flags("a/* */b"), [false, false, false]);
    assert_eq!(flags("\na"), [true, false]);
    // EOF after a newline also has the flag.
    assert_eq!(flags("a\n"), [false, true]);
    // `//` comments end at any line terminator.
    for nl in ["\n", "\r", "\u{2028}", "\u{2029}"] {
        assert_eq!(kinds(&format!("a // x {nl} b")), [Ident, Ident], "{nl:?}");
    }
    assert_eq!(kinds("a // x"), [Ident]);
    assert_eq!(
        kinds("a /* x */ b /**/ c /***/ d /* * / */"),
        [Ident, Ident, Ident, Ident]
    );
    // HTML-like comments are not comments.
    assert_eq!(kinds("a<!--b"), [Ident, Lt, Bang, MinusMinus, Ident]);
    assert_eq!(kinds("\n-->b"), [MinusMinus, Gt, Ident]);
}

#[test]
fn comments_invalid() {
    let e = err("a /* x");
    assert_eq!((e.message, e.pos), ("unexpected end of comment", 2));
    assert_eq!(err("/*/").message, "unexpected end of comment");
    assert_eq!(err("/* *").message, "unexpected end of comment");
}

#[test]
fn hashbang() {
    assert_eq!(kinds("#!/usr/bin/env node\nx"), [Ident]);
    assert_eq!(kinds("#!"), []);
    assert_eq!(kinds("#! x\u{2028}y"), [Ident]);
    // The newline after the hashbang counts for the first token.
    assert!(lex("#!x\ny").unwrap()[0].newline_before());
    // Only at the very start.
    assert_eq!(
        err(" #!x").message,
        "invalid first character of private name"
    );
    assert_eq!(
        err("\n#!x").message,
        "invalid first character of private name"
    );
    assert_eq!(
        err("\u{FEFF}#!x").message,
        "invalid first character of private name"
    );
}

// ---------------------------------------------------------------------------------
// UTF-8, errors, positions, checkpoints

#[test]
fn invalid_utf8() {
    let cases: &[(&[u8], u32)] = &[
        (b"\xFF", 0),
        (b"a \x80", 2),
        (b"'\xC3'", 1),
        (b"'\xC3\x28'", 1),
        (b"// \xE2\x82", 3),
        (b"/* \xF0\x9F\x98 */", 3),
        (b"`\xC0\xAF`", 1),         // overlong
        (b"'\xED\xA0\x80'", 1),     // surrogate
        (b"'\xF4\x90\x80\x80'", 1), // > U+10FFFF
        (b"a\xE0\x80\x80", 1),      // overlong 3-byte
        (b"'\\\xFF'", 2),
        (b"`\\\xFF`", 2),
        (b"#!\xFF", 2),
    ];
    for &(src, pos) in cases {
        let mut lx = match Lexer::new(src) {
            Ok(lx) => lx,
            Err(e) => {
                assert_eq!(
                    (e.message, e.pos),
                    ("invalid UTF-8 sequence", pos),
                    "{src:?}"
                );
                continue;
            }
        };
        let e = loop {
            match lx.next_token() {
                Ok(t) if t.kind == Eof => panic!("{src:?} lexed"),
                Ok(_) => {}
                Err(e) => break e,
            }
        };
        assert_eq!(
            (e.message, e.pos),
            ("invalid UTF-8 sequence", pos),
            "{src:?}"
        );
    }
    // Invalid UTF-8 in a regex body.
    let mut lx = Lexer::new(b"/\xFF/").unwrap();
    let slash = lx.next_token().unwrap();
    assert_eq!(
        lx.rescan_as_regex(slash).unwrap_err().message,
        "invalid UTF-8 sequence"
    );
}

#[test]
fn decode_utf8_matches_std() {
    // Every 1-, 2- and 3-byte sequence, and the 4-byte boundaries.
    let check = |bytes: &[u8]| {
        let ours = decode_utf8(bytes, 0);
        let valid = match core::str::from_utf8(bytes) {
            Ok(_) => bytes.len(),
            Err(e) => e.valid_up_to(),
        };
        match ours {
            Some((c, len)) => {
                assert!(valid >= len, "{bytes:x?}");
                let s = core::str::from_utf8(&bytes[..len]).unwrap();
                assert_eq!(s.chars().next().map(|c| c as u32), Some(c), "{bytes:x?}");
                assert_eq!(s.chars().count(), 1);
            }
            None => assert_eq!(valid, 0, "{bytes:x?}"),
        }
    };
    for a in 0..=255u8 {
        check(&[a]);
        for b in 0..=255u8 {
            check(&[a, b]);
            if a >= 0xE0 {
                for c in [0x00, 0x80, 0x9F, 0xA0, 0xBF, 0xC0] {
                    check(&[a, b, c]);
                    if a >= 0xF0 {
                        check(&[a, b, c, 0x80]);
                        check(&[a, b, c, 0xC0]);
                    }
                }
            }
        }
    }
}

#[test]
fn errors() {
    for (src, pos) in [
        ("a = @@ \u{1}", 7),
        ("\\", 0),
        ("a = \u{7F}", 4),
        ("€", 0),
        ("x\u{0}", 1),
    ] {
        let e = match lex(src) {
            Err(e) => e,
            Ok(t) => panic!("{src:?}: {t:?}"),
        };
        assert_eq!(e.pos, pos, "{src:?}");
    }
    assert_eq!(
        err("\\").message,
        "invalid Unicode escape sequence in identifier"
    );
    assert_eq!(format!("{}", err("€")), "unexpected character");
}

#[test]
fn spans_and_text() {
    let src = "let  x =\n'é' + 0x1F;";
    let lx = Lexer::new(src.as_bytes()).unwrap();
    let toks = lex(src).unwrap();
    let texts: Vec<&str> = toks
        .iter()
        .map(|t| core::str::from_utf8(lx.text(t)).unwrap())
        .collect();
    assert_eq!(texts, ["let", "x", "=", "'é'", "+", "0x1F", ";", ""]);
    assert_eq!(toks.last().unwrap().start, src.len() as u32);
    assert_eq!(toks[0].span(), Span { start: 0, end: 3 });
    assert_eq!(toks[0].span().len(), 3);
    assert!(toks[7].span().is_empty());
}

#[test]
fn eof_is_sticky() {
    let mut lx = Lexer::new(b"a").unwrap();
    assert_eq!(lx.next_token().unwrap().kind, Ident);
    for _ in 0..3 {
        let t = lx.next_token().unwrap();
        assert_eq!((t.kind, t.start, t.end), (Eof, 1, 1));
    }
    assert_eq!(kinds(""), []);
    assert_eq!(kinds("  \n "), []);
}

#[test]
fn checkpoint_and_rewind() {
    let mut lx = Lexer::new(b"(a, b) => a / 2").unwrap();
    let lparen = lx.next_token().unwrap();
    assert_eq!(lparen.kind, LParen);
    // Look ahead for `=>` by scanning a copy, then rewind.
    let checkpoint = lx;
    let mut depth = 1;
    while depth > 0 {
        match lx.next_token().unwrap().kind {
            LParen => depth += 1,
            RParen => depth -= 1,
            _ => {}
        }
    }
    assert_eq!(lx.next_token().unwrap().kind, Arrow);
    lx = checkpoint;
    assert_eq!(lx.next_token().unwrap().kind, Ident);
    assert_eq!(core::mem::size_of::<Token>(), 12);
}

// ---------------------------------------------------------------------------------
// Line table

#[test]
fn line_table() {
    let src = "ab\ncd\r\nef\rgh\u{2028}ij\u{2029}k\n";
    let t = LineTable::new(src.as_bytes());
    assert_eq!(t.line_count(), 7);
    let pos = |s: &str| src.find(s).unwrap() as u32;
    assert_eq!(t.line_col(src.as_bytes(), 0), (1, 1));
    assert_eq!(t.line_col(src.as_bytes(), 1), (1, 2));
    assert_eq!(t.line_col(src.as_bytes(), 2), (1, 3)); // the LF itself
    assert_eq!(t.line_col(src.as_bytes(), pos("cd")), (2, 1));
    assert_eq!(t.line_col(src.as_bytes(), pos("\r\n") + 1), (2, 4)); // inside CRLF
    assert_eq!(t.line_col(src.as_bytes(), pos("ef")), (3, 1));
    assert_eq!(t.line_col(src.as_bytes(), pos("gh")), (4, 1));
    assert_eq!(t.line_col(src.as_bytes(), pos("ij")), (5, 1));
    assert_eq!(t.line_col(src.as_bytes(), pos("k")), (6, 1));
    assert_eq!(t.line_col(src.as_bytes(), src.len() as u32), (7, 1));
    assert_eq!(t.line_col(src.as_bytes(), 1000), (7, 1));
    assert_eq!(t.line_start(2), 3);
    assert_eq!(t.line_start(0), 0);
    assert_eq!(t.line_start(100), src.len() as u32);

    // Columns count code points (like QuickJS), so `@` here is at column 7.
    let src = "\"😀😀\"; @";
    let t = LineTable::new(src.as_bytes());
    assert_eq!(
        t.line_col(src.as_bytes(), src.find('@').unwrap() as u32),
        (1, 7)
    );
    let t = LineTable::new(b"");
    assert_eq!((t.line_count(), t.line_col(b"", 0)), (1, (1, 1)));
}

#[test]
fn line_table_word_scan() {
    // Line breaks at every offset of the eight-byte words the scan skips by, in ASCII
    // and non-ASCII sources: the same lines and columns as a byte-by-byte scan.
    for unit in ["abcdefghij", "aé€😀bcdefg"] {
        for k in 1..20 {
            let mut src = String::new();
            for i in 0..40 {
                src.push_str(&unit[..unit.char_indices().nth(i % 7).map_or(unit.len(), |c| c.0)]);
                src.push_str(["\n", "\r", "\r\n", "\u{2028}"][(i * k) % 4]);
            }
            let b = src.as_bytes();
            let t = LineTable::new(b);
            let mut line = 1;
            let mut col = 1;
            let mut p = 0;
            while p <= b.len() {
                assert_eq!(t.line_col(b, p as u32), (line, col), "{k} at {p}");
                let c = src[p..].chars().next();
                let n = c.map_or(1, char::len_utf8);
                match c {
                    Some('\n' | '\u{2028}') => (line, col) = (line + 1, 1),
                    Some('\r') if b.get(p + 1) != Some(&b'\n') => (line, col) = (line + 1, 1),
                    _ => col += 1,
                }
                p += n;
            }
        }
    }
}

#[test]
fn line_cursor_matches_line_col() {
    use crate::lexer::LineCursor;
    let src = "ab\ncd\r\nef\rgh\u{2028}ij\u{2029}k\n\"😀😀\"; @ x\n\nlast";
    let b = src.as_bytes();
    let t = LineTable::new(b);
    let n = b.len() as u32 + 3;
    // Increasing, decreasing and jumping offsets, all through one cursor.
    let mut order: Vec<u32> = (0..n).collect();
    order.extend((0..n).rev());
    order.extend((0..n).map(|i| (i * 7) % n));
    let mut c = LineCursor::default();
    for pos in order {
        assert_eq!(
            t.line_col_from(b, pos, &mut c),
            t.line_col(b, pos),
            "at {pos}"
        );
    }
}

// ---------------------------------------------------------------------------------
// A realistic snippet, end to end.

#[test]
fn snippet() {
    let src = r#"
        async function main(tools) {
            const { items } = await tools.list({ limit: 10_000 });
            let out = `found ${items.length} items:\n`;
            for (const it of items) out += `- ${it.name ?? "?"} (${it.size >>> 0}b)\n`;
            return out.trim();
        }
    "#;
    let ks = kinds(src);
    assert_eq!(ks[0], Ident); // async
    assert_eq!(ks[1], Function);
    assert!(ks.contains(&TemplateHead) && ks.contains(&TemplateTail));
    assert_eq!(ks.iter().filter(|&&k| k == Gt).count(), 3);
    let toks = lex(src).unwrap();
    assert!(toks[0].is_contextual(Kw::Async));
    assert!(toks[0].newline_before());
}

// ---------------------------------------------------------------------------------
// test262 corpus and throughput (ignored: they need vendor/test262 and take a while).

/// A rough stand-in for the parser's regex-vs-division decision: `/` starts a regex
/// unless the previous token ends an expression. `)` ends one unless it closes the
/// head of `if`/`while`/`for`/`with`; `}` is assumed to close a block.
struct Driver<'a> {
    lx: Lexer<'a>,
    prev: Option<Token>,
    parens: Vec<bool>,
    last_paren_was_head: bool,
    templates: Vec<usize>,
    depth: usize,
}

impl<'a> Driver<'a> {
    fn new(src: &'a [u8]) -> Result<Self, LexError> {
        Ok(Driver {
            lx: Lexer::new(src)?,
            prev: None,
            parens: Vec::new(),
            last_paren_was_head: false,
            templates: Vec::new(),
            depth: 0,
        })
    }

    fn regex_allowed(&self) -> bool {
        let Some(prev) = self.prev else { return true };
        match prev.kind {
            Ident => matches!(prev.kw, Kw::Yield | Kw::Await) && !prev.has_escape(),
            RParen => self.last_paren_was_head,
            Number | BigInt | String | NoSubstTemplate | TemplateTail | Regex | PrivateName
            | RBracket | This | Super | True | False | Null => false,
            PlusPlus | MinusMinus => false,
            _ => true,
        }
    }

    fn next(&mut self) -> Result<Token, LexError> {
        let mut tok = self.lx.next_token()?;
        match tok.kind {
            Slash | SlashAssign if self.regex_allowed() => tok = self.lx.rescan_as_regex(tok)?,
            Gt => tok = self.lx.rescan_greater(tok),
            LBrace => self.depth += 1,
            RBrace if self.templates.last() == Some(&self.depth) => {
                tok = self.lx.rescan_template_continuation(tok)?;
                if tok.kind == TemplateTail {
                    self.templates.pop();
                }
            }
            RBrace => self.depth = self.depth.saturating_sub(1),
            TemplateHead => self.templates.push(self.depth),
            LParen => {
                let head = self
                    .prev
                    .is_some_and(|p| matches!(p.kind, If | While | For | With));
                self.parens.push(head);
            }
            RParen => self.last_paren_was_head = self.parens.pop().unwrap_or(false),
            _ => {}
        }
        self.prev = Some(tok);
        Ok(tok)
    }
}

/// Lexes `src` completely with the driver; returns the number of tokens.
fn lex_with_driver(src: &[u8]) -> Result<usize, LexError> {
    let mut d = Driver::new(src)?;
    let mut n = 0;
    let mut buf16 = Vec::new();
    let mut buf8 = Vec::new();
    loop {
        let tok = d.next()?;
        // Cook everything, to exercise the value paths on real code.
        buf16.clear();
        match tok.kind {
            Eof => return Ok(n),
            String => {
                let latin1 = d.lx.cook_string(&tok, &mut buf16);
                assert_eq!(latin1, buf16.iter().all(|&u| u <= 0xFF));
                if !tok.has_escape() {
                    let content = d.lx.string_content(&tok);
                    assert_eq!(
                        buf16,
                        std::str::from_utf8(content)
                            .unwrap()
                            .encode_utf16()
                            .collect::<Vec<_>>()
                    );
                }
            }
            NoSubstTemplate | TemplateHead | TemplateMiddle | TemplateTail => {
                let cooked = d.lx.cook_template(&tok, &mut buf16);
                assert_eq!(cooked.is_err(), tok.has_flag(Token::INVALID_ESCAPE));
                d.lx.template_raw(&tok, &mut buf16);
            }
            k if k.is_identifier_name() || k == PrivateName => {
                assert!(std::str::from_utf8(d.lx.ident_value(&tok, &mut buf8)).is_ok());
            }
            _ => {}
        }
        n += 1;
    }
}

fn test262_dir() -> Option<std::path::PathBuf> {
    if let Some(dir) = std::env::var_os("DJS_TEST262") {
        return Some(dir.into());
    }
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()?;
    let common = std::path::PathBuf::from(std::string::String::from_utf8(out.stdout).ok()?.trim());
    let dir = common.parent()?.join("vendor/test262");
    dir.is_dir().then_some(dir)
}

fn collect_js(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_js(&path, out);
        } else if path.extension().is_some_and(|e| e == "js") {
            out.push(path);
        }
    }
}

/// Lexes every file in test262's `test/` and `harness/` directories. Must not panic.
/// Lexing errors are expected in negative tests and sloppy-mode tests; the remaining
/// ones are listed (they come from the driver's regex heuristic, or are real bugs).
#[test]
#[ignore]
fn lex_test262() {
    let Some(root) = test262_dir() else {
        std::println!("vendor/test262 not found; skipping");
        return;
    };
    let mut files = Vec::new();
    collect_js(&root.join("test"), &mut files);
    collect_js(&root.join("harness"), &mut files);
    files.sort();
    assert!(!files.is_empty());
    let (mut ok, mut negative, mut sloppy, mut unexpected) = (0, 0, 0, Vec::new());
    for path in &files {
        let src = std::fs::read(path).unwrap();
        // Must not panic, whatever the outcome.
        let result = std::panic::catch_unwind(|| lex_with_driver(&src));
        let Ok(result) = result else {
            panic!("lexer panicked on {}", path.display());
        };
        let Err(e) = result else {
            ok += 1;
            continue;
        };
        let text = std::string::String::from_utf8_lossy(&src);
        if text.contains("negative:") {
            negative += 1;
            if !text.contains("phase: parse") {
                let rel = path.strip_prefix(&root).unwrap_or(path);
                unexpected.push(format!(
                    "{}:{}: {} (not a parse-phase negative)",
                    rel.display(),
                    e.pos,
                    e.message
                ));
            }
        } else if text.contains("noStrict") || text.contains("flags: [raw]") {
            sloppy += 1;
            if std::env::var_os("DJS_LEX_LIST_NEGATIVE").is_some() {
                std::println!("sloppy error: {}: {}", path.display(), e.message);
            }
        } else {
            let rel = path.strip_prefix(&root).unwrap_or(path);
            unexpected.push(format!("{}:{}: {}", rel.display(), e.pos, e.message));
        }
    }
    // Optional: list negative tests in lexical directories that lex without error, to
    // review by hand (most are parser-level errors).
    if std::env::var_os("DJS_LEX_LIST_NEGATIVE").is_some() {
        for path in &files {
            let src = std::fs::read(path).unwrap();
            let text = std::string::String::from_utf8_lossy(&src);
            if text.contains("negative:") && lex_with_driver(&src).is_ok() {
                std::println!(
                    "negative ok: {}",
                    path.strip_prefix(&root).unwrap_or(path).display()
                );
            }
        }
    }
    std::println!(
        "{} files: {ok} ok, {negative} negative with errors, {sloppy} sloppy with errors, {} unexpected",
        files.len(),
        unexpected.len()
    );
    for u in &unexpected {
        std::println!("  {u}");
    }
    // Where the driver's heuristic guesses wrong (`{...} / 1` is a division).
    const HEURISTIC: &[&str] = &[
        "test/language/expressions/division/S11.5.2_A2.2_T1.js",
        "test/language/expressions/division/S11.5.2_A3_T1.5.js",
        "test/language/expressions/division/bigint-toprimitive.js",
        "test/language/expressions/division/bigint-wrapped-values.js",
    ];
    unexpected.retain(|u| !HEURISTIC.iter().any(|h| u.starts_with(h)));
    assert!(unexpected.is_empty(), "unexpected lexing errors");
}

/// Throughput (run with `--release --ignored`). Lexes the files listed in
/// `DJS_LEX_BENCH` (separated by `:`) one by one, or else all of test262 concatenated.
#[test]
#[ignore]
fn lex_throughput() {
    let inputs: Vec<(std::string::String, Vec<u8>)> = match std::env::var("DJS_LEX_BENCH") {
        Ok(list) => list
            .split(':')
            .map(|p| (p.into(), std::fs::read(p).unwrap()))
            .collect(),
        Err(_) => {
            let Some(root) = test262_dir() else {
                std::println!("vendor/test262 not found; skipping");
                return;
            };
            let mut files = Vec::new();
            collect_js(&root.join("test"), &mut files);
            files.sort();
            // Only files that lex cleanly, so the measurement covers whole files.
            let mut corpus = Vec::new();
            for path in &files {
                let src = std::fs::read(path).unwrap();
                if lex_with_driver(&src).is_ok() {
                    corpus.extend_from_slice(&src);
                    corpus.push(b'\n');
                }
            }
            vec![("test262 (concatenated)".into(), corpus)]
        }
    };
    for (name, src) in &inputs {
        let mut tokens = 0;
        let iters = 10;
        let start = std::time::Instant::now();
        for _ in 0..iters {
            tokens += lex_whole(src);
        }
        let secs = start.elapsed().as_secs_f64();
        let mb = (src.len() * iters) as f64 / 1e6;
        std::println!(
            "{name}: {:.1} MB, {} tokens: {:.0} MB/s, {:.1} ns/token",
            src.len() as f64 / 1e6,
            tokens / iters,
            mb / secs,
            secs * 1e9 / tokens as f64
        );
    }
}

/// Lexes a concatenation of files, resynchronizing after errors (concatenation can
/// break the regex heuristic across file boundaries).
fn lex_whole(src: &[u8]) -> usize {
    let mut n = 0;
    let mut errors = 0;
    let mut d = Driver::new(src).unwrap();
    loop {
        match d.next() {
            Ok(t) if t.kind == Eof => {
                if errors > 0 {
                    std::println!("({errors} lexing errors, resynchronized)");
                }
                return n;
            }
            Ok(_) => n += 1,
            Err(e) => {
                errors += 1;
                // Skip to the next line.
                let mut p = e.pos as usize + 1;
                while p < src.len() && src[p] != b'\n' {
                    p += 1;
                }
                d.lx.pos = p as u32;
                d.templates.clear();
            }
        }
    }
}

// ---------------------------------------------------------------------------------
// Robustness: random inputs built from interesting fragments must never panic, and
// every successfully scanned token must cook consistently.

#[test]
fn random_inputs_do_not_panic() {
    const FRAGMENTS: &[&[u8]] = &[
        b"a",
        b"if",
        b"let",
        b"\\u0061",
        b"\\u{62}",
        b"\\u",
        b"\\",
        b"#",
        b"#x",
        b"0",
        b"1",
        b"0x",
        b"0b1",
        b"_",
        b".",
        b"e",
        b"n",
        b"'",
        b"\"",
        b"`",
        b"${",
        b"}",
        b"{",
        b"/",
        b"*",
        b"/*",
        b"*/",
        b"//",
        b"\n",
        b"\r",
        b"\r\n",
        b" ",
        b"\t",
        b"\\x4",
        b"\\0",
        b"\\8",
        b"[",
        b"]",
        b"(",
        b")",
        b">",
        b"=",
        b"?",
        b"?.",
        b"!",
        b"<",
        b"-",
        "é".as_bytes(),
        "😀".as_bytes(),
        "\u{2028}".as_bytes(),
        "\u{A0}".as_bytes(),
        "\u{FEFF}".as_bytes(),
        b"\xFF",
        b"\xE2\x80",
        b"\x00",
        b"g",
        b"u",
        b"v",
    ];
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut src = Vec::new();
    let mut tokens = 0;
    for _ in 0..50_000 {
        src.clear();
        for _ in 0..(rand() % 12) {
            src.extend_from_slice(FRAGMENTS[(rand() % FRAGMENTS.len() as u64) as usize]);
        }
        let Ok(mut d) = Driver::new(&src) else {
            continue;
        };
        for _ in 0..100 {
            let Ok(tok) = d.next() else { break };
            tokens += 1;
            assert!(tok.start <= tok.end && tok.end as usize <= src.len());
            assert!(d.lx.pos() >= tok.end);
            let lx = d.lx;
            match tok.kind {
                Eof => break,
                k if k.is_identifier_name() || k == PrivateName => {
                    let mut buf = Vec::new();
                    let name = lx.ident_value(&tok, &mut buf);
                    assert!(core::str::from_utf8(name).is_ok());
                }
                String => {
                    lx.cook_string(&tok, &mut Vec::new());
                }
                NoSubstTemplate | TemplateHead | TemplateMiddle | TemplateTail => {
                    let cooked = lx.cook_template(&tok, &mut Vec::new());
                    assert_eq!(cooked.is_err(), tok.has_flag(Token::INVALID_ESCAPE));
                    lx.template_raw(&tok, &mut Vec::new());
                }
                Number | BigInt => {
                    let lit = lx.number_literal(&tok);
                    assert!(!lit.digits.is_empty());
                }
                Regex => {
                    let (body, flags) = lx.regex_parts(&tok);
                    assert!(body.end < flags.start || body.is_empty());
                }
                _ => {}
            }
        }
    }
    // Make sure the inputs are not all rejected immediately.
    assert!(tokens > 50_000, "{tokens}");
}
