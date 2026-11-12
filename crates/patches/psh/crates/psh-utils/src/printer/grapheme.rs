use psh_pattern::bytes::Regex;

const GRAPHEME: &str = r"(?x)
^(?:
\p{gcb=CR} \p{gcb=LF}
|
\p{gcb=Control}
|
\p{gcb=Prepend}*
(
  (
    (\p{gcb=L}* (\p{gcb=V}+ | \p{gcb=LV} \p{gcb=V}* | \p{gcb=LVT}) \p{gcb=T}*)
    |
    \p{gcb=L}+
    |
    \p{gcb=T}+
  )
  |
  \p{gcb=RI} \p{gcb=RI}
  |
  \p{Extended_Pictographic} (\p{gcb=Extend}* \p{gcb=ZWJ} \p{Extended_Pictographic})*
  |
  [^\p{gcb=Control} \p{gcb=CR} \p{gcb=LF}]
)
[\p{gcb=Extend} \p{gcb=ZWJ} \p{gcb=SpacingMark}]*
|
\p{any}
)";

thread_local! {
    static GRAPHEMES: Regex = Regex::new(GRAPHEME).expect("the grapheme pattern compiles");
}

fn grapheme_len(bytes: &[u8], regex: &Regex) -> usize {
    if bytes.len() >= 2
        && bytes[0].is_ascii()
        && bytes[1].is_ascii()
        && !bytes[0].is_ascii_whitespace()
    {
        return 1;
    }
    if let Some(found) = regex.find(bytes) {
        assert!(found.start() == 0 && found.end() > 0);
        return found.end();
    }
    let invalid = bytes
        .utf8_chunks()
        .next()
        .map_or(0, |chunk| chunk.invalid().len());
    assert!(invalid > 0);
    invalid
}

pub(crate) fn end_of_graphemes(bytes: &[u8], count: usize) -> usize {
    GRAPHEMES.with(|regex| {
        let mut end = 0;
        for _ in 0..count {
            if end == bytes.len() {
                break;
            }
            end += grapheme_len(&bytes[end..], regex);
        }
        end
    })
}
