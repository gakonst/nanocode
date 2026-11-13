const BASE: u32 = 36;
const T_MIN: u32 = 1;
const T_MAX: u32 = 26;
const SKEW: u32 = 38;
const DAMP: u32 = 700;
const INITIAL_BIAS: u32 = 72;
const INITIAL_N: u32 = 0x80;
const DELIMITER: char = '-';
const LETTER_DIGITS: u32 = 26;

const fn adapt(delta: u32, points: u32, first: bool) -> u32 {
    let mut delta = if first { delta / DAMP } else { delta / 2 };
    delta += delta / points;
    let mut k = 0;
    while delta > ((BASE - T_MIN) * T_MAX) / 2 {
        delta /= BASE - T_MIN;
        k += BASE;
    }
    k + (((BASE - T_MIN + 1) * delta) / (delta + SKEW))
}

const fn threshold(k: u32, bias: u32) -> u32 {
    if k <= bias {
        T_MIN
    } else if k >= bias + T_MAX {
        T_MAX
    } else {
        k - bias
    }
}

const fn digit_value(byte: u8) -> Option<u32> {
    match byte {
        b'a'..=b'z' => Some((byte - b'a') as u32),
        b'A'..=b'Z' => Some((byte - b'A') as u32),
        b'0'..=b'9' => Some((byte - b'0') as u32 + LETTER_DIGITS),
        _ => None,
    }
}

fn digit_char(digit: u32) -> char {
    assert!(digit < BASE, "a punycode digit is below the base");
    let byte = u8::try_from(digit).expect("a punycode digit fits in a byte");
    let letters = u8::try_from(LETTER_DIGITS).expect("the letter digits fit in a byte");
    if byte < letters {
        char::from(b'a' + byte)
    } else {
        char::from(b'0' + byte - letters)
    }
}

#[must_use]
pub fn decode(input: &str) -> Option<String> {
    if !input.is_ascii() {
        return None;
    }
    let (basic, extended) = match input.rfind(DELIMITER) {
        Some(position) if position > 0 => (&input[..position], &input[position + 1..]),
        Some(_) | None => ("", input),
    };
    let mut output: Vec<char> = basic.chars().collect();
    let mut code = INITIAL_N;
    let mut index: u32 = 0;
    let mut bias = INITIAL_BIAS;
    let mut bytes = extended.bytes().peekable();
    while bytes.peek().is_some() {
        let previous = index;
        let mut weight: u32 = 1;
        let mut k = BASE;
        loop {
            let digit = digit_value(bytes.next()?)?;
            index = index.checked_add(digit.checked_mul(weight)?)?;
            let t = threshold(k, bias);
            if digit < t {
                break;
            }
            weight = weight.checked_mul(BASE - t)?;
            k += BASE;
        }
        let length = u32::try_from(output.len()).ok()? + 1;
        bias = adapt(index - previous, length, previous == 0);
        code = code.checked_add(index / length)?;
        index %= length;
        output.insert(usize::try_from(index).ok()?, char::from_u32(code)?);
        index += 1;
    }
    Some(output.into_iter().collect())
}

#[must_use]
pub fn encode(input: &str) -> Option<String> {
    let points: Vec<u32> = input.chars().map(u32::from).collect();
    let mut output: String = input.chars().filter(char::is_ascii).collect();
    let basic = u32::try_from(output.len()).ok()?;
    let total = u32::try_from(points.len()).ok()?;
    if basic > 0 {
        output.push(DELIMITER);
    }
    let mut handled = basic;
    let mut code = INITIAL_N;
    let mut delta: u32 = 0;
    let mut bias = INITIAL_BIAS;
    while handled < total {
        let next = points
            .iter()
            .copied()
            .filter(|&point| point >= code)
            .min()?;
        delta = delta.checked_add((next - code).checked_mul(handled + 1)?)?;
        code = next;
        for &point in &points {
            if point < code {
                delta = delta.checked_add(1)?;
            }
            if point == code {
                let mut q = delta;
                let mut k = BASE;
                loop {
                    let t = threshold(k, bias);
                    if q < t {
                        break;
                    }
                    output.push(digit_char(t + (q - t) % (BASE - t)));
                    q = (q - t) / (BASE - t);
                    k += BASE;
                }
                output.push(digit_char(q));
                bias = adapt(delta, handled + 1, handled == basic);
                delta = 0;
                handled += 1;
            }
        }
        delta = delta.checked_add(1)?;
        code = code.checked_add(1)?;
    }
    Some(output)
}
