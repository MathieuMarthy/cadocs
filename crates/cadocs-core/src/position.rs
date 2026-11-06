//! Fractional indexes: strings that sort between their neighbours, so reordering a page never
//! rewrites its siblings.

const DIGITS: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const MIDDLE: usize = DIGITS.len() / 2;

fn digit(byte: &u8) -> usize {
    DIGITS.binary_search(byte).unwrap_or(0)
}

/// A key strictly between `before` and `after`, `None` being the end of the list. Both must come
/// from this function, with `before` below `after`. A key never ends in `0`, which leaves room
/// below it, and adding at either end only grows it by a digit every 30 keys or so.
pub fn between(before: Option<&str>, after: Option<&str>) -> String {
    let key = midpoint(before.unwrap_or("").as_bytes(), after.map(str::as_bytes));
    key.into_iter().map(char::from).collect()
}

/// `low` reads as a fraction, so it goes on with zeros past its end.
fn midpoint(low: &[u8], high: Option<&[u8]>) -> Vec<u8> {
    if let Some(high) = high {
        let digit_of_low = |index| low.get(index).copied().unwrap_or(DIGITS[0]);
        let same = |(index, digit): &(usize, &u8)| digit_of_low(*index) == **digit;
        let shared = high.iter().enumerate().take_while(same).count();
        if shared > 0 {
            let mut key = high[..shared].to_vec();
            let rest = low.get(shared..).unwrap_or_default();
            key.extend(midpoint(rest, Some(&high[shared..])));
            return key;
        }
    }

    let first_low = low.first().map_or(0, digit);
    let first_high = high.and_then(<[u8]>::first).map_or(DIGITS.len(), digit);
    if first_high.saturating_sub(first_low) > 1 {
        let chosen = match (low.is_empty(), high.is_some()) {
            (true, false) => MIDDLE,
            (false, false) => first_low + 1,
            (true, true) => first_high - 1,
            (false, true) => (first_low + first_high) / 2,
        };
        return vec![DIGITS[chosen]];
    }
    match high {
        Some(high) if high.len() > 1 => vec![high[0]],
        _ => {
            let mut key = vec![DIGITS[first_low]];
            key.extend(midpoint(low.get(1..).unwrap_or_default(), None));
            key
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_valid(key: &str) -> bool {
        !key.is_empty() && !key.ends_with('0') && key.bytes().all(|b| b.is_ascii_alphanumeric())
    }

    #[test]
    fn a_key_sorts_between_its_neighbours() {
        let first = between(None, None);
        let last = between(Some(&first), None);
        let middle = between(Some(&first), Some(&last));
        let start = between(None, Some(&first));
        assert!(start < first && first < middle && middle < last);
    }

    #[test]
    fn appending_stays_short() {
        let mut key = between(None, None);
        for _ in 0..1000 {
            let next = between(Some(&key), None);
            assert!(key < next && is_valid(&next), "{key} then {next}");
            key = next;
        }
        assert!(key.len() <= 40, "{key}");
    }

    #[test]
    fn prepending_stays_short() {
        let mut key = between(None, None);
        for _ in 0..1000 {
            let next = between(None, Some(&key));
            assert!(next < key && is_valid(&next), "{next} then {key}");
            key = next;
        }
        assert!(key.len() <= 40, "{key}");
    }

    #[test]
    fn the_same_gap_can_be_split_again_and_again() {
        let low = between(None, None);
        let mut high = between(Some(&low), None);
        for _ in 0..1000 {
            let next = between(Some(&low), Some(&high));
            assert!(low < next && next < high && is_valid(&next), "{next}");
            high = next;
        }
        let mut low = low;
        for _ in 0..1000 {
            let next = between(Some(&low), Some(&high));
            assert!(low < next && next < high && is_valid(&next), "{next}");
            low = next;
        }
    }

    #[test]
    fn mixed_inserts_keep_every_key_distinct_and_sorted() {
        let mut keys = vec![between(None, None)];
        let mut seed = 12345_usize;
        for _ in 0..3000 {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let slot = (seed >> 33) % (keys.len() + 1);
            let before = slot.checked_sub(1).map(|index| keys[index].as_str());
            let key = between(before, keys.get(slot).map(String::as_str));
            assert!(is_valid(&key));
            keys.insert(slot, key);
        }
        assert!(keys.windows(2).all(|pair| pair[0] < pair[1]));
    }
}
