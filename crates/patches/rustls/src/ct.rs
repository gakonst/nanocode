use core::hint::black_box;

pub(crate) struct Choice(u8);

impl From<Choice> for bool {
    fn from(choice: Choice) -> Self {
        black_box(choice.0) != 0
    }
}

pub(crate) trait ConstantTimeEq {
    fn ct_eq(&self, other: &Self) -> Choice;
}

impl ConstantTimeEq for [u8] {
    fn ct_eq(&self, other: &Self) -> Choice {
        if self.len() != other.len() {
            return Choice(0);
        }
        let difference = self
            .iter()
            .zip(other)
            .fold(0u8, |difference, (a, b)| difference | black_box(a ^ b));
        Choice(u8::from(black_box(difference) == 0))
    }
}
