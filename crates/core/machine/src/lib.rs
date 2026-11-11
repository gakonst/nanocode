use std::{error::Error, fmt};
use tracing::warn;

#[macro_export]
macro_rules! machine {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident $(stored in $table:literal . $column:literal by $($key:literal $(as $kty:literal)?),+)? $(sql $sql:ident)? $(export sql $export:ident)? {
            $($variant:ident = $wire:literal),+ $(,)?
        }
        moves {
            $($from:ident => [$($to:ident),* $(,)?]),* $(,)?
        }
        $(sets {
            $($set:ident : $konst:ident = [$($member:ident = $mwire:literal),+ $(,)?]),* $(,)?
        })?
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
        $vis enum $name {
            $(#[serde(rename = $wire)] $variant),+
        }

        impl $crate::Machine for $name {
            const NAME: &'static str = stringify!($name);
            const ALL: &'static [Self] = &[$(Self::$variant),+];
            const MOVES: &'static [(Self, &'static [Self])] = &[$((Self::$from, &[$(Self::$to),*])),*];
            const STORED: Option<$crate::Stored> = $crate::machine!(@stored $($table, $column, [$($key [$($kty)?]),+])?);

            fn wire(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire),+
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str($crate::Machine::wire(*self))
            }
        }

        impl TryFrom<&str> for $name {
            type Error = $crate::Unknown;
            fn try_from(wire: &str) -> Result<Self, $crate::Unknown> {
                <Self as $crate::Machine>::parse(wire).ok_or_else(|| $crate::Unknown {
                    machine: stringify!($name),
                    wire: wire.to_owned(),
                })
            }
        }

        impl TryFrom<String> for $name {
            type Error = $crate::Unknown;
            fn try_from(wire: String) -> Result<Self, $crate::Unknown> {
                Self::try_from(wire.as_str())
            }
        }

        $(impl $name {
            $($vis const $konst: &'static [Self] = &[$(Self::$member),+];)*
        })?

        $crate::machine!(@sets $name [$($variant = $wire),+] [$($($($member = $mwire)+)*)?]);
        $crate::machine!(@literals [allow(unused_macros)] [$($sql)?] [$($variant = $wire),+] [$($($set = [$($member = $mwire),+])*)?]);
        $crate::machine!(@literals [macro_export] [$($export)?] [$($variant = $wire),+] [$($($set = [$($member = $mwire),+])*)?]);
    };
    (@sets $name:ident [$($state:tt)*] []) => {};
    (@sets $name:ident [$($variant:ident = $wire:literal),+] [$($member:ident = $mwire:literal)+]) => {
        const _: () = {
            const fn wire(state: $name) -> &'static str {
                match state {
                    $($name::$variant => $wire),+
                }
            }
            $(assert!($crate::same(wire($name::$member), $mwire));)+
        };
    };
    (@literals [$scope:meta] [] [$($state:tt)*] [$($set:tt)*]) => {};
    (@literals [$scope:meta] [$sql:ident] [$($variant:ident = $wire:literal),+] [$($set:ident = [$first:ident = $wired:literal $(, $rest:ident = $more:literal)*])*]) => {
        #[$scope]
        macro_rules! $sql {
            $(($variant) => { concat!("'", $wire, "'") };)+
            $(($set) => { concat!("('", $wired, "'" $(, ",'", $more, "'")*, ")") };)*
        }
    };
    (@stored $table:literal, $column:literal, [$($key:literal [$($kty:literal)?]),+]) => {
        Some($crate::Stored {
            table: $table,
            column: $column,
            key: &[$($crate::Key { name: $key, ty: $crate::machine!(@ty $($kty)?) }),+],
        })
    };
    (@stored) => {
        None
    };
    (@ty) => {
        "text"
    };
    (@ty $kty:literal) => {
        $kty
    };
}

