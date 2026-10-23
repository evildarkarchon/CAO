//! A size in bytes as people read it, for the Application Log and the run
//! view.

use std::fmt;

/// A byte count displayed in binary units: whole bytes below 1 KiB, such as
/// `12 B`, and one decimal place above, such as `12.3 MiB`.
///
/// It is for people only: Run Evidence keeps exact byte counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ByteSize(pub u64);

/// The binary units above bytes, each 1024 times the one before. Exbibytes
/// are the last needed: `u64::MAX` is just under 16 EiB.
const UNITS: [&str; 6] = ["KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];

impl fmt::Display for ByteSize {
    /// Writes the size in the largest unit that keeps the value under 1024
    /// once rounded to one decimal place, so 1023.96 KiB reads `1.0 MiB`
    /// rather than `1024.0 KiB`.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 < 1024 {
            return write!(formatter, "{} B", self.0);
        }
        // An `f64` loses low bits only above 2^53 bytes, far below one
        // decimal place of the unit such a size is shown in.
        let mut value = self.0 as f64 / 1024.0;
        let mut unit = 0;
        while value >= 1023.95 && unit + 1 < UNITS.len() {
            value /= 1024.0;
            unit += 1;
        }
        write!(formatter, "{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::ByteSize;

    /// Each size reads as its known-good text.
    fn assert_shown(bytes: u64, expected: &str) {
        assert_eq!(ByteSize(bytes).to_string(), expected, "{bytes} bytes");
    }

    #[test]
    fn sizes_below_one_kibibyte_are_whole_bytes() {
        assert_shown(0, "0 B");
        assert_shown(7, "7 B");
        assert_shown(1023, "1023 B");
    }

    #[test]
    fn larger_sizes_take_a_binary_unit_and_one_decimal() {
        assert_shown(1024, "1.0 KiB");
        assert_shown(1536, "1.5 KiB");
        assert_shown(12_897_485, "12.3 MiB");
        assert_shown(3 * 1024 * 1024 * 1024, "3.0 GiB");
        assert_shown(5 << 40, "5.0 TiB");
        assert_shown(5 << 50, "5.0 PiB");
    }

    #[test]
    fn a_size_that_rounds_up_to_1024_takes_the_next_unit() {
        // 1023.96 KiB would round to 1024.0 KiB.
        assert_shown(1_048_535, "1.0 MiB");
        // Just below the rounding boundary it stays in KiB.
        assert_shown(1_048_524, "1023.9 KiB");
    }

    #[test]
    fn the_largest_size_is_in_exbibytes() {
        assert_shown(u64::MAX, "16.0 EiB");
    }
}
