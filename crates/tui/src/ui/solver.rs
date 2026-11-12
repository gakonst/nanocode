use std::collections::BTreeMap;

const NEAR_ZERO: f64 = 1e-8;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Kind {
    Invalid,
    External,
    Slack,
    Error,
    Dummy,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Symbol {
    id: usize,
    kind: Kind,
}

const INVALID: Symbol = Symbol {
    id: 0,
    kind: Kind::Invalid,
};

fn near_zero(value: f64) -> bool {
    value.abs() < NEAR_ZERO
}

#[derive(Clone, Debug, Default)]
struct Row {
    cells: BTreeMap<Symbol, f64>,
    constant: f64,
}

impl Row {
    const fn new(constant: f64) -> Self {
        Self {
            cells: BTreeMap::new(),
            constant,
        }
    }

    fn insert_symbol(&mut self, symbol: Symbol, coefficient: f64) {
        match self.cells.get_mut(&symbol) {
            None => {
                if !near_zero(coefficient) {
                    self.cells.insert(symbol, coefficient);
                }
            }
            Some(existing) => {
                *existing += coefficient;
                if near_zero(*existing) {
                    self.cells.remove(&symbol);
                }
            }
        }
    }

    fn insert_row(&mut self, other: &Self, coefficient: f64) {
        self.constant += other.constant * coefficient;
        for (symbol, value) in &other.cells {
            self.insert_symbol(*symbol, value * coefficient);
        }
    }

    fn reverse_sign(&mut self) {
        self.constant = -self.constant;
        for value in self.cells.values_mut() {
            *value = -*value;
        }
    }

    fn solve_for_symbol(&mut self, symbol: Symbol) {
        let removed = self
            .cells
            .remove(&symbol)
            .expect("a row is solved for a symbol it holds");
        let coefficient = -1.0 / removed;
        self.constant *= coefficient;
        for value in self.cells.values_mut() {
            *value *= coefficient;
        }
    }

    fn solve_for_symbols(&mut self, left: Symbol, right: Symbol) {
        self.insert_symbol(left, -1.0);
        self.solve_for_symbol(right);
    }

    fn coefficient_for(&self, symbol: Symbol) -> f64 {
        self.cells.get(&symbol).copied().unwrap_or(0.0)
    }

    fn substitute(&mut self, symbol: Symbol, row: &Self) {
        if let Some(coefficient) = self.cells.remove(&symbol) {
            self.insert_row(row, coefficient);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Relation {
    Equal,
    LessOrEqual,
    GreaterOrEqual,
}

pub(crate) struct Constraint {
    pub(crate) terms: Vec<(usize, f64)>,
    pub(crate) constant: f64,
    pub(crate) relation: Relation,
    pub(crate) strength: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SolveError {
    Unsatisfiable,
    Unbounded,
}

struct Tag {
    marker: Symbol,
    other: Symbol,
}

pub(crate) const REQUIRED: f64 = 1_001_001_000.0;

pub(crate) struct Solver {
    rows: BTreeMap<Symbol, Row>,
    objective: Row,
    artificial: Option<Row>,
    variables: Vec<Option<Symbol>>,
    tick: usize,
}

impl Solver {
    pub(crate) fn new(variables: usize) -> Self {
        Self {
            rows: BTreeMap::new(),
            objective: Row::new(0.0),
            artificial: None,
            variables: vec![None; variables],
            tick: 1,
        }
    }

    const fn symbol(&mut self, kind: Kind) -> Symbol {
        let symbol = Symbol {
            id: self.tick,
            kind,
        };
        self.tick += 1;
        symbol
    }

    fn variable_symbol(&mut self, variable: usize) -> Symbol {
        if let Some(symbol) = self.variables[variable] {
            return symbol;
        }
        let symbol = self.symbol(Kind::External);
        self.variables[variable] = Some(symbol);
        symbol
    }

    pub(crate) fn value(&self, variable: usize) -> f64 {
        self.variables[variable]
            .and_then(|symbol| self.rows.get(&symbol))
            .map_or(0.0, |row| row.constant)
    }

    pub(crate) fn add(&mut self, constraint: &Constraint) -> Result<(), SolveError> {
        let (mut row, tag) = self.create_row(constraint);
        let mut subject = choose_subject(&row, &tag);
        if subject.kind == Kind::Invalid
            && row.cells.keys().all(|symbol| symbol.kind == Kind::Dummy)
        {
            if !near_zero(row.constant) {
                return Err(SolveError::Unsatisfiable);
            }
            subject = tag.marker;
        }
        if subject.kind == Kind::Invalid {
            if !self.add_with_artificial(&row)? {
                return Err(SolveError::Unsatisfiable);
            }
        } else {
            row.solve_for_symbol(subject);
            self.substitute(subject, &row);
            self.rows.insert(subject, row);
        }
        self.optimize(false)
    }

    fn create_row(&mut self, constraint: &Constraint) -> (Row, Tag) {
        let mut row = Row::new(constraint.constant);
        for &(variable, coefficient) in &constraint.terms {
            if near_zero(coefficient) {
                continue;
            }
            let symbol = self.variable_symbol(variable);
            match self.rows.get(&symbol) {
                Some(other) => row.insert_row(other, coefficient),
                None => row.insert_symbol(symbol, coefficient),
            }
        }
        let tag = match constraint.relation {
            Relation::GreaterOrEqual | Relation::LessOrEqual => {
                let coefficient = if constraint.relation == Relation::LessOrEqual {
                    1.0
                } else {
                    -1.0
                };
                let slack = self.symbol(Kind::Slack);
                row.insert_symbol(slack, coefficient);
                if constraint.strength < REQUIRED {
                    let error = self.symbol(Kind::Error);
                    row.insert_symbol(error, -coefficient);
                    self.objective.insert_symbol(error, constraint.strength);
                    Tag {
                        marker: slack,
                        other: error,
                    }
                } else {
                    Tag {
                        marker: slack,
                        other: INVALID,
                    }
                }
            }
            Relation::Equal => {
                if constraint.strength < REQUIRED {
                    let plus = self.symbol(Kind::Error);
                    let minus = self.symbol(Kind::Error);
                    row.insert_symbol(plus, -1.0);
                    row.insert_symbol(minus, 1.0);
                    self.objective.insert_symbol(plus, constraint.strength);
                    self.objective.insert_symbol(minus, constraint.strength);
                    Tag {
                        marker: plus,
                        other: minus,
                    }
                } else {
                    let dummy = self.symbol(Kind::Dummy);
                    row.insert_symbol(dummy, 1.0);
                    Tag {
                        marker: dummy,
                        other: INVALID,
                    }
                }
            }
        };
        if row.constant < 0.0 {
            row.reverse_sign();
        }
        (row, tag)
    }

    fn add_with_artificial(&mut self, row: &Row) -> Result<bool, SolveError> {
        let artificial = self.symbol(Kind::Slack);
        self.rows.insert(artificial, row.clone());
        self.artificial = Some(row.clone());
        self.optimize(true)?;
        let success = near_zero(
            self.artificial
                .take()
                .expect("the artificial row lives through its optimization")
                .constant,
        );
        if let Some(mut row) = self.rows.remove(&artificial) {
            if row.cells.is_empty() {
                return Ok(success);
            }
            let Some(entering) = row
                .cells
                .keys()
                .copied()
                .find(|symbol| matches!(symbol.kind, Kind::Slack | Kind::Error))
            else {
                return Ok(false);
            };
            row.solve_for_symbols(artificial, entering);
            self.substitute(entering, &row);
            self.rows.insert(entering, row);
        }
        for row in self.rows.values_mut() {
            row.cells.remove(&artificial);
        }
        self.objective.cells.remove(&artificial);
        Ok(success)
    }

    fn substitute(&mut self, symbol: Symbol, row: &Row) {
        for other in self.rows.values_mut() {
            other.substitute(symbol, row);
        }
        self.objective.substitute(symbol, row);
        if let Some(artificial) = &mut self.artificial {
            artificial.substitute(symbol, row);
        }
    }

    fn optimize(&mut self, artificial: bool) -> Result<(), SolveError> {
        loop {
            let objective = if artificial {
                self.artificial
                    .as_ref()
                    .expect("the artificial row lives through its optimization")
            } else {
                &self.objective
            };
            let Some(entering) = objective
                .cells
                .iter()
                .find(|(symbol, value)| symbol.kind != Kind::Dummy && **value < 0.0)
                .map(|(symbol, _)| *symbol)
            else {
                return Ok(());
            };
            let (leaving, mut row) = self.leaving_row(entering).ok_or(SolveError::Unbounded)?;
            row.solve_for_symbols(leaving, entering);
            self.substitute(entering, &row);
            self.rows.insert(entering, row);
        }
    }

    fn leaving_row(&mut self, entering: Symbol) -> Option<(Symbol, Row)> {
        let mut ratio = f64::INFINITY;
        let mut found = None;
        for (symbol, row) in &self.rows {
            if symbol.kind == Kind::External {
                continue;
            }
            let coefficient = row.coefficient_for(entering);
            if coefficient < 0.0 {
                let candidate = -row.constant / coefficient;
                if candidate < ratio {
                    ratio = candidate;
                    found = Some(*symbol);
                }
            }
        }
        found.map(|symbol| {
            let row = self
                .rows
                .remove(&symbol)
                .expect("the leaving row was found among the rows");
            (symbol, row)
        })
    }
}

fn choose_subject(row: &Row, tag: &Tag) -> Symbol {
    if let Some(symbol) = row
        .cells
        .keys()
        .find(|symbol| symbol.kind == Kind::External)
    {
        return *symbol;
    }
    for symbol in [tag.marker, tag.other] {
        if matches!(symbol.kind, Kind::Slack | Kind::Error) && row.coefficient_for(symbol) < 0.0 {
            return symbol;
        }
    }
    INVALID
}
