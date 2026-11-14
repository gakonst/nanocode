use std::str::Chars;

pub struct Shlex<'a> {
    characters: Chars<'a>,
    pub line_no: usize,
    pub had_error: bool,
}

struct Unterminated;

impl<'a> Shlex<'a> {
    #[must_use]
    pub fn new(text: &'a str) -> Self {
        Self {
            characters: text.chars(),
            line_no: 1,
            had_error: false,
        }
    }

    fn advance(&mut self) -> Option<char> {
        let character = self.characters.next();
        if character == Some('\n') {
            self.line_no += 1;
        }
        character
    }

    fn first_of_word(&mut self) -> Option<char> {
        loop {
            match self.advance()? {
                ' ' | '\t' | '\n' => {}
                '#' => self.skip_comment(),
                character => return Some(character),
            }
        }
    }

    fn skip_comment(&mut self) {
        while let Some(character) = self.advance() {
            if character == '\n' {
                return;
            }
        }
    }

    fn read_word(&mut self, first: char, word: &mut String) -> Result<(), Unterminated> {
        let mut character = first;
        loop {
            match character {
                '"' => self.read_double_quoted(word)?,
                '\'' => self.read_single_quoted(word)?,
                '\\' => match self.advance().ok_or(Unterminated)? {
                    '\n' => {}
                    escaped => word.push(escaped),
                },
                ' ' | '\t' | '\n' => return Ok(()),
                literal => word.push(literal),
            }
            match self.advance() {
                Some(next) => character = next,
                None => return Ok(()),
            }
        }
    }

    fn read_double_quoted(&mut self, word: &mut String) -> Result<(), Unterminated> {
        loop {
            match self.advance().ok_or(Unterminated)? {
                '"' => return Ok(()),
                '\\' => match self.advance().ok_or(Unterminated)? {
                    '\n' => {}
                    escaped @ ('$' | '`' | '"' | '\\') => word.push(escaped),
                    kept => {
                        word.push('\\');
                        word.push(kept);
                    }
                },
                literal => word.push(literal),
            }
        }
    }

    fn read_single_quoted(&mut self, word: &mut String) -> Result<(), Unterminated> {
        loop {
            match self.advance().ok_or(Unterminated)? {
                '\'' => return Ok(()),
                literal => word.push(literal),
            }
        }
    }
}

impl Iterator for Shlex<'_> {
    type Item = String;

    fn next(&mut self) -> Option<String> {
        let first = self.first_of_word()?;
        let mut word = String::new();
        match self.read_word(first, &mut word) {
            Ok(()) => Some(word),
            Err(Unterminated) => {
                self.had_error = true;
                None
            }
        }
    }
}

#[must_use]
pub fn split(text: &str) -> Option<Vec<String>> {
    let mut words = Shlex::new(text);
    let split = words.by_ref().collect();
    if words.had_error { None } else { Some(split) }
}
