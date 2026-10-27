//! Brotli backward-distance history.
//!
//! Specification: RFC 7392, section 4.
//! <https://www.rfc-editor.org/rfc/rfc7932.html#section-4>

use super::{FontError, error};

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
}
