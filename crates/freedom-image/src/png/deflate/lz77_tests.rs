use super::{Lz77Finder, Lz77Token};

fn restore(tokens: &[Lz77Token]) -> Vec<u8> {
    let mut output = Vec::new();

    for token in tokens {
        match *token {
            Lz77Token::Literal(byte) => {
                output.push(byte);
            }

            Lz77Token::Match { length, distance } => {
                assert!((3..=258).contains(&length));
                assert!((1..=32_768).contains(&distance));
                assert!(usize::from(distance) <= output.len());

                // Read from the growing output to allow overlap.
                for _ in 0..length {
                    let source = output.len() - usize::from(distance);

                    output.push(output[source]);
                }
            }
        }
    }

    output
}

#[test]
fn recognizes_overlapping_and_non_overlapping_matches() {
    let tokens: Vec<_> = Lz77Finder::new(b"aaaaaaaaaa").unwrap().collect();

    assert_eq!(
        tokens,
        [
            Lz77Token::Literal(b'a'),
            Lz77Token::Match {
                length: 9,
                distance: 1,
            }
        ]
    );

    let tokens: Vec<_> = Lz77Finder::new(b"abcdabcd").unwrap().collect();

    assert_eq!(
        tokens,
        [
            Lz77Token::Literal(b'a'),
            Lz77Token::Literal(b'b'),
            Lz77Token::Literal(b'c'),
            Lz77Token::Literal(b'd'),
            Lz77Token::Match {
                length: 4,
                distance: 4,
            }
        ]
    );
}

#[test]
fn respects_maximum_match_length() {
    let input = vec![42; 520];
    let tokens: Vec<_> = Lz77Finder::new(&input).unwrap().collect();

    assert_eq!(tokens[0], Lz77Token::Literal(42));

    assert_eq!(
        tokens[1],
        Lz77Token::Match {
            length: 258,
            distance: 1
        }
    );

    assert_eq!(restore(&tokens), input);
}

fn token_at_boundary(boundary: usize) -> Lz77Token {
    let mut input = vec![0, 1, 2];

    input.resize(boundary, 255);
    input.extend_from_slice(&[0, 1, 2]);

    let mut position = 0;

    for token in Lz77Finder::new(&input).unwrap() {
        if position == boundary {
            return token;
        }

        position += match token {
            Lz77Token::Literal(_) => 1,
            Lz77Token::Match { length, .. } => usize::from(length),
        };
    }

    panic!("no token at boundary {boundary}");
}

#[test]
fn accepts_the_window_boundary_and_rejects_older_data() {
    assert_eq!(
        token_at_boundary(32_768),
        Lz77Token::Match {
            length: 3,
            distance: 32_768
        }
    );

    assert_eq!(token_at_boundary(32_769), Lz77Token::Literal(0));
}

#[test]
fn reconstructs_varied_inputs() {
    let mut random = Vec::new();
    let mut state = 0x1234_5678u32;

    for _ in 0..80_000 {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;

        random.push((state >> 24) as u8);
    }

    let inputs = [
        Vec::new(),
        vec![0],
        vec![1, 2],
        b"abracadabra!".repeat(8_000),
        (0..=255).collect::<Vec<u8>>(),
        random,
    ];

    for input in inputs {
        let tokens: Vec<_> = Lz77Finder::new(&input).unwrap().collect();

        assert_eq!(restore(&tokens), input);
    }
}

#[test]
fn iterator_stays_exhausted() {
    let mut finder = Lz77Finder::new(b"x").unwrap();

    assert_eq!(finder.next(), Some(Lz77Token::Literal(b'x')));
    assert_eq!(finder.next(), None);
    assert_eq!(finder.next(), None);
}
