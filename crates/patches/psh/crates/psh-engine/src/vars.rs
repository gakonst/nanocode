//! Shell variables: scalars, indexed and associative arrays, and their attributes
//! (D1260).
//!
//! Associative arrays iterate in bash's order, which is observable (`${!h[@]}`,
//! `declare -p`): a hash table of 1024 buckets indexed by the 32-bit FNV-1 hash of the
//! key, new keys at the head of their bucket's chain, and a table that grows four times
//! once it holds twice as many entries as buckets. The model was derived by running
//! bash (black box) on thousands of keys and matches it exactly (D1261).

use std::collections::{BTreeMap, HashMap};
use std::hash::{BuildHasherDefault, Hasher};

/// The hash of the engine's own tables (variable names, spans): FxHash, a rotate, an
/// xor and a multiply per word. The keys come from the script the shell is running,
/// which can do anything a collision could, so the keyed SipHash of `HashMap`'s
/// default bought nothing and was 7% of a loop's samples.
#[derive(Clone, Copy, Default)]
pub(crate) struct NameHasher(u64);

impl NameHasher {
    fn add(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
}

impl Hasher for NameHasher {
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.add(u64::from_le_bytes(word));
        }
    }

    fn write_u8(&mut self, n: u8) {
        self.add(u64::from(n));
    }

    fn write_u32(&mut self, n: u32) {
        self.add(u64::from(n));
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

/// A `HashMap` hashed with [`NameHasher`].
pub(crate) type NameMap<K, V> = HashMap<K, V, BuildHasherDefault<NameHasher>>;

/// A variable's value.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum Value {
    /// Declared (`export x`, `local x`, `declare -i x`) but without a value.
    #[default]
    Unset,
    Str(Vec<u8>),
    Indexed(BTreeMap<i64, Vec<u8>>),
    Assoc(Assoc),
}

/// Variables whose value the shell computes when they are read (until unset).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Dynamic {
    Random,
    Srandom,
    Seconds,
    EpochSeconds,
    EpochRealtime,
    BashPid,
    BashSubshell,
    /// The call stack (D1320).
    FuncName,
    BashSource,
    BashLineno,
    BashArgv,
    BashArgc,
    /// The host's groups (D1328).
    Groups,
    /// `$0`; assigning sets it (D1329).
    BashArgv0,
    /// The aliases as an associative array; assigning an element defines one.
    BashAliases,
    BashMonoseconds,
    Lineno,
    /// `DIRSTACK`: the current directory, then `pushd`'s stack.
    Dirstack,
}

/// `declare -l` / `-u` (and `-c` is not bash 5.3's).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Case {
    #[default]
    None,
    Lower,
    Upper,
    /// `declare -c`: the first char upper, the rest lower.
    Capitalize,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Var {
    pub value: Value,
    pub exported: bool,
    pub readonly: bool,
    /// `declare -i`: assignments are evaluated as arithmetic.
    pub integer: bool,
    pub case: Case,
    /// `declare -n`: the value names another variable.
    pub nameref: bool,
    /// `declare -t`.
    pub trace: bool,
    /// An array declared (`declare -a x`) but never assigned: `declare -p` shows no
    /// `=()`.
    pub declared_only: bool,
    /// Computed when read (`$RANDOM`, `$SECONDS`, …).
    pub dynamic: Option<Dynamic>,
}

impl Var {
    pub(crate) fn scalar(value: Vec<u8>) -> Var {
        Var {
            value: Value::Str(value),
            ..Var::default()
        }
    }

    /// The value as a scalar: the string, or element 0 / key "0" of an array.
    pub(crate) fn get(&self) -> Option<&[u8]> {
        match &self.value {
            Value::Unset => None,
            Value::Str(s) => Some(s),
            Value::Indexed(m) => m.get(&0).map(Vec::as_slice),
            Value::Assoc(a) => a.get(b"0"),
        }
    }

    pub(crate) fn is_array(&self) -> bool {
        matches!(self.value, Value::Indexed(_) | Value::Assoc(_))
    }

    pub(crate) fn is_assoc(&self) -> bool {
        matches!(self.value, Value::Assoc(_))
    }

    /// The values, in order: one for a scalar, the elements of an array.
    pub(crate) fn values(&self) -> Vec<Vec<u8>> {
        match &self.value {
            Value::Unset => Vec::new(),
            Value::Str(s) => vec![s.clone()],
            Value::Indexed(m) => m.values().cloned().collect(),
            Value::Assoc(a) => a.iter().map(|(_, v)| v.to_vec()).collect(),
        }
    }

    /// The keys, in order (`${!a[@]}`): `0` for a set scalar.
    pub(crate) fn keys(&self) -> Vec<Vec<u8>> {
        match &self.value {
            Value::Unset => Vec::new(),
            Value::Str(_) => vec![b"0".to_vec()],
            Value::Indexed(m) => m.keys().map(|k| k.to_string().into_bytes()).collect(),
            Value::Assoc(a) => a.iter().map(|(k, _)| k.to_vec()).collect(),
        }
    }

    /// Sets the scalar value: the string, or element 0 of an array.
    pub(crate) fn set(&mut self, v: Vec<u8>) {
        self.declared_only = false;
        match &mut self.value {
            Value::Indexed(m) => {
                m.insert(0, v);
            }
            Value::Assoc(a) => a.insert(b"0".to_vec(), v),
            other => *other = Value::Str(v),
        }
    }

