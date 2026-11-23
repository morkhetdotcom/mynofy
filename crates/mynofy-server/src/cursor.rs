//! Stateless topic position: eight ASCII digits, with modular wrap resolution.
use crate::ids::{Id, SPACE};

pub(crate) fn encode(position: u64) -> String {
    // Id only contains ASCII, so this conversion cannot fail.
    Id::from_index(position % SPACE)
        .0
        .iter()
        .map(|&c| char::from(c))
        .collect()
}
pub(crate) fn decode(text: &str, head: u64) -> Option<u64> {
    if text.len() != 8 {
        return None;
    }
    let mut value = 0u64;
    for c in text.bytes() {
        let digit = match c {
            b'0'..=b'9' => c - b'0',
            b'A'..=b'Z' => c - b'A' + 10,
            b'a'..=b'z' => c - b'a' + 36,
            _ => return None,
        };
        value = value * 62 + u64::from(digit);
    }
    // Select the latest matching position at/before head. No active anchor needed.
    let distance = (head % SPACE + SPACE - value) % SPACE;
    head.checked_sub(distance)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn positions_wrap_and_resolve_without_anchor() {
        for p in [0, 1, 61, 62, SPACE - 1, SPACE, SPACE + 1, u64::MAX] {
            let encoded = encode(p);
            assert_eq!(encoded.len(), 8);
            assert_eq!(decode(&encoded, p), Some(p));
        }
        assert_eq!(decode("zzzzzzzz", SPACE + 2), Some(SPACE - 1));
        assert_eq!(decode("00000001", 0), None);
        for invalid in ["", "0000000", "000000000", "0000000!", "000000é"] {
            assert_eq!(decode(invalid, 100), None);
        }
    }
}
