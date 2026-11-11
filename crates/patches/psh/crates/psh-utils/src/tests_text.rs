//! The text filters on the test host. Behavior against GNU and BSD is L4.

use super::{check, check_all};

#[test]
fn tee() {
    check("echo hi | tee /tmp/o; cat /tmp/o", "hi\nhi\n");
    check(
        "echo a | tee /tmp/o >/dev/null; echo b | tee -a /tmp/o >/dev/null; cat /tmp/o",
        "a\nb\n",
    );
    check_all(
        "echo x | tee /nodir/o",
        "",
        1,
        "x\n",
        "tee: /nodir/o: No such file or directory\n",
    );
    // A closed stdout ends tee as SIGPIPE would.
    check("yes | tee /tmp/o | head -n 1", "y\n");
}

#[test]
fn seq() {
    check("seq 3", "1\n2\n3\n");
    check("seq -w 9 11", "09\n10\n11\n");
    check("seq -s, 1 3", "1,2,3\n");
    check("seq -w -2 0", "-2\n-1\n00\n");
    check("seq 0 0.1 0.3", "0.0\n0.1\n0.2\n0.3\n");
    check("seq -f 'n%g' 2", "n1\nn2\n");
    check("seq 5 -2 1", "5\n3\n1\n");
    check("seq inf | head -n 2", "1\n2\n");
    check("seq -t . 2", "1\n2.");
    check_all(
        "seq 1 0 2",
        "",
        1,
        "",
        "seq: invalid Zero increment value: '0'\nTry 'seq --help' for more information.\n",
    );
}

#[test]
fn tr() {
    check("echo abc | tr a-c A-C", "ABC\n");
    check("echo 'a  b' | tr -s ' '", "a b\n");
    check("echo 'a1b2' | tr -d '[:digit:]'", "ab\n");
    check("echo 'ab-12' | tr -cd 'a-z'", "ab");
    check("echo 'abc' | tr -c 'a\\n' 'x'", "axx\n");
    check("printf 'caf\\303\\251\\n' | tr '\\303\\251' 'e'", "cafee\n");
    check("printf 'caf\\303\\251\\n' | tr 'é' 'e'", "cafe\n");
    check("echo 'aé' | tr -d 'é'", "a\n");
    check_all(
        "tr --frobnicate a b",
        "",
        2,
        "",
        "tr: unsupported option '--frobnicate' (psh built-in tr)\n",
    );
}

#[test]
fn cut() {
    check("cut -d' ' -f2 g", "b\n");
    check("cut -c2-3 f", "ne\nwo\nhr\nou\n");
    check("echo 'héllo' | cut -c1-2", "hé\n");
    check("echo 'héllo' | cut -b1-2 | wc -c", "3\n");
    check("echo a:b:c | cut -d: -f1,3 --output-delimiter=-", "a-c\n");
    check("echo a:b:c | cut -d: --complement -f1", "b:c\n");
    check("printf 'a  b\\n' | cut -w -f2", "b\n");
    check_all(
        "cut -c1 -d: g",
        "",
        1,
        "",
        "cut: an input delimiter may be specified only when operating on fields\nTry 'cut --help' for more information.\n",
    );
}

#[test]
fn paste() {
    check("paste -sd+ f", "one+two+three+four\n");
    check("paste g nonl", "a b c\tx\n\ty\n");
    check("paste - - < f", "one\ttwo\nthree\tfour\n");
    check("paste -d, -s nonl g", "x,y\na b c\n");
}

#[test]
fn uniq() {
    check("printf 'a\\na\\nb\\n' | uniq", "a\nb\n");
    check("printf 'a\\na\\nb\\n' | uniq -c", "      2 a\n      1 b\n");
    check("printf 'a\\na\\nb\\n' | uniq -d", "a\n");
    check("printf 'a\\na\\nb\\n' | uniq -u", "b\n");
    check(
        "printf 'a\\na\\nb\\nb\\n' | uniq --all-repeated=prepend",
        "\na\na\n\nb\nb\n",
    );
    check(
        "printf 'a\\na\\nb\\n' | uniq --group=append",
        "a\na\n\nb\n\n",
    );
    check(
        "printf 'a\\na\\nb\\n' | uniq - /tmp/o; cat /tmp/o",
        "a\nb\n",
    );
    check("printf '1 x\\n2 x\\n' | uniq -f 1 -c", "      2 1 x\n");
}

#[test]
fn comm() {
    check(
        "printf 'a\\nb\\n' > /tmp/x; printf 'b\\nc\\n' | comm /tmp/x -",
        "a\n\t\tb\n\tc\n",
    );
    check(
        "printf 'a\\nb\\n' > /tmp/x; printf 'b\\nc\\n' | comm -12 /tmp/x -",
        "b\n",
    );
    check_all(
        "printf 'b\\na\\n' > /tmp/x; printf 'c\\n' | comm --check-order /tmp/x -",
        "",
        1,
        "b\n",
        "comm: file 1 is not in sorted order\n",
    );
}

#[test]
fn nl() {
    check("nl g", "     1\ta b c\n");
    check("printf 'a\\n\\nb\\n' | nl -ba -w1 -s' '", "1 a\n2 \n3 b\n");
    // A regular expression style has no implementation yet: the real nl, or an error.
    check_all(
        "nl -bpx g",
        "",
        2,
        "",
        "nl: unsupported numbering style 'pREGEX' (psh built-in nl)\n",
    );
}

#[test]
fn sort() {
    check("sort f", "four\none\nthree\ntwo\n");
    check("sort -r f", "two\nthree\none\nfour\n");
    check("printf 'b\\na' | sort", "a\nb\n");
    check("printf '2\\n10\\n1\\n' | sort -n", "1\n2\n10\n");
    check("sort -o f f; cat f", "four\none\nthree\ntwo\n");
    // Byte order whatever the locale (D1160).
    check(
        "printf 'b\\nB\\na\\n' | LC_ALL=en_US.UTF-8 sort",
        "B\na\nb\n",
    );
    check_all(
        "printf 'a\\nc\\nb\\n' | sort -c",
        "",
        1,
        "",
        "sort: -:3: disorder: b\n",
    );
    check_all(
        "sort --debug f",
        "",
        2,
        "",
        "sort: unsupported option '--debug' (psh built-in sort)\n",
    );
}

#[test]
fn unknown_options_fail_clearly() {
    // The test host has none of these programs, so there is nothing to fall through to.
    for name in [
        "sort", "uniq", "cut", "tr", "paste", "nl", "tee", "seq", "comm",
    ] {
        check_all(
            &format!("{name} --frobnicate < /dev/null"),
            "",
            2,
            "",
            &format!("{name}: unsupported option '--frobnicate' (psh built-in {name})\n"),
        );
    }
}

#[test]
fn messages_quote_as_gnu_does_for_the_locale() {
    check_all(
        "LC_ALL=C.UTF-8 cut -f x < /dev/null",
        "",
        1,
        "",
        "cut: invalid field value \u{2018}x\u{2019}\nTry 'cut --help' for more information.\n",
    );
    check_all(
        "LANG=C cut -f x < /dev/null",
        "",
        1,
        "",
        "cut: invalid field value 'x'\nTry 'cut --help' for more information.\n",
    );
}
