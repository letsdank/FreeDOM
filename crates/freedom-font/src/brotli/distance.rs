//! Brotli backward-distance history.
//!
//! Specification: RFC 7932, section 4.
//! <https://www.rfc-editor.org/rfc/rfc7932.html#section-4>

use super::{BitReader, FontError, error};

/// The four most recent distances, ordered from newest to oldest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DistanceHistory {
    distances: [usize; 4],
}

impl DistanceHistory {
    pub(super) fn new() -> Self {
        Self {
            distances: [4, 11, 15, 16],
        }
    }

    /// Resolves one of the 16 short distance symbols.
    ///
    /// `offset` is the current bit position, used for error reporting.
    pub(super) fn resolve_short(&self, symbol: usize, offset: usize) -> Result<usize, FontError> {
        match symbol {
            0..=3 => Ok(self.distances[symbol]),

            4..=15 => {
                let (history_index, relative_symbol) = if symbol < 10 {
                    (0, symbol - 4)
                } else {
                    (1, symbol - 10)
                };

                let delta = relative_symbol / 2 + 1;
                let base = self.distances[history_index];

                let distance = if relative_symbol % 2 == 0 {
                    base.checked_sub(delta)
                } else {
                    base.checked_add(delta)
                };

                match distance {
                    Some(distance) if distance > 0 => Ok(distance),
                    _ => Err(error(offset, "invalid Brotli short distance")),
                }
            }

            _ => Err(error(
                offset,
                "Brotli short distance symbol exceeds its alphabet",
            )),
        }
    }

    /// Records an accepted distance after its source has been determined.
    ///
    /// Symbol 0 and static dictionary references do not update the history.
    pub(super) fn record(
        &mut self,
        symbol: usize,
        distance: usize,
        is_dictionary_reference: bool,
        offset: usize,
    ) -> Result<(), FontError> {
        if distance == 0 {
            return Err(error(offset, "Brotli distance must be positive"));
        }

        if symbol != 0 && !is_dictionary_reference {
            self.distances = [
                distance,
                self.distances[0],
                self.distances[1],
                self.distances[2],
            ];
        }

        Ok(())
    }
}

