use syn::punctuated::{Pair, Punctuated};

pub trait PunctuatedExt<T, P> {
    fn remove(&mut self, index: usize) -> Pair<T, P>
    where
        P: Default;
}

impl<T, P> PunctuatedExt<T, P> for Punctuated<T, P> {
    fn remove(&mut self, index: usize) -> Pair<T, P>
    where
        P: Default,
    {
        let mut stack = Vec::new();
        for _ in index + 1..self.len() {
            stack.push(self.pop().unwrap().into_value());
        }
        let removed = self.pop().unwrap();
        while let Some(item) = stack.pop() {
            self.push(item)
        }
        removed
    }
}