    /// The attribute letters of `declare -p` (`-` when there are none).
    pub(crate) fn flags(&self) -> String {
        let mut f = String::new();
        match self.value {
            Value::Indexed(_) => f.push('a'),
            Value::Assoc(_) => f.push('A'),
            _ => {}
        }
        if self.case == Case::Capitalize {
            f.push('c');
        }
        if self.integer {
            f.push('i');
        }
        if self.case == Case::Lower {
            f.push('l');
        }
        if self.nameref {
            f.push('n');
        }
        if self.readonly {
            f.push('r');
        }
        if self.trace {
            f.push('t');
        }
        if self.case == Case::Upper {
            f.push('u');
        }
        if self.exported {
            f.push('x');
        }
        if f.is_empty() {
            f.push('-');
        }
        f
    }
}

/// bash's string hash: 32-bit FNV-1.
fn fnv1(key: &[u8]) -> u32 {
    let mut h: u32 = 2_166_136_261;
    for &b in key {
        h = h.wrapping_mul(16_777_619);
        h ^= u32::from(b);
    }
    h
}

const INITIAL_BUCKETS: usize = 1024;

/// An associative array in bash's iteration order (see the module docs).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Assoc {
    /// Chains, head first; empty until the first insert.
    buckets: Vec<Vec<(Vec<u8>, Vec<u8>)>>,
    /// Lookups scan one chain: at most two entries per bucket on average.
    len: usize,
}

impl Assoc {
    fn bucket(&self, key: &[u8]) -> usize {
        fnv1(key) as usize & (self.buckets.len() - 1)
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn get(&self, key: &[u8]) -> Option<&[u8]> {
        if self.buckets.is_empty() {
            return None;
        }
        self.buckets[self.bucket(key)]
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_slice())
    }

    pub(crate) fn insert(&mut self, key: Vec<u8>, value: Vec<u8>) {
        if self.buckets.is_empty() {
            self.buckets = vec![Vec::new(); INITIAL_BUCKETS];
        }
        let b = self.bucket(&key);
        if let Some(e) = self.buckets[b].iter_mut().find(|(k, _)| *k == key) {
            e.1 = value;
            return;
        }
        if self.len >= self.buckets.len() * 2 {
            self.grow();
        }
        let b = self.bucket(&key);
        self.buckets[b].insert(0, (key, value));
        self.len += 1;
    }

    /// Four times the buckets; entries move in old bucket order, each to the head of
    /// its new chain.
    fn grow(&mut self) {
        let old = std::mem::take(&mut self.buckets);
        self.buckets = vec![Vec::new(); old.len() * 4];
        for chain in old {
            for (k, v) in chain {
                let b = self.bucket(&k);
                self.buckets[b].insert(0, (k, v));
            }
        }
    }

    pub(crate) fn remove(&mut self, key: &[u8]) -> bool {
        if self.buckets.is_empty() {
            return false;
        }
        let b = self.bucket(key);
        let chain = &mut self.buckets[b];
        match chain.iter().position(|(k, _)| k == key) {
            Some(i) => {
                chain.remove(i);
                self.len -= 1;
                true
            }
            None => false,
        }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&[u8], &[u8])> {
        self.buckets
            .iter()
            .flat_map(|c| c.iter().map(|(k, v)| (k.as_slice(), v.as_slice())))
    }
}

/// The variable table of a vproc.
#[derive(Clone, Default)]
pub(crate) struct Vars {
    map: NameMap<String, Var>,
}

impl Vars {
    pub(crate) fn get(&self, name: &str) -> Option<&Var> {
        self.map.get(name)
    }

    pub(crate) fn get_mut(&mut self, name: &str) -> Option<&mut Var> {
        self.map.get_mut(name)
    }

    /// The scalar value of `name` (no nameref resolution; see `Vp::var`).
    pub(crate) fn value(&self, name: &str) -> Option<&[u8]> {
        self.map.get(name).and_then(Var::get)
    }

    pub(crate) fn entry(&mut self, name: &str) -> &mut Var {
        // The name is copied only for a variable that is new.
        if !self.map.contains_key(name) {
            self.map.insert(name.to_string(), Var::default());
        }
        self.map
            .get_mut(name)
            .expect("the variable is in the table")
    }

    pub(crate) fn remove(&mut self, name: &str) -> Option<Var> {
        self.map.remove(name)
    }

    pub(crate) fn insert(&mut self, name: &str, var: Var) {
        self.map.insert(name.to_string(), var);
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&String, &Var)> {
        self.map.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assoc_order_is_bashs() {
        // Orders recorded from bash 5.3.
        let mut a = Assoc::default();
        for k in ["a", "b", "c", "d", "zz", "foo", "10", "2"] {
            a.insert(k.into(), b"1".to_vec());
        }
        let keys: Vec<&[u8]> = a.iter().map(|(k, _)| k).collect();
        assert_eq!(
            keys,
            [&b"2"[..], b"d", b"c", b"b", b"a", b"zz", b"foo", b"10"]
        );
        // Colliding keys: the newer first.
        let mut a = Assoc::default();
        for k in ["k8", "k116", "k9", "k117", "k6", "k118"] {
            a.insert(k.into(), b"1".to_vec());
        }
        let keys: Vec<&[u8]> = a.iter().map(|(k, _)| k).collect();
        assert_eq!(keys, [&b"k116"[..], b"k8", b"k117", b"k9", b"k118", b"k6"]);
        // Past 2048 entries the table grows.
        let mut a = Assoc::default();
        for i in 1..=3000 {
            a.insert(format!("k{i}").into_bytes(), b"1".to_vec());
        }
        let first: Vec<&[u8]> = a.iter().map(|(k, _)| k).take(4).collect();
        assert_eq!(first, [&b"k1698"[..], b"k1699", b"k1696", b"k1697"]);
        assert!(a.remove(b"k5"));
        assert_eq!(a.len(), 2999);
    }
}
