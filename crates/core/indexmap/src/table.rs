pub(crate) const LINEAR_ENTRIES_MAX: usize = 8;
const LOAD_NUMERATOR: usize = 3;
const LOAD_DENOMINATOR: usize = 4;
const SLOT_EMPTY: u64 = 0;
const SHIFT_PROBE_SLOTS_PER_ENTRY: usize = 2;

#[derive(Clone)]
pub(crate) struct Table {
    slots: Vec<u64>,
    hashes: Vec<u64>,
}

pub(crate) enum Vacancy {
    Linear,
    Hashed { hash: u64, position: usize },
}

const fn slots_for(entries: usize) -> usize {
    let Some(scaled) = entries.checked_mul(LOAD_DENOMINATOR) else {
        panic!("IndexMap: capacity overflow");
    };
    let Some(slots) = (scaled / LOAD_NUMERATOR + 1).checked_next_power_of_two() else {
        panic!("IndexMap: capacity overflow");
    };
    slots
}

const fn home(hash: u64, mask: usize) -> usize {
    hash as usize & mask
}

const fn tag(hash: u64, mask: usize) -> u64 {
    hash & !(mask as u64)
}

fn slot_for(hash: u64, index: usize, mask: usize) -> u64 {
    assert!(index < mask);
    tag(hash, mask) | (index as u64 + 1)
}

const fn slot_index(slot: u64, mask: usize) -> usize {
    (slot & mask as u64) as usize - 1
}

fn empty_position(slots: &[u64], hash: u64) -> usize {
    let mask = slots.len() - 1;
    let mut position = home(hash, mask);
    for _ in 0..slots.len() {
        if slots[position] == SLOT_EMPTY {
            return position;
        }
        position = (position + 1) & mask;
    }
    panic!("IndexMap: index table has no empty slot");
}

impl Table {
    pub(crate) fn build(hashes: Vec<u64>, entries_planned: usize) -> Self {
        assert!(entries_planned >= hashes.len());
        let mut table = Self {
            slots: vec![SLOT_EMPTY; slots_for(entries_planned)],
            hashes,
        };
        table.place_all();
        table
    }

    fn place_all(&mut self) {
        let mask = self.slots.len() - 1;
        for (index, &hash) in self.hashes.iter().enumerate() {
            let position = empty_position(&self.slots, hash);
            self.slots[position] = slot_for(hash, index, mask);
        }
    }

    fn resize(&mut self, slots_len: usize) {
        assert!(slots_len.is_power_of_two());
        assert!(slots_len >= slots_for(self.hashes.len()));
        self.slots.clear();
        self.slots.resize(slots_len, SLOT_EMPTY);
        self.place_all();
    }

    pub(crate) fn reserve(&mut self, entries_total: usize) {
        let slots_len = slots_for(entries_total);
        if slots_len > self.slots.len() {
            self.resize(slots_len);
        }
        assert!(entries_total >= self.hashes.len());
        self.hashes.reserve(entries_total - self.hashes.len());
    }

    pub(crate) const fn len(&self) -> usize {
        self.hashes.len()
    }

    pub(crate) const fn hashes_mut(&mut self) -> &mut Vec<u64> {
        &mut self.hashes
    }

    pub(crate) fn into_hashes(self) -> Vec<u64> {
        self.hashes
    }

    #[inline]
    pub(crate) fn find(
        &self,
        hash: u64,
        mut matches: impl FnMut(usize) -> bool,
    ) -> Result<usize, usize> {
        let mask = self.slots.len() - 1;
        let tag = tag(hash, mask);
        let mut position = home(hash, mask);
        for _ in 0..self.slots.len() {
            let slot = self.slots[position];
            if slot == SLOT_EMPTY {
                return Err(position);
            }
            if slot & !(mask as u64) == tag {
                let index = slot_index(slot, mask);
                if matches(index) {
                    return Ok(index);
                }
            }
            position = (position + 1) & mask;
        }
        panic!("IndexMap: index table has no empty slot");
    }

