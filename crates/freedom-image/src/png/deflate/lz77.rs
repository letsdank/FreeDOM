//! Greedy, bounded-chain LZ77 tokenization for DEFLATE.
//!
//! RFC 1951, sections 3.2.3 and 3.2.5:
//! <https://www.rfc-editor.org/rfc/rfc1951#section-3.2.5>

use std::io;

const WINDOW: usize = 32_768;
const HASH_SIZE: usize = 32_768;
const MAX_CHAIN: usize = 64;
const MAX_MATCH: usize = 258;
const NONE: usize = usize::MAX;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Lz77Token {
    Literal(u8),

    Match { length: u16, distance: u16 },
}

/// Greedy match finder with a 32 KiB history and bounded hash chains.
///
/// At most 64 candidates are inspected per token. Overlapping matches
/// are allowed. Compression ratio is not guaranteed to be optimal.
pub struct Lz77Finder<'a> {
    input: &'a [u8],
    position: usize,

    heads: Vec<usize>,
    previous: Vec<usize>,
}

impl<'a> Lz77Finder<'a> {
    pub fn new(input: &'a [u8]) -> io::Result<Self> {
        fn table(size: usize) -> io::Result<Vec<usize>> {
            let mut result = Vec::new();

            result
                .try_reserve_exact(size)
                .map_err(|_| io::Error::other("could not allocate LZ77 history"))?;

            result.resize(size, NONE);

            Ok(result)
        }

        Ok(Self {
            input,
            position: 0,
            heads: table(HASH_SIZE)?,
            previous: table(WINDOW)?,
        })
    }

    fn hash(&self, position: usize) -> usize {
        let a = usize::from(self.input[position]);
        let b = usize::from(self.input[position + 1]);
        let c = usize::from(self.input[position + 2]);

        ((a * 251 + b) * 251 + c) & (HASH_SIZE - 1)
    }

    fn insert(&mut self, position: usize) {
        if self.input.len() - position < 3 {
            return;
        }

        let hash = self.hash(position);

        self.previous[position % WINDOW] = self.heads[hash];
        self.heads[hash] = position;
    }

    fn best_match(&self) -> Option<(usize, usize)> {
        let remaining = self.input.len() - self.position;

        if remaining < 3 {
            return None;
        }

        let max_length = remaining.min(MAX_MATCH);

        let mut candidate = self.heads[self.hash(self.position)];
        let mut best_length = 0;
        let mut best_distance = 0;
        let mut visited = 0;

        while candidate != NONE && visited < MAX_CHAIN {
            if candidate >= self.position || self.position - candidate > WINDOW {
                break;
            }

            let mut length = 0;

            while length < max_length
                && self.input[candidate + length] == self.input[self.position + length]
            {
                length += 1;
            }

            if length >= 3 && length > best_length {
                best_length = length;
                best_distance = self.position - candidate;

                if length == max_length {
                    break;
                }
            }

            candidate = self.previous[candidate % WINDOW];
            visited += 1;
        }

        if best_length == 0 {
            None
        } else {
            Some((best_length, best_distance))
        }
    }
}

impl Iterator for Lz77Finder<'_> {
    type Item = Lz77Token;

    fn next(&mut self) -> Option<Self::Item> {
        if self.position == self.input.len() {
            return None;
        }

        let (token, length) = match self.best_match() {
            Some((length, distance)) => (
                Lz77Token::Match {
                    length: length as u16,
                    distance: distance as u16,
                },
                length,
            ),

            None => (Lz77Token::Literal(self.input[self.position]), 1),
        };

        let end = self.position + length;

        for position in self.position..end {
            self.insert(position);
        }

        self.position = end;

        Some(token)
    }
}

#[cfg(test)]
#[path = "lz77_tests.rs"]
mod tests;
