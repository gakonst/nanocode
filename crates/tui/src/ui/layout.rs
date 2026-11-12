use super::solver::{Constraint as SolverConstraint, REQUIRED, Relation, Solver};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Position {
    pub x: u16,
    pub y: u16,
}

impl Position {
    #[must_use]
    pub const fn new(x: u16, y: u16) -> Self {
        Self { x, y }
    }
}

impl From<(u16, u16)> for Position {
    fn from((x, y): (u16, u16)) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl Rect {
    pub const ZERO: Self = Self {
        x: 0,
        y: 0,
        width: 0,
        height: 0,
    };

    #[must_use]
    pub const fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width: x.saturating_add(width) - x,
            height: y.saturating_add(height) - y,
        }
    }

    #[must_use]
    pub const fn area(self) -> u32 {
        self.width as u32 * self.height as u32
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }

    #[must_use]
    pub const fn left(self) -> u16 {
        self.x
    }

    #[must_use]
    pub const fn right(self) -> u16 {
        self.x.saturating_add(self.width)
    }

    #[must_use]
    pub const fn top(self) -> u16 {
        self.y
    }

    #[must_use]
    pub const fn bottom(self) -> u16 {
        self.y.saturating_add(self.height)
    }

    #[must_use]
    pub fn intersection(self, other: Self) -> Self {
        let left = self.x.max(other.x);
        let top = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        Self {
            x: left,
            y: top,
            width: right.saturating_sub(left),
            height: bottom.saturating_sub(top),
        }
    }

    #[must_use]
    pub const fn contains(self, position: Position) -> bool {
        position.x >= self.x
            && position.x < self.right()
            && position.y >= self.y
            && position.y < self.bottom()
    }

    pub fn rows(self) -> impl DoubleEndedIterator<Item = Self> {
        (self.top()..self.bottom()).map(move |y| Self {
            y,
            height: 1,
            ..self
        })
    }

    #[must_use]
    pub(crate) const fn indent_x(self, offset: u16) -> Self {
        Self {
            x: self.x.saturating_add(offset),
            width: self.width.saturating_sub(offset),
            ..self
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum Alignment {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Constraint {
    Min(u16),
    Length(u16),
}

pub struct Layout<const N: usize> {
    constraints: [Constraint; N],
}

const PRECISION: f64 = 100.0;
const SPACER_SIZE_EQ: f64 = REQUIRED / 10.0;
const EMPTY_EQ: f64 = REQUIRED - WEAK;
const MIN_SIZE_GE: f64 = STRONG * 100.0;
const LENGTH_SIZE_EQ: f64 = STRONG * 10.0;
const FILL_GROW: f64 = MEDIUM;
const GROW: f64 = MEDIUM / 10.0;
const ALL_SEGMENT_GROW: f64 = WEAK;
const STRONG: f64 = 1_000_000.0;
const MEDIUM: f64 = 1_000.0;
const WEAK: f64 = 1.0;

#[derive(Clone, Copy)]
struct Element {
    start: usize,
    end: usize,
}

impl Element {
    fn size(self) -> Vec<(usize, f64)> {
        vec![(self.end, 1.0), (self.start, -1.0)]
    }

    fn sized(self, size: f64, relation: Relation, strength: f64) -> SolverConstraint {
        SolverConstraint {
            terms: self.size(),
            constant: -size,
            relation,
            strength,
        }
    }

    fn sized_as(self, other: Self, strength: f64) -> SolverConstraint {
        let mut terms = self.size();
        terms.extend(
            other
                .size()
                .into_iter()
                .map(|(variable, coefficient)| (variable, -coefficient)),
        );
        SolverConstraint {
            terms,
            constant: 0.0,
            relation: Relation::Equal,
            strength,
        }
    }
}

fn between(left: usize, relation: Relation, right: usize) -> SolverConstraint {
    SolverConstraint {
        terms: vec![(left, 1.0), (right, -1.0)],
        constant: 0.0,
        relation,
        strength: REQUIRED,
    }
}

impl<const N: usize> Layout<N> {
    #[must_use]
    pub const fn vertical(constraints: [Constraint; N]) -> Self {
        Self { constraints }
    }

    #[must_use]
    pub fn areas(&self, area: Rect) -> [Rect; N] {
        let variables = N * 2 + 2;
        let last = variables - 1;
        let mut spacers = (0..=N).map(|index| Element {
            start: index * 2,
            end: index * 2 + 1,
        });
        let segments = (0..N).map(|index| Element {
            start: index * 2 + 1,
            end: index * 2 + 2,
        });
        let whole = Element {
            start: 0,
            end: last,
        };
        let mut constraints = vec![
            SolverConstraint {
                terms: vec![(0, 1.0)],
                constant: -f64::from(area.top()) * PRECISION,
                relation: Relation::Equal,
                strength: REQUIRED,
            },
            SolverConstraint {
                terms: vec![(last, 1.0)],
                constant: -f64::from(area.bottom()) * PRECISION,
                relation: Relation::Equal,
                strength: REQUIRED,
            },
        ];
        for variable in 0..variables {
            constraints.push(between(variable, Relation::GreaterOrEqual, 0));
            constraints.push(between(variable, Relation::LessOrEqual, last));
        }
        for left in (1..last).step_by(2) {
            constraints.push(between(left, Relation::LessOrEqual, left + 1));
        }
        for spacer in spacers.clone().skip(1).take(N.saturating_sub(1)) {
            constraints.push(spacer.sized(0.0, Relation::Equal, SPACER_SIZE_EQ));
        }
        constraints.push(Element { start: 0, end: 1 }.sized(0.0, Relation::Equal, EMPTY_EQ));
        if let Some(final_spacer) = spacers.next_back() {
            constraints.push(final_spacer.sized_as(whole, GROW));
        }
        for (constraint, segment) in self.constraints.iter().zip(segments.clone()) {
            match *constraint {
                Constraint::Min(size) => {
                    constraints.push(segment.sized(
                        f64::from(size) * PRECISION,
                        Relation::GreaterOrEqual,
                        MIN_SIZE_GE,
                    ));
                    constraints.push(segment.sized_as(whole, FILL_GROW));
                }
                Constraint::Length(size) => {
                    constraints.push(segment.sized(
                        f64::from(size) * PRECISION,
                        Relation::Equal,
                        LENGTH_SIZE_EQ,
                    ));
                }
            }
        }
        let filling = self
            .constraints
            .iter()
            .zip(segments.clone())
            .filter(|(constraint, _)| matches!(constraint, Constraint::Min(_)))
            .map(|(_, segment)| segment)
            .collect::<Vec<_>>();
        for (index, left) in filling.iter().enumerate() {
            for right in &filling[index + 1..] {
                constraints.push(left.sized_as(*right, GROW));
            }
        }
        let segment_list = segments.collect::<Vec<_>>();
        for pair in segment_list.windows(2) {
            constraints.push(pair[0].sized_as(pair[1], ALL_SEGMENT_GROW));
        }
        let mut solver = Solver::new(variables);
        for constraint in &constraints {
            solver
                .add(constraint)
                .expect("the layout's constraints are satisfiable and bounded");
        }
        let position = |variable| (solver.value(variable).round() / PRECISION).round() as u16;
        std::array::from_fn(|index| {
            let segment = segment_list[index];
            let start = position(segment.start);
            let end = position(segment.end);
            Rect {
                x: area.x,
                y: start,
                width: area.width,
                height: end.saturating_sub(start),
            }
        })
    }
}