    fn position_of(&self, index: usize) -> usize {
        let mask = self.slots.len() - 1;
        let hash = self.hashes[index];
        let wanted = slot_for(hash, index, mask);
        let mut position = home(hash, mask);
        for _ in 0..self.slots.len() {
            let slot = self.slots[position];
            if slot == wanted {
                return position;
            }
            assert!(
                slot != SLOT_EMPTY,
                "IndexMap: entry missing from its index table"
            );
            position = (position + 1) & mask;
        }
        panic!("IndexMap: entry missing from its index table");
    }

    pub(crate) fn push(&mut self, hash: u64, position: usize) {
        let index = self.hashes.len();
        let slots_len = slots_for(index + 1);
        self.hashes.push(hash);
        if slots_len > self.slots.len() {
            self.resize(slots_len);
        } else {
            assert!(self.slots[position] == SLOT_EMPTY);
            self.slots[position] = slot_for(hash, index, self.slots.len() - 1);
        }
    }

    fn erase(&mut self, position: usize) {
        let mask = self.slots.len() - 1;
        let mut hole = position;
        let mut next = (position + 1) & mask;
        for _ in 0..self.slots.len() {
            let slot = self.slots[next];
            if slot == SLOT_EMPTY {
                self.slots[hole] = SLOT_EMPTY;
                return;
            }
            let ideal = home(self.hashes[slot_index(slot, mask)], mask);
            if (next.wrapping_sub(ideal) & mask) >= (next.wrapping_sub(hole) & mask) {
                self.slots[hole] = slot;
                hole = next;
            }
            next = (next + 1) & mask;
        }
        panic!("IndexMap: index table has no empty slot");
    }

    pub(crate) fn swap_remove(&mut self, index: usize) {
        let last = self.hashes.len() - 1;
        let position = self.position_of(index);
        self.erase(position);
        if index != last {
            let position_last = self.position_of(last);
            self.slots[position_last] = slot_for(self.hashes[last], index, self.slots.len() - 1);
        }
        self.hashes.swap_remove(index);
    }

    pub(crate) fn shift_remove(&mut self, index: usize) {
        let position = self.position_of(index);
        self.erase(position);
        self.shift_down(index + 1, self.hashes.len());
        self.hashes.remove(index);
    }

    pub(crate) fn move_index(&mut self, from: usize, to: usize) {
        assert!(from != to);
        let hash = self.hashes[from];
        let position = self.position_of(from);
        if from < to {
            self.shift_down(from + 1, to + 1);
            self.hashes[from..=to].rotate_left(1);
        } else {
            self.shift_up(to, from);
            self.hashes[to..=from].rotate_right(1);
        }
        self.slots[position] = slot_for(hash, to, self.slots.len() - 1);
    }

    const fn probes_each(&self, start: usize, end: usize) -> bool {
        (end - start) * SHIFT_PROBE_SLOTS_PER_ENTRY <= self.slots.len()
    }

    fn shift_down(&mut self, start: usize, end: usize) {
        assert!(start >= 1);
        if self.probes_each(start, end) {
            for index in start..end {
                let position = self.position_of(index);
                self.slots[position] -= 1;
            }
        } else {
            let mask = (self.slots.len() - 1) as u64;
            for slot in &mut self.slots {
                let index_next = *slot & mask;
                if *slot != SLOT_EMPTY && index_next > start as u64 && index_next <= end as u64 {
                    *slot -= 1;
                }
            }
        }
    }

    fn shift_up(&mut self, start: usize, end: usize) {
        if self.probes_each(start, end) {
            for index in (start..end).rev() {
                let position = self.position_of(index);
                self.slots[position] += 1;
            }
        } else {
            let mask = (self.slots.len() - 1) as u64;
            for slot in &mut self.slots {
                let index_next = *slot & mask;
                if *slot != SLOT_EMPTY && index_next > start as u64 && index_next <= end as u64 {
                    *slot += 1;
                }
            }
        }
    }
}
