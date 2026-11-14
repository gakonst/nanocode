#[derive(Clone, Copy)]
pub(crate) struct Input<'a> {
    bytes: &'a [u8],
}

impl core::fmt::Debug for Input<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Input").finish()
    }
}

impl<'a> Input<'a> {
    pub(crate) const fn from(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.bytes.len()
    }

    pub(crate) fn read_all<F, R, E>(&self, incomplete_read: E, read: F) -> Result<R, E>
    where
        F: FnOnce(&mut Reader<'a>) -> Result<R, E>,
    {
        let mut input = Reader::new(*self);
        let result = read(&mut input)?;
        if input.at_end() {
            Ok(result)
        } else {
            Err(incomplete_read)
        }
    }

    #[inline]
    pub(crate) fn as_slice_less_safe(&self) -> &'a [u8] {
        self.bytes
    }
}

impl<'a> From<&'a [u8]> for Input<'a> {
    #[inline]
    fn from(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }
}

pub(crate) struct Reader<'a> {
    input: &'a [u8],
    i: usize,
}

impl core::fmt::Debug for Reader<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Reader").finish()
    }
}

impl<'a> Reader<'a> {
    #[inline]
    pub(crate) fn new(input: Input<'a>) -> Self {
        Self {
            input: input.bytes,
            i: 0,
        }
    }

    #[inline]
    pub(crate) fn at_end(&self) -> bool {
        self.i == self.input.len()
    }

    #[inline]
    pub(crate) fn peek(&self, b: u8) -> bool {
        self.input.get(self.i) == Some(&b)
    }

    #[inline]
    pub(crate) fn read_byte(&mut self) -> Result<u8, EndOfInput> {
        match self.input.get(self.i) {
            Some(b) => {
                self.i += 1;
                Ok(*b)
            }
            None => Err(EndOfInput),
        }
    }

    #[inline]
    pub(crate) fn read_bytes(&mut self, num_bytes: usize) -> Result<Input<'a>, EndOfInput> {
        let new_i = self.i.checked_add(num_bytes).ok_or(EndOfInput)?;
        let bytes = self.input.get(self.i..new_i).ok_or(EndOfInput)?;
        self.i = new_i;
        Ok(Input { bytes })
    }

    #[inline]
    pub(crate) fn read_bytes_to_end(&mut self) -> Input<'a> {
        let bytes = &self.input[self.i..];
        self.i = self.input.len();
        Input { bytes }
    }

    pub(crate) fn read_partial<F, R, E>(&mut self, read: F) -> Result<(Input<'a>, R), E>
    where
        F: FnOnce(&mut Reader<'a>) -> Result<R, E>,
    {
        let start = self.i;
        let r = read(self)?;
        Ok((
            Input {
                bytes: &self.input[start..self.i],
            },
            r,
        ))
    }

    #[inline]
    pub(crate) fn skip(&mut self, num_bytes: usize) -> Result<(), EndOfInput> {
        self.read_bytes(num_bytes).map(|_| ())
    }

    #[inline]
    pub(crate) fn skip_to_end(&mut self) {
        self.i = self.input.len();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EndOfInput;

pub(crate) fn read_all_optional<'a, F, R, E>(
    input: Option<Input<'a>>,
    incomplete_read: E,
    read: F,
) -> Result<R, E>
where
    F: FnOnce(Option<&mut Reader<'a>>) -> Result<R, E>,
{
    match input {
        Some(input) => {
            let mut input = Reader::new(input);
            let result = read(Some(&mut input))?;
            if input.at_end() {
                Ok(result)
            } else {
                Err(incomplete_read)
            }
        }
        None => read(None),
    }
}