pub trait Machine: Copy + Eq + fmt::Debug + Send + Sync + 'static {
    const NAME: &'static str;
    const ALL: &'static [Self];
    const MOVES: &'static [(Self, &'static [Self])];
    const STORED: Option<Stored>;

    fn wire(self) -> &'static str;

    fn parse(wire: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|state| state.wire() == wire)
    }

    fn may(self, to: Self) -> bool {
        Self::MOVES
            .iter()
            .any(|(from, moves)| *from == self && moves.contains(&to))
    }

    fn next(self, to: Self) -> Result<Self, Illegal<Self>> {
        if self.may(to) {
            Ok(to)
        } else {
            warn!(
                machine = Self::NAME,
                from = self.wire(),
                to = to.wire(),
                "illegal move refused"
            );
            Err(Illegal { from: self, to })
        }
    }

    fn sources(to: Self) -> Vec<&'static str> {
        Self::ALL
            .iter()
            .filter(|from| from.may(to))
            .map(|from| from.wire())
            .collect()
    }

    fn wires(states: &[Self]) -> Vec<&'static str> {
        states.iter().map(|state| state.wire()).collect()
    }

    fn terminal(self) -> bool {
        !Self::MOVES
            .iter()
            .any(|(from, moves)| *from == self && !moves.is_empty())
    }

    fn check() -> Option<String> {
        let Stored { column, .. } = Self::STORED?;
        let wires: Vec<String> = Self::ALL
            .iter()
            .map(|state| format!("'{}'", state.wire()))
            .collect();
        Some(format!("CHECK ({column} IN ({}))", wires.join(",")))
    }

    fn fence(set: &str, filter: &str) -> Option<String> {
        let stored @ Stored { table, column, .. } = Self::STORED?;
        Some(format!(
            "UPDATE {table} SET {column} = $2{set} WHERE {} AND {column}::text = ANY($3){filter} RETURNING {}",
            stored.matches("$1"),
            stored.columns()
        ))
    }
}

#[must_use]
pub const fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut at = 0;
    while at < a.len() {
        if a[at] != b[at] {
            return false;
        }
        at += 1;
    }
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stored {
    pub table: &'static str,
    pub column: &'static str,
    pub key: &'static [Key],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    pub name: &'static str,
    pub ty: &'static str,
}

const UNIT_SEPARATOR: &str = "\u{1f}";

#[must_use]
pub fn key(parts: &[&str]) -> String {
    parts.join(UNIT_SEPARATOR)
}

impl Stored {
    #[must_use]
    pub fn matches(&self, ids: &str) -> String {
        match self.key {
            [Key { name, ty: "text" }] => format!("{name} = ANY({ids})"),
            [Key { name, ty }] => format!("{name} = ANY({ids}::text[]::{ty}[])"),
            keys => {
                let parts: Vec<String> = (1..)
                    .zip(keys)
                    .map(|(at, Key { ty, .. })| format!("split_part(id, chr(31), {at})::{ty}"))
                    .collect();
                format!(
                    "({}) IN (SELECT {} FROM unnest({ids}::text[]) AS ids(id))",
                    self.columns(),
                    parts.join(", ")
                )
            }
        }
    }

    #[must_use]
    pub fn columns(&self) -> String {
        let names: Vec<&str> = self.key.iter().map(|key| key.name).collect();
        names.join(", ")
    }

    #[must_use]
    pub fn text(&self) -> String {
        match self.key {
            [Key { name, ty: "text" }] => (*name).to_owned(),
            [Key { name, .. }] => format!("{name}::text"),
            keys => {
                let names: Vec<String> = keys
                    .iter()
                    .map(|key| format!("{}::text", key.name))
                    .collect();
                format!("concat_ws(chr(31), {})", names.join(", "))
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unknown {
    pub machine: &'static str,
    pub wire: String,
}

impl fmt::Display for Unknown {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} has no state {:?}", self.machine, self.wire)
    }
}

impl Error for Unknown {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Illegal<M> {
    pub from: M,
    pub to: M,
}

impl<M: Machine> fmt::Display for Illegal<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} may not move from {} to {}",
            M::NAME,
            self.from.wire(),
            self.to.wire()
        )
    }
}

impl<M: Machine> Error for Illegal<M> {}
