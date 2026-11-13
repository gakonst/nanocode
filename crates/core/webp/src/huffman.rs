use alloc::vec;
use alloc::vec::Vec;

use crate::error::WebpError;
use crate::lossless::BitReader;

const CODE_LENGTH_MAX: usize = 15;
const TABLE_BITS_MAX: u8 = 10;
const ENTRY_LENGTH_SHIFT: u32 = 16;
const ENTRY_SYMBOL_MASK: u32 = 0xffff;

#[derive(Clone, Copy)]
pub(crate) enum Node {
    Branch(usize),
    Leaf(u16),
    Empty,
}

pub(crate) enum HuffmanTree {
    Single(u16),
    Table {
        nodes: Vec<Node>,
        table: Vec<u32>,
        mask: u16,
    },
}

impl Default for HuffmanTree {
    fn default() -> Self {
        Self::Single(0)
    }
}

impl HuffmanTree {
    pub(crate) fn from_lengths(lengths: &[u16]) -> Result<Self, WebpError> {
        let mut histogram = [0u16; CODE_LENGTH_MAX + 1];
        let mut symbol_count = 0usize;
        for &length in lengths.iter().filter(|&&length| length != 0) {
            histogram[usize::from(length)] += 1;
            symbol_count += 1;
        }
        if symbol_count == 0 {
            return Err(WebpError::HuffmanCode);
        }
        if symbol_count == 1 {
            let Some(symbol) = lengths.iter().position(|&length| length != 0) else {
                unreachable!("one symbol has a length");
            };
            return Ok(Self::Single(symbol as u16));
        }
        let Some(length_max) = histogram.iter().rposition(|&count| count != 0) else {
            unreachable!("symbols have lengths");
        };
        let mut next_codes = [0u16; CODE_LENGTH_MAX + 1];
        let mut code = 0u16;
        for length in 1..=length_max {
            next_codes[length] = code;
            code = code.wrapping_add(histogram[length]) << 1;
        }
        if code != 2u16 << length_max {
            return Err(WebpError::HuffmanCode);
        }
        let table_bits = (length_max as u16).min(u16::from(TABLE_BITS_MAX));
        let table_size = 1usize << table_bits;
        let mask = table_size as u16 - 1;
        let long_codes: usize = histogram[usize::from(table_bits) + 1..=length_max]
            .iter()
            .map(|&count| usize::from(count))
            .sum();
        let mut nodes = Vec::with_capacity(2 * long_codes);
        let mut table = vec![0u32; table_size];
        for (symbol, &length) in lengths.iter().enumerate() {
            if length == 0 {
                continue;
            }
            let code = next_codes[usize::from(length)];
            next_codes[usize::from(length)] = code.wrapping_add(1);
            let reversed = usize::from(code.reverse_bits() >> (16 - length));
            if length <= table_bits {
                let entry = (u32::from(length) << ENTRY_LENGTH_SHIFT) | symbol as u32;
                let mut index = reversed;
                while index < table_size {
                    table[index] = entry;
                    index += 1 << length;
                }
                continue;
            }
            let table_index = reversed & usize::from(mask);
            let mut node = match table[table_index] {
                0 => {
                    let node = nodes.len();
                    table[table_index] = (node + 1) as u32;
                    nodes.push(Node::Empty);
                    node
                }
                value if value >> ENTRY_LENGTH_SHIFT != 0 => return Err(WebpError::HuffmanCode),
                value => (value - 1) as usize,
            };
            let code = usize::from(code);
            for depth in (0..length - table_bits).rev() {
                let offset = match nodes[node] {
                    Node::Empty => {
                        let offset = nodes.len() - node;
                        nodes[node] = Node::Branch(offset);
                        nodes.push(Node::Empty);
                        nodes.push(Node::Empty);
                        offset
                    }
                    Node::Leaf(_) => return Err(WebpError::HuffmanCode),
                    Node::Branch(offset) => offset,
                };
                node += offset + ((code >> depth) & 1);
            }
            match nodes[node] {
                Node::Empty => nodes[node] = Node::Leaf(symbol as u16),
                Node::Leaf(_) | Node::Branch(_) => return Err(WebpError::HuffmanCode),
            }
        }
        Ok(Self::Table { nodes, table, mask })
    }

    pub(crate) fn two_symbols(zero: u16, one: u16) -> Self {
        Self::Table {
            nodes: vec![Node::Leaf(zero), Node::Leaf(one), Node::Empty],
            table: vec![
                (1 << ENTRY_LENGTH_SHIFT) | u32::from(zero),
                (1 << ENTRY_LENGTH_SHIFT) | u32::from(one),
            ],
            mask: 1,
        }
    }

    pub(crate) const fn is_single(&self) -> bool {
        matches!(self, Self::Single(_))
    }

    #[inline(always)]
    pub(crate) fn read_symbol(&self, reader: &mut BitReader<'_>) -> Result<u16, WebpError> {
        match self {
            Self::Table { nodes, table, mask } => {
                let bits = reader.peek_full() as u16;
                let entry = table[usize::from(bits & mask)];
                if entry >> ENTRY_LENGTH_SHIFT != 0 {
                    reader.consume((entry >> ENTRY_LENGTH_SHIFT) as u8)?;
                    return Ok(entry as u16);
                }
                if entry == 0 {
                    return Err(WebpError::HuffmanCode);
                }
                read_long_symbol(
                    nodes,
                    usize::from(bits >> TABLE_BITS_MAX),
                    ((entry & ENTRY_SYMBOL_MASK) - 1) as usize,
                    reader,
                )
            }
            Self::Single(symbol) => Ok(*symbol),
        }
    }

    pub(crate) fn peek_symbol(&self, reader: &BitReader<'_>) -> Option<(u8, u16)> {
        match self {
            Self::Table { table, mask, .. } => {
                let bits = reader.peek_full() as u16;
                let entry = table[usize::from(bits & mask)];
                (entry >> ENTRY_LENGTH_SHIFT != 0)
                    .then_some(((entry >> ENTRY_LENGTH_SHIFT) as u8, entry as u16))
            }
            Self::Single(symbol) => Some((0, *symbol)),
        }
    }
}

#[inline(never)]
fn read_long_symbol(
    nodes: &[Node],
    mut bits: usize,
    start: usize,
    reader: &mut BitReader<'_>,
) -> Result<u16, WebpError> {
    let mut depth = TABLE_BITS_MAX;
    let mut index = start;
    for _ in 0..nodes.len() {
        match nodes[index] {
            Node::Branch(offset) => {
                index += offset + (bits & 1);
                depth += 1;
                bits >>= 1;
            }
            Node::Leaf(symbol) => {
                reader.consume(depth)?;
                return Ok(symbol);
            }
            Node::Empty => return Err(WebpError::HuffmanCode),
        }
    }
    unreachable!("Huffman tree branches point forward")
}
