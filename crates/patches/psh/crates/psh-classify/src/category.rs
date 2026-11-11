//! Categories of commands, for UI summaries and permission checks.

/// What a command does, coarsely. The order is the priority used by
/// [`Categories::primary`]: a command that is both a build and a mutation is shown as a
/// mutation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    /// Reads files or state and changes nothing outside the shell (`ls`, `grep`, `cd`,
    /// `git status`, `sed -n`).
    Read,
    /// Talks to the network (`curl`, `gh`, `git fetch`).
    Network,
    /// Builds, tests or lints a project, running its code (`cargo test`, `npm run x`).
    Build,
    /// Runs until stopped or for a long time: servers, watchers, `tail -f`, background
    /// jobs.
    LongRunning,
    /// Changes files or processes (`rm`, `sed -i`, `> file`, `kill`).
    Mutate,
    /// Changes a repository (`git commit`, `git push`, `gh pr create`).
    Vcs,
    /// Installs or removes packages (`npm install`, `cargo add`, `brew install`).
    Install,
    /// Cannot be known without running it: a dynamic command name, an inline
    /// interpreter, a local script, `eval` of a non-literal, an unknown program.
    Unknown,
}

impl Category {
    pub const ALL: [Category; 8] = [
        Category::Read,
        Category::Network,
        Category::Build,
        Category::LongRunning,
        Category::Mutate,
        Category::Vcs,
        Category::Install,
        Category::Unknown,
    ];

    /// The name used in JSON and in the test cases.
    pub fn name(self) -> &'static str {
        match self {
            Category::Read => "read",
            Category::Network => "network",
            Category::LongRunning => "long-running",
            Category::Build => "build",
            Category::Mutate => "mutate",
            Category::Vcs => "vcs",
            Category::Install => "install",
            Category::Unknown => "unknown",
        }
    }

    pub fn from_name(name: &str) -> Option<Category> {
        Category::ALL.into_iter().find(|c| c.name() == name)
    }

    fn bit(self) -> u8 {
        1 << (self as u8)
    }
}

/// A set of categories.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Categories(u8);

impl Categories {
    pub const fn empty() -> Categories {
        Categories(0)
    }

    pub fn of(c: Category) -> Categories {
        Categories(c.bit())
    }

    pub fn contains(self, c: Category) -> bool {
        self.0 & c.bit() != 0
    }

    pub fn insert(&mut self, c: Category) {
        self.0 |= c.bit();
    }

    pub fn extend(&mut self, other: Categories) {
        self.0 |= other.0;
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn iter(self) -> impl Iterator<Item = Category> {
        Category::ALL.into_iter().filter(move |c| self.contains(*c))
    }

    /// The category with the highest priority; `Read` for the empty set.
    pub fn primary(self) -> Category {
        self.iter().last().unwrap_or(Category::Read)
    }

    /// Nothing but [`Category::Read`] (or nothing at all).
    pub fn read_only(self) -> bool {
        self.0 & !Category::Read.bit() == 0
    }
}

impl From<Category> for Categories {
    fn from(c: Category) -> Categories {
        Categories::of(c)
    }
}