/// Resolves a distance symbol and reads its extra bits when required.
///
/// This does not update the distance history. The caller must first determine
/// whether the distance refers to previously decoded data or the dictionary.
pub(super) fn read_distance(
    reader: &mut BitReader<'_>,
    symbol: usize,
    postfix_bits: u8,
    direct_code_count: usize,
    history: &DistanceHistory,
) -> Result<usize, FontError> {
    if postfix_bits > 3 {
        return Err(error(
            reader.bit_position(),
            "invalid Brotli distance postfix bit count",
        ));
    }

    let postfix = usize::from(postfix_bits);
    let postfix_factor = 1_usize << postfix;

    // NDIRECT is a four-bit value shifted left by NPOSTFIX.
    if direct_code_count > 15 * postfix_factor || direct_code_count % postfix_factor != 0 {
        return Err(error(
            reader.bit_position(),
            "invalid Brotli direct distance code count",
        ));
    }

    let alphabet_size = 16 + direct_code_count + (48 << postfix);

    if symbol >= alphabet_size {
        return Err(error(
            reader.bit_position(),
            "Brotli distance symbol exceeds its alphabet",
        ));
    }

    if symbol < 16 {
        return history.resolve_short(symbol, reader.bit_position());
    }

    if symbol < 16 + direct_code_count {
        return Ok(symbol - 15);
    }

    let encoded = symbol - 16 - direct_code_count;
    let high_code = encoded >> postfix;
    let low_code = encoded & (postfix_factor - 1);
    let extra_bit_count = 1 + (encoded >> (postfix + 1));

    let extra = reader.read_bits(extra_bit_count)? as usize;

    // RFC 7932, section 4: conversion from a distance code to a distance.
    let base = ((2 + (high_code & 1)) << extra_bit_count) - 4;

    Ok(((base + extra) << postfix) + low_code + direct_code_count + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_all_short_distance_symbols() {
        let history = DistanceHistory::new();

        let expected = [4, 11, 15, 16, 3, 5, 2, 6, 1, 7, 10, 12, 9, 13, 8, 14];

        for (symbol, distance) in expected.into_iter().enumerate() {
            assert_eq!(history.resolve_short(symbol, 0).unwrap(), distance);
        }

        assert!(history.resolve_short(16, 0).is_err());
    }

    #[test]
    fn updates_history_only_for_nonzero_copy_symbols() {
        let mut history = DistanceHistory::new();

        history.record(16, 12, false, 0).unwrap();

        assert_eq!(
            (0..4)
                .map(|symbol| history.resolve_short(symbol, 0).unwrap())
                .collect::<Vec<_>>(),
            vec![12, 4, 11, 15],
        );

        history.record(0, 12, false, 0).unwrap();
        history.record(16, 100, true, 0).unwrap();

        assert_eq!(
            (0..4)
                .map(|symbol| history.resolve_short(symbol, 0).unwrap())
                .collect::<Vec<_>>(),
            vec![12, 4, 11, 15],
        );
    }

    #[test]
    fn rejects_zero_or_negative_short_distances() {
        let mut history = DistanceHistory::new();
        history.record(16, 1, false, 0).unwrap();

        assert!(history.resolve_short(4, 7).is_err());
        assert!(history.resolve_short(6, 7).is_err());
        assert!(history.record(16, 0, false, 7).is_err());
    }

    #[test]
    fn reads_direct_brotli_distances_without_extra_bits() {
        let history = DistanceHistory::new();
        let mut reader = BitReader::new(&[]);

        assert_eq!(read_distance(&mut reader, 0, 0, 4, &history).unwrap(), 4);
        assert_eq!(read_distance(&mut reader, 16, 0, 4, &history).unwrap(), 1);
        assert_eq!(read_distance(&mut reader, 19, 0, 4, &history).unwrap(), 4);
        assert_eq!(reader.bit_position(), 0);
    }

    #[test]
    fn reads_brotli_distances_with_extra_bits() {
        let history = DistanceHistory::new();

        let mut reader = BitReader::new(&[0b0000_0001]);
        assert_eq!(read_distance(&mut reader, 17, 0, 0, &history).unwrap(), 4);
        assert_eq!(reader.bit_position(), 1);

        let mut reader = BitReader::new(&[0b0000_0011]);
        assert_eq!(read_distance(&mut reader, 18, 0, 0, &history).unwrap(), 8);
        assert_eq!(reader.bit_position(), 2);
    }

    #[test]
    fn reads_brotli_distances_with_postfix_bits() {
        let history = DistanceHistory::new();

        let mut reader = BitReader::new(&[0b0000_0001]);
        assert_eq!(read_distance(&mut reader, 20, 1, 4, &history).unwrap(), 7);

        let mut reader = BitReader::new(&[0b0000_0001]);
        assert_eq!(read_distance(&mut reader, 21, 1, 4, &history).unwrap(), 8);

        let mut reader = BitReader::new(&[0b0000_0011]);
        assert_eq!(read_distance(&mut reader, 24, 1, 4, &history).unwrap(), 19);
        assert_eq!(reader.bit_position(), 2);
    }

    #[test]
    fn rejects_invalid_or_truncated_brotli_distance_codes() {
        let history = DistanceHistory::new();

        let mut reader = BitReader::new(&[]);
        assert!(read_distance(&mut reader, 16, 4, 0, &history).is_err());
        assert!(read_distance(&mut reader, 16, 1, 3, &history).is_err());
        assert!(read_distance(&mut reader, 116, 1, 4, &history).is_err());

        // Symbol 16 requires one extra bit when there are no direct codes.
        assert!(read_distance(&mut reader, 16, 0, 0, &history).is_err());
    }
}
