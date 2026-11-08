//! Decoding one quantized block in an 8-bit baseline JPEG scan.
//!
//! ITU-T T.81, sections F.1.2.1, F.1.2.2 and F.2.2:
//! <https://www.w3.org/Graphics/JPEG/itu-t81.pdf>
//!
//! This does not handle MCU ordering, restart markers, dequantization or IDCT.

use super::{EntropyError, EntropyReader, HuffmanDecodeError, HuffmanDecoder, ZIGZAG_TO_NATURAL};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockDecodeError {
    DcHuffman(HuffmanDecodeError),
    AcHuffman(HuffmanDecodeError),
    Entropy(EntropyError),
    InvalidDcCategory(u8),
    InvalidAcSymbol(u8),
    RunExceedsBlock,
    DcOverflow,
}

impl std::fmt::Display for BlockDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JPEG block decode error: {self:?}")
    }
}

impl std::error::Error for BlockDecodeError {}

/// Decodes quantized coefficients into natural 8x8 row-major order.
///
/// Pass the preceding DC value for this component; use zero at the start of
/// a scan or restart interval. On success, `block[0]` is the next predictor.
/// Bits already read remain consumed on failure.
pub fn decode_baseline_block(
    reader: &mut EntropyReader<'_>,
    dc: &HuffmanDecoder,
    ac: &HuffmanDecoder,
    previous_dc: i32,
) -> Result<[i32; 64], BlockDecodeError> {
    let dc_value = decode_dc(reader, dc, previous_dc)?;

    let mut block = [0; 64];
    block[0] = dc_value;

    let mut zigzag = 1usize;

    while zigzag < 64 {
        let symbol = ac.decode(reader).map_err(BlockDecodeError::AcHuffman)?;

        let run = usize::from(symbol >> 4);
        let category = symbol & 0x0f;

        if category == 0 {
            match run {
                // EOB: all remaining coefficients are zero.
                0 => break,

                // ZRL: sixteen zero coefficients.
                15 => {
                    zigzag += 16;

                    if zigzag > 64 {
                        return Err(BlockDecodeError::RunExceedsBlock);
                    }

                    continue;
                }

                _ => {
                    return Err(BlockDecodeError::InvalidAcSymbol(symbol));
                }
            }
        }

        if category > 10 {
            return Err(BlockDecodeError::InvalidAcSymbol(symbol));
        }

        zigzag += run;

        if zigzag >= 64 {
            return Err(BlockDecodeError::RunExceedsBlock);
        }

        block[ZIGZAG_TO_NATURAL[zigzag]] = read_signed(reader, category)?;

        zigzag += 1;
    }

    Ok(block)
}

pub(super) fn decode_dc(
    reader: &mut EntropyReader<'_>,
    dc: &HuffmanDecoder,
    previous_dc: i32,
) -> Result<i32, BlockDecodeError> {
    let category = dc.decode(reader).map_err(BlockDecodeError::DcHuffman)?;

    if category > 11 {
        return Err(BlockDecodeError::InvalidDcCategory(category));
    }

    let difference = read_signed(reader, category)?;

    previous_dc
        .checked_add(difference)
        .ok_or(BlockDecodeError::DcOverflow)
}

pub(super) fn read_signed(
    reader: &mut EntropyReader<'_>,
    category: u8,
) -> Result<i32, BlockDecodeError> {
    if category == 0 {
        return Ok(0);
    }

    let value = i32::from(
        reader
            .read_bits(category)
            .map_err(BlockDecodeError::Entropy)?,
    );

    let threshold = 1i32 << (category - 1);

    Ok(if value < threshold {
        value + 1 - (1i32 << category)
    } else {
        value
    })
}

#[cfg(test)]
mod tests {
    use super::super::{HuffmanTable, read_first_scan_header, read_huffman_tables};
    use super::*;

    fn table(symbols: &[u8]) -> HuffmanDecoder {
        let mut counts = [0; 16];

        for count in counts.iter_mut().take(symbols.len()) {
            *count = 1;
        }

        HuffmanDecoder::new(&HuffmanTable {
            counts,
            symbols: symbols.to_vec(),
        })
        .unwrap()
    }

    #[test]
    fn decodes_dc_prediction_and_signed_ac_in_zigzag_order() {
        let dc = table(&[0, 2]);
        let ac = table(&[0x00, 0x11]);

        let block = decode_baseline_block(&mut EntropyReader::new(&[0xa8]), &dc, &ac, 10).unwrap();

        assert_eq!(block[0], 12);
        assert_eq!(block[8], -1);

        assert_eq!(block.iter().filter(|&&value| value != 0).count(), 2);
    }

    #[test]
    fn supports_zrl_and_rejects_runs_past_the_block() {
        let dc = table(&[0]);
        let ac = table(&[0x00, 0xf0]);

        // DC=0; four ZRL codes each use 10.
        // The fourth would pass index 63.
        let error =
            decode_baseline_block(&mut EntropyReader::new(&[0x55, 0x00]), &dc, &ac, 0).unwrap_err();

        assert_eq!(error, BlockDecodeError::RunExceedsBlock);

        // DC=0, three ZRL codes, EOB.
        let block = decode_baseline_block(&mut EntropyReader::new(&[0x54]), &dc, &ac, 0).unwrap();

        assert_eq!(block, [0; 64]);
    }

    #[test]
    fn rejects_reserved_symbols_categories_and_dc_overflow() {
        let ac_eob = table(&[0]);

        assert_eq!(
            decode_baseline_block(&mut EntropyReader::new(&[0]), &table(&[12]), &ac_eob, 0)
                .unwrap_err(),
            BlockDecodeError::InvalidDcCategory(12)
        );

        assert_eq!(
            decode_baseline_block(
                &mut EntropyReader::new(&[0]),
                &table(&[0]),
                &table(&[0x10]),
                0
            )
            .unwrap_err(),
            BlockDecodeError::InvalidAcSymbol(0x10)
        );

        assert_eq!(
            decode_baseline_block(
                &mut EntropyReader::new(&[0]),
                &table(&[0]),
                &table(&[0x0b]),
                0
            )
            .unwrap_err(),
            BlockDecodeError::InvalidAcSymbol(0x0b)
        );

        assert_eq!(
            decode_baseline_block(
                &mut EntropyReader::new(&[0x40]),
                &table(&[1]),
                &ac_eob,
                i32::MAX
            )
            .unwrap_err(),
            BlockDecodeError::DcOverflow
        );
    }

    #[test]
    fn decodes_first_quantized_block_of_baseline_wpt_image() {
        // Expected coefficients were cross-checked with libjpeg.
        // This tests a WPT asset, not the original browser test.
        let bytes = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/images/arrow-oriented-upright.jpg"
        ));

        let scan = read_first_scan_header(bytes).unwrap();
        let tables = read_huffman_tables(bytes).unwrap();

        let component = scan.components[0];

        let dc =
            HuffmanDecoder::new(tables.dc[component.dc_table as usize].as_ref().unwrap()).unwrap();

        let ac =
            HuffmanDecoder::new(tables.ac[component.ac_table as usize].as_ref().unwrap()).unwrap();

        let mut reader = EntropyReader::new(&bytes[scan.entropy_offset..]);

        let block = decode_baseline_block(&mut reader, &dc, &ac, 0).unwrap();

        let mut expected = [0; 64];
        expected[0] = -1024;

        assert_eq!(block, expected);
    }
}
