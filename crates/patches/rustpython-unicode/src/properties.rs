include!(concat!(env!("OUT_DIR"), "/generated/properties.rs"));

const PROPERTY_BLOCK_MASK: u32 = (1 << PROPERTY_BLOCK_BITS) - 1;

fn record(c: char) -> u32 {
    let code = u32::from(c);
    let block = usize::from(PROPERTY_BLOCKS[(code >> PROPERTY_BLOCK_BITS) as usize]);
    let entry = (block << PROPERTY_BLOCK_BITS) | (code & PROPERTY_BLOCK_MASK) as usize;
    PROPERTY_RECORDS[usize::from(PROPERTY_RECORD_INDEXES[entry])]
}

fn field(c: char, shift: u32, mask: u32) -> usize {
    ((record(c) >> shift) & mask) as usize
}

pub(crate) fn general_category(c: char) -> GeneralCategory {
    GeneralCategory::ALL[field(c, GENERAL_CATEGORY_SHIFT, GENERAL_CATEGORY_MASK)]
}

pub(crate) fn bidi_class(c: char) -> BidiClass {
    BidiClass::ALL[field(c, BIDI_CLASS_SHIFT, BIDI_CLASS_MASK)]
}

pub(crate) fn east_asian_width(c: char) -> EastAsianWidth {
    EastAsianWidth::ALL[field(c, EAST_ASIAN_WIDTH_SHIFT, EAST_ASIAN_WIDTH_MASK)]
}

pub(crate) fn numeric_type(c: char) -> NumericType {
    NumericType::ALL[field(c, NUMERIC_TYPE_SHIFT, NUMERIC_TYPE_MASK)]
}

pub(crate) fn has(c: char, flag: u32) -> bool {
    record(c) & flag != 0
}
