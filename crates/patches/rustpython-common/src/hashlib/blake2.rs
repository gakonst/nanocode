pub(super) const SIGMA: [[usize; 16]; 10] = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
    [11, 8, 12, 0, 5, 2, 15, 13, 10, 14, 3, 6, 7, 1, 9, 4],
    [7, 9, 3, 1, 13, 12, 11, 14, 2, 6, 5, 10, 4, 0, 15, 8],
    [9, 0, 5, 7, 2, 4, 10, 15, 14, 1, 11, 12, 6, 8, 3, 13],
    [2, 12, 6, 10, 0, 11, 8, 3, 4, 13, 7, 5, 15, 14, 1, 9],
    [12, 5, 1, 15, 14, 13, 4, 10, 0, 7, 6, 3, 9, 2, 8, 11],
    [13, 11, 7, 14, 12, 1, 3, 9, 5, 0, 15, 4, 8, 6, 2, 10],
    [6, 15, 14, 9, 11, 3, 0, 8, 12, 2, 13, 7, 1, 4, 10, 5],
    [10, 2, 8, 4, 7, 6, 1, 5, 15, 11, 9, 14, 3, 12, 13, 0],
];

pub(super) const COLUMNS_THEN_DIAGONALS: [[usize; 4]; 8] = [
    [0, 4, 8, 12],
    [1, 5, 9, 13],
    [2, 6, 10, 14],
    [3, 7, 11, 15],
    [0, 5, 10, 15],
    [1, 6, 11, 12],
    [2, 7, 8, 13],
    [3, 4, 9, 14],
];

pub(super) struct Parameters<'a> {
    pub(super) digest_size: usize,
    pub(super) key: &'a [u8],
    pub(super) salt: &'a [u8],
    pub(super) person: &'a [u8],
    pub(super) fanout: u8,
    pub(super) depth: u8,
    pub(super) leaf_size: u32,
    pub(super) node_offset: u64,
    pub(super) node_depth: u8,
    pub(super) inner_size: usize,
    pub(super) last_node: bool,
}

const SEQUENTIAL_FANOUT: u8 = 1;
const SEQUENTIAL_DEPTH: u8 = 1;

impl Parameters<'static> {
    pub(super) const fn default_for(digest_size: usize) -> Self {
        Self {
            digest_size,
            key: &[],
            salt: &[],
            person: &[],
            fanout: SEQUENTIAL_FANOUT,
            depth: SEQUENTIAL_DEPTH,
            leaf_size: 0,
            node_offset: 0,
            node_depth: 0,
            inner_size: 0,
            last_node: false,
        }
    }
}

pub(super) struct Layout {
    pub(super) output_bytes: usize,
    pub(super) key_bytes: usize,
    pub(super) node_offset_bytes: usize,
    pub(super) reserved_bytes: usize,
    pub(super) salt_bytes: usize,
}

impl Parameters<'_> {
    pub(super) fn block<const BYTES: usize>(&self, layout: &Layout) -> [u8; BYTES] {
        assert!((1..=layout.output_bytes).contains(&self.digest_size));
        assert!(self.key.len() <= layout.key_bytes);
        assert!(self.salt.len() <= layout.salt_bytes);
        assert!(self.person.len() <= layout.salt_bytes);
        assert!(self.inner_size <= layout.output_bytes);
        let node_offset = self.node_offset.to_le_bytes();
        assert!(
            node_offset[layout.node_offset_bytes..]
                .iter()
                .all(|byte| *byte == 0)
        );
        let digest_size = u8::try_from(self.digest_size).expect("digest size fits a byte");
        let key_size = u8::try_from(self.key.len()).expect("key size fits a byte");
        let inner_size = u8::try_from(self.inner_size).expect("inner size fits a byte");
        let mut block = [0; BYTES];
        let mut cursor = 0;
        let mut put = |bytes: &[u8], width: usize| {
            assert!(bytes.len() <= width);
            block[cursor..cursor + bytes.len()].copy_from_slice(bytes);
            cursor += width;
        };
        let header = [digest_size, key_size, self.fanout, self.depth];
        put(&header, header.len());
        let leaf_size = self.leaf_size.to_le_bytes();
        put(&leaf_size, leaf_size.len());
        put(
            &node_offset[..layout.node_offset_bytes],
            layout.node_offset_bytes,
        );
        let node = [self.node_depth, inner_size];
        put(&node, node.len());
        put(&[], layout.reserved_bytes);
        put(self.salt, layout.salt_bytes);
        put(self.person, layout.salt_bytes);
        assert_eq!(cursor, BYTES);
        block
    }
}
