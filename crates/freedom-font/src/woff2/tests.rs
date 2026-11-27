use super::*;

#[test]
fn parses_a_null_transformed_woff2_directory() {
    let mut data = minimal_woff2(1, &[5, 4], &[0]);

    data[16..20].copy_from_slice(&32_u32.to_be_bytes());

    let directory = parse_woff2_directory(&data).unwrap();

    assert_eq!(directory.flavor, 0x0001_0000);
    assert_eq!(directory.total_sfnt_size, 32);
    assert_eq!(directory.compressed_data, 50..51);
    assert_eq!(directory.tables.len(), 1);
    assert_eq!(directory.tables[0].tag, TableTag::NAME);
    assert_eq!(directory.tables[0].original_length, 4);
    assert_eq!(directory.tables[0].transformed_length, None);
    assert_eq!(directory.tables[0].transform, Woff2Transform::Null);
}

#[test]
fn parses_transformed_glyf_and_loca_directory_entries() {
    let data = minimal_woff2(2, &[10, 8, 6, 11, 4, 0], &[0]);

    let directory = parse_woff2_directory(&data).unwrap();

    assert_eq!(directory.compressed_data, 54..55);
    assert_eq!(directory.tables[0].tag, TableTag::GLYF);
    assert_eq!(directory.tables[0].original_length, 8);
    assert_eq!(directory.tables[0].transformed_length, Some(6));
    assert_eq!(directory.tables[0].transform, Woff2Transform::GlyfLoca);
    assert_eq!(directory.tables[1].tag, TableTag::LOCA);
    assert_eq!(directory.tables[1].transformed_length, Some(0));
}

#[test]
fn rejects_uint_base_128_with_a_leading_zero() {
    let data = minimal_woff2(1, &[5, 0x80, 4], &[0]);

    let error = parse_woff2_directory(&data).unwrap_err();

    assert_eq!(error.message, "WOFF2 UIntBase128 has a leading zero");
}

#[test]
fn decodes_an_untransformed_woff2_font() {
    let compressed = uncompressed_brotli(b"abcd");
    let mut data = minimal_woff2(1, &[5, 4], &compressed);

    data[16..20].copy_from_slice(&32_u32.to_be_bytes());

    let sfnt = decode_woff2(&data).unwrap();

    assert_eq!(sfnt.len(), 32);
    assert_eq!(&sfnt[0..4], &0x0001_0000_u32.to_be_bytes());
    assert_eq!(&sfnt[4..6], &1_u16.to_be_bytes());
    assert_eq!(&sfnt[12..16], b"name");
    assert_eq!(&sfnt[16..20], &0x6162_6364_u32.to_be_bytes());
    assert_eq!(&sfnt[20..24], &28_u32.to_be_bytes());
    assert_eq!(&sfnt[24..28], &4_u32.to_be_bytes());
    assert_eq!(&sfnt[28..32], b"abcd");
}

#[test]
fn rejects_invalid_hmtx_compressed_data() {
    let data = minimal_woff2(1, &[0x43, 4, 3], &[0x06]);

    assert!(decode_woff2(&data).is_err());
}

#[test]
fn parses_distinct_cvt_and_fdsc_known_tags() {
    // Known tag indexes 8 and 44.
    let data = minimal_woff2(2, &[8, 4, 44, 4], &[0]);
    let directory = parse_woff2_directory(&data).unwrap();

    assert_eq!(directory.tables[0].tag.bytes(), *b"cvt ");
    assert_eq!(directory.tables[1].tag.bytes(), *b"fdsc");
}

#[test]
fn reconstructs_woff2_table_from_compressed_brotli() {
    use crate::sfnt::SfntFont;

    // Brotli stream producing eleven 'a' bytes using an overlapping copy.
    // The stream was independently verified with libbrolidec.
    let compressed = [0x42, 0x01, 0x00, 0x04, 0x44, 0x58, 0x20, 0x13, 0x10];

    // One untransformed `name` table containing eleven bytes.
    let mut data = minimal_woff2(1, &[5, 11], &compressed);
    data[16..20].copy_from_slice(&40_u32.to_be_bytes());

    let sfnt_data = decode_woff2(&data).unwrap();
    let font = SfntFont::parse(&sfnt_data).unwrap();

    assert_eq!(sfnt_data.len(), 40);
    assert_eq!(font.table(TableTag::NAME), Some(&b"aaaaaaaaaaa"[..]));
}

#[test]
fn reconstructs_woff2_when_total_sfnt_size_differs() {
    use crate::sfnt::SfntFont;

    let compressed = uncompressed_brotli(b"abcd");

    // The reconstructed SFNT occupies 32 bytes. The header value is
    // deliberately varied to check that it is not used as an allocation size.
    for declared_size in [0_u32, 16, 32, 40, u32::MAX] {
        let mut data = minimal_woff2(1, &[5, 4], &compressed);
        data[16..20].copy_from_slice(&declared_size.to_be_bytes());

        let sfnt_data = decode_woff2(&data).unwrap();
        let font = SfntFont::parse(&sfnt_data).unwrap();

        assert_eq!(sfnt_data.len(), 32, "declared size: {declared_size}");
        assert_eq!(
            font.table(TableTag::NAME),
            Some(&b"abcd"[..]),
            "declared size: {declared_size}"
        );
    }
}

#[test]
fn parses_real_wpt_woff2_directory() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/wpt/css/WOFF2/support/",
        "metadatadisplay-schema-metadata-004.woff2",
    );

    let data = std::fs::read(path).expect("WPT WOFF2 fixture must exist");

    assert!(data.starts_with(b"wOF2"), "fixture is not a WOFF2 file");

    let directory = parse_woff2_directory(&data).unwrap();

    assert!(!directory.tables.is_empty());
    assert!(directory.compressed_data.start < directory.compressed_data.end);
    assert!(directory.compressed_data.end <= data.len());

    println!(
        "WOFF2 fixture: {} byte, {} tables, {} compressed bytes",
        data.len(),
        directory.tables.len(),
        directory.compressed_data.len()
    );

    for table in &directory.tables {
        println!(
            "  {:?}: original={}, transformed={:?}, transform={:?}",
            table.tag, table.original_length, table.transformed_length, table.transform,
        );
    }
}

#[test]
fn decodes_real_wpt_woff2_fixture() {
    use crate::sfnt::SfntFont;

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/wpt/css/WOFF2/support/",
        "metadatadisplay-schema-metadata-004.woff2",
    );

    let data = std::fs::read(path).expect("WPT WOFF2 fixture must exist");
    let directory = parse_woff2_directory(&data).unwrap();

    let decoded = decode_brotli(&data[directory.compressed_data.clone()], 1691).unwrap();

    assert_eq!(decoded.len(), 1691);

    // Check the decompressed bytes against an independent Brotli decoder.
    let mut crc = 0xffff_ffff_u32;

    for byte in decoded {
        crc ^= u32::from(byte);

        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }

    assert_eq!(!crc, 0x293b_f12b);

    let sfnt_data = decode_woff2(&data).unwrap();
    let font = SfntFont::parse(&sfnt_data).unwrap();

    assert_eq!(font.table(TableTag::NAME).unwrap().len(), 663);
}

#[test]
fn decompresses_transformed_table_lengths() {
    let compressed = uncompressed_brotli(b"abcdef");

    // Transformed glyf: original length 8, transformed length 6.
    // Transformed loca: original length 4, transformed length 0.
    let data = minimal_woff2(2, &[10, 8, 6, 11, 4, 0], &compressed);

    let directory = parse_woff2_directory(&data).unwrap();
    let decoded = decode_table_data(&data, &directory).unwrap();

    assert_eq!(decoded, b"abcdef");

    // Six bytes are not a valid transformed glyf table.
    assert!(decode_woff2(&data).is_err());
}

#[test]
fn rejects_shorter_transformed_table_data() {
    let compressed = uncompressed_brotli(b"abcde");

    // The directory declares six decompressed bytes, but the stream
    // contains only five.
    let data = minimal_woff2(2, &[10, 8, 6, 11, 4, 0], &compressed);

    let directory = parse_woff2_directory(&data).unwrap();
    let error = decode_table_data(&data, &directory).unwrap_err();

    assert_eq!(error.message, "WOFF2 Brotli data has an unexpected length");
}

#[test]
fn splits_mixed_transformed_and_untransformed_table_data() {
    let compressed = uncompressed_brotli(b"xyzabcdef");

    // name: original length 3.
    // glyf: original length 8, transformed length 6.
    // loca: original length 4, transformed length 0.
    let data = minimal_woff2(3, &[5, 3, 10, 8, 6, 11, 4, 0], &compressed);

    let directory = parse_woff2_directory(&data).unwrap();
    let decoded = decode_table_data(&data, &directory).unwrap();
    let tables = split_table_data(&decoded, &directory).unwrap();

    assert_eq!(
        tables,
        vec![
            (TableTag::NAME, &b"xyz"[..]),
            (TableTag::GLYF, &b"abcdef"[..]),
            (TableTag::LOCA, &b""[..]),
        ]
    );
}

#[test]
fn rejects_incomplete_or_extra_table_data() {
    let compressed = uncompressed_brotli(b"abcdef");
    let data = minimal_woff2(2, &[10, 8, 6, 11, 4, 0], &compressed);

    let directory = parse_woff2_directory(&data).unwrap();

    let truncated = split_table_data(b"abcde", &directory).unwrap_err();

    assert_eq!(truncated.message, "WOFF2 table data is truncated");

    let extra = split_table_data(b"abcdefg", &directory).unwrap_err();

    assert_eq!(extra.message, "WOFF2 table data has trailing bytes");
}

#[test]
fn parses_glyf_from_decompressed_table_data() {
    // One empty glyph: two contour bytes and a four-byte bbox bitmap.
    let mut transformed = vec![0_u8; 42];

    write_u16(&mut transformed, 4, 1).unwrap();
    write_u32(&mut transformed, 8, 2).unwrap();
    write_u32(&mut transformed, 28, 4).unwrap();

    let compressed = uncompressed_brotli(&transformed);

    // glyf: original length 10, transformed length 42.
    // loca: original length 4, transformed length 0.
    let data = minimal_woff2(2, &[10, 10, 42, 11, 4, 0], &compressed);

    let directory = parse_woff2_directory(&data).unwrap();
    let decoded = decode_table_data(&data, &directory).unwrap();
    let tables = split_table_data(&decoded, &directory).unwrap();

    assert_eq!(tables[0].0, TableTag::GLYF);

    let streams = glyf::parse(tables[0].1).unwrap();

    assert_eq!(streams.num_glyphs, 1);
    assert_eq!(streams.contours, &[0, 0]);
    assert_eq!(streams.bbox_bitmap, &[0, 0, 0, 0]);
    assert_eq!(tables[1], (TableTag::LOCA, &b""[..]));
}

#[test]
fn accepts_other_tables_between_glyf_and_loca() {
    let directories: &[&[u8]] = &[
        // Transformed glyf, hhea, transformed loca.
        &[10, 8, 6, 2, 2, 11, 4, 0],
        // Untransformed glyf, hhea, untransformed loca.
        &[0xca, 8, 2, 2, 0xcb, 4],
    ];

    for &directory in directories {
        let data = minimal_woff2(3, directory, &[0]);
        let parsed = parse_woff2_directory(&data).unwrap();

        assert_eq!(parsed.tables[0].tag, TableTag::GLYF);
        assert_eq!(parsed.tables[1].tag, TableTag::HHEA);
        assert_eq!(parsed.tables[2].tag, TableTag::LOCA);
    }
}

#[test]
fn rejects_reversed_or_mismatched_glyf_loca_pairs() {
    let reversed: &[&[u8]] = &[
        // Transformed loca before transformed glyf.
        &[11, 4, 0, 10, 8, 6],
        // Untransformed loca before untransformed glyf.
        &[0xcb, 4, 0xca, 8],
    ];

    for &directory in reversed {
        let data = minimal_woff2(2, directory, &[0]);
        let error = parse_woff2_directory(&data).unwrap_err();

        assert_eq!(error.message, "WOFF2 loca table must follow glyf");
    }

    let mismatched: &[&[u8]] = &[
        // Transformed glyf, untransformed loca.
        &[10, 8, 6, 0xcb, 4],
        // Untransformed glyf, transformed loca.
        &[0xca, 8, 11, 4, 0],
    ];

    for &directory in mismatched {
        let data = minimal_woff2(2, directory, &[0]);
        let error = parse_woff2_directory(&data).unwrap_err();

        assert_eq!(error.message, "WOFF2 glyf and loca transforms must match");
    }
}

#[test]
fn rejects_unpaired_transformed_glyf_and_loca() {
    let data = minimal_woff2(1, &[10, 8, 6], &[0]);
    let error = parse_woff2_directory(&data).unwrap_err();

    assert_eq!(
        error.message,
        "transformed WOFF2 glyf table is missing loca"
    );

    let data = minimal_woff2(1, &[11, 4, 0], &[0]);
    let error = parse_woff2_directory(&data).unwrap_err();

    assert_eq!(
        error.message,
        "transformed WOFF2 loca table is missing glyf"
    );
}

fn minimal_woff2(table_count: u16, directory: &[u8], compressed: &[u8]) -> Vec<u8> {
    let length = WOFF2_HEADER_LENGTH + directory.len() + compressed.len();
    let mut data = vec![0_u8; length];

    data[0..4].copy_from_slice(&WOFF2_SIGNATURE.to_be_bytes());
    data[4..8].copy_from_slice(&0x0001_0000_u32.to_be_bytes());
    data[8..12].copy_from_slice(&(length as u32).to_be_bytes());
    data[12..14].copy_from_slice(&table_count.to_be_bytes());
    data[20..24].copy_from_slice(&(compressed.len() as u32).to_be_bytes());

    let directory_end = WOFF2_HEADER_LENGTH + directory.len();
    data[WOFF2_HEADER_LENGTH..directory_end].copy_from_slice(directory);
    data[directory_end..].copy_from_slice(compressed);

    data
}

fn uncompressed_brotli(payload: &[u8]) -> Vec<u8> {
    assert!(!payload.is_empty());
    assert!(payload.len() <= 65_536);

    let mut data = Vec::new();
    let mut bit_position = 0_usize;

    append_bits(&mut data, &mut bit_position, 0, 1);
    append_bits(&mut data, &mut bit_position, 0, 1);
    append_bits(&mut data, &mut bit_position, 0, 2);
    append_bits(
        &mut data,
        &mut bit_position,
        u32::try_from(payload.len() - 1).unwrap(),
        16,
    );
    append_bits(&mut data, &mut bit_position, 1, 1);

    while !bit_position.is_multiple_of(8) {
        append_bits(&mut data, &mut bit_position, 0, 1);
    }

    data.extend_from_slice(payload);
    bit_position += payload.len() * 8;

    append_bits(&mut data, &mut bit_position, 1, 1);
    append_bits(&mut data, &mut bit_position, 1, 1);

    while !bit_position.is_multiple_of(8) {
        append_bits(&mut data, &mut bit_position, 0, 1);
    }

    data
}

fn transformed_glyf_fixture(maxp_glyphs: u16, head_format: u16, loca_length: u8) -> Vec<u8> {
    let mut head = vec![0_u8; 54];
    write_u16(&mut head, 50, head_format).unwrap();

    let mut maxp = vec![0_u8; 6];
    write_u32(&mut maxp, 0, 0x0001_0000).unwrap();
    write_u16(&mut maxp, 4, maxp_glyphs).unwrap();

    let mut glyf = vec![0_u8; 36];
    write_u16(&mut glyf, 4, 2).unwrap();

    let sizes = [4_u32, 1, 3, 5, 0, 4, 2];

    for (index, size) in sizes.into_iter().enumerate() {
        write_u32(&mut glyf, 8 + index * 4, size).unwrap()
    }

    glyf.extend_from_slice(&[
        0, 0, 0, 1, // empty glyph, then one-contour glyph
        3, // three points
        1, 11, 86, // point flags
        0, 50, 24, 79, // coordinates
        2,  // instruction length
        0, 0, 0, 0, // bbox bitmap
        0xaa, 0xbb, // instructions
    ]);

    let mut raw = head;
    raw.extend_from_slice(&maxp);
    raw.extend_from_slice(&glyf);

    let compressed = uncompressed_brotli(&raw);

    minimal_woff2(
        4,
        &[
            1,
            54, // head
            4,
            6, // maxp
            10,
            20,
            55, // transformed glyf
            11,
            loca_length,
            0, // transformed loca
        ],
        &compressed,
    )
}

#[test]
fn decodes_transformed_glyf_loca_in_a_woff2_container() {
    use crate::{
        glyf::{Glyph, parse_glyph},
        sfnt::SfntFont,
    };

    let data = transformed_glyf_fixture(2, 0, 6);
    let sfnt = decode_woff2(&data).unwrap();
    let font = SfntFont::parse(&sfnt).unwrap();

    let glyf = font.table(TableTag::GLYF).unwrap();

    assert_eq!(font.table(TableTag::LOCA), Some(&[0, 0, 0, 0, 0, 11][..]));

    assert_eq!(glyf.len(), 22);

    let Glyph::Simple(glyph) = parse_glyph(glyf, 0).unwrap() else {
        panic!("expected a simple glyph");
    };

    assert_eq!(glyph.bounds.x_max, 50);
    assert_eq!(glyph.bounds.y_max, 80);
}

#[test]
fn rejects_transformed_glyf_with_inconsistent_metadata() {
    let cases = [
        (
            transformed_glyf_fixture(3, 0, 6),
            "WOFF2 glyf glyph count does not match maxp",
        ),
        (
            transformed_glyf_fixture(2, 1, 6),
            "WOFF2 glyf index format does not match head",
        ),
        (
            transformed_glyf_fixture(2, 0, 8),
            "WOFF2 loca original length does not match glyph count",
        ),
    ];

    for (data, expected_message) in cases {
        let error = decode_woff2(&data).unwrap_err();

        assert_eq!(error.message, expected_message);
    }
}

#[test]
fn decodes_real_wpt_noto_nko_transformed_glyf() {
    use crate::{glyf::Glyph, truetype::TrueTypeFont};
    use freedom_graphics_core::geometry::{PathCommand, Point};

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/wpt/fonts/noto/",
        "NotoSansNko-regular-webfont.woff2",
    );

    let data = std::fs::read(path).expect("WPT Noto Sans NKo fixture must exist");

    let directory = parse_woff2_directory(&data).unwrap();

    for tag in [TableTag::GLYF, TableTag::LOCA] {
        let table = directory
            .tables
            .iter()
            .find(|table| table.tag == tag)
            .expect("fixture must contain glyf and loca");

        assert_eq!(table.transform, Woff2Transform::GlyfLoca);
    }

    let sfnt = decode_woff2(&data).unwrap();
    let font = TrueTypeFont::parse(&sfnt).unwrap();

    assert_eq!(font.maxp().num_glyphs, 190);

    // NKO LETTER A. Expected glyph and metrics were checked with
    // FreeType using the original WOFF2 font.
    let glyph_id = font.glyph_id('\u{07ca}');

    assert_eq!(glyph_id, 20);
    assert_eq!(font.metric(glyph_id).unwrap().advance_width, 543);

    let Glyph::Simple(glyph) = font.glyph(glyph_id).unwrap() else {
        panic!("expected a simple glyph");
    };

    assert_eq!(
        glyph.path.commands(),
        &[
            PathCommand::MoveTo(Point::new(362.0, 1462.0)),
            PathCommand::LineTo(Point::new(362.0, 0.0)),
            PathCommand::LineTo(Point::new(180.0, 0.0)),
            PathCommand::LineTo(Point::new(180.0, 1462.0)),
            PathCommand::Close,
        ]
    );
}

#[test]
fn decodes_wpt_mongolian_composite_glyph() {
    use crate::{
        glyf::{Glyph, GlyphBounds},
        truetype::TrueTypeFont,
    };
    use freedom_graphics_core::geometry::{Point, Transform};

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/wpt/fonts/noto/",
        "NotoSansMongolian-regular.woff2"
    );

    let data = std::fs::read(path).expect("WPT Noto Sans Mongolian fixture must exist");

    let directory = parse_woff2_directory(&data).unwrap();

    let index = |tag| {
        directory
            .tables
            .iter()
            .position(|table| table.tag == tag)
            .expect("required table is missing")
    };

    let glyf_index = index(TableTag::GLYF);
    let hhea_index = index(TableTag::HHEA);
    let loca_index = index(TableTag::LOCA);

    // This font has other tables between glyf and loca.
    assert!(glyf_index < hhea_index);
    assert!(hhea_index < loca_index);

    assert_eq!(
        directory.tables[glyf_index].transform,
        Woff2Transform::GlyfLoca,
    );

    assert_eq!(
        directory.tables[loca_index].transform,
        Woff2Transform::GlyfLoca,
    );

    let sfnt = decode_woff2(&data).unwrap();
    let font = TrueTypeFont::parse(&sfnt).unwrap();

    assert_eq!(font.maxp().num_glyphs, 1513);

    // Glyph 9 contains four translated copies of glyph 10.
    // Values were checked with FreeType using the original WOFF2.
    let Glyph::Compound(glyph) = font.glyph(9).unwrap() else {
        panic!("expected a composite glyph");
    };

    assert_eq!(
        glyph.bounds,
        GlyphBounds {
            x_min: 147,
            y_min: 694,
            x_max: 1962,
            y_max: 971,
        }
    );

    assert_eq!(glyph.components.len(), 4);

    let offsets = [-385.0, 135.0, 655.0, 1176.0];

    for (component, x) in glyph.components.iter().zip(offsets) {
        assert_eq!(component.glyph_id, 10);
        assert_eq!(component.transform, Transform::translate(x, 0.0));
    }

    // Check the path after resolving and transforming the components.
    let path = font.glyph_path(9).unwrap();
    let bounds = path.control_bounds().unwrap();

    assert_eq!(bounds.min, Point::new(147.0, 694.0));
    assert_eq!(bounds.max, Point::new(1962.0, 971.0));
}

fn transformed_hmtx_fixture(transformed_glyf: bool) -> Vec<u8> {
    let mut head = vec![0_u8; 54];
    write_u16(&mut head, 50, 0).unwrap();

    let mut maxp = vec![0_u8; 6];
    write_u32(&mut maxp, 0, 0x0001_0000).unwrap();
    write_u16(&mut maxp, 4, 2).unwrap();

    let mut hhea = vec![0_u8; 36];
    write_u16(&mut hhea, 34, 2).unwrap();

    let mut raw = head;
    raw.extend_from_slice(&maxp);
    raw.extend_from_slice(&hhea);

    let directory: Vec<u8> = if transformed_glyf {
        let mut glyf = vec![0_u8; 36];

        write_u16(&mut glyf, 4, 2).unwrap();

        let sizes = [4_u32, 1, 3, 5, 0, 4, 0];

        for (index, size) in sizes.into_iter().enumerate() {
            write_u32(&mut glyf, 8 + index * 4, size).unwrap();
        }

        glyf.extend_from_slice(&[
            0, 0, 0, 1, // empty glyph, then one contour
            3, // point count
            11, 11, 86, // flags
            10, 50, 24, 79, // coordinate data
            0,  // instruction length
            0, 0, 0, 0, // bbox bitmap
        ]);

        raw.extend_from_slice(&glyf);

        // Both bearing arrays are omitted.
        raw.extend_from_slice(&[3, 1, 244, 2, 88]);

        vec![
            1, 54, // head
            4, 6, // maxp
            2, 36, // hhea
            10, 20, 53, // transformed glyf
            0x43, 8, 5, // transformed hmtx
            11, 6, 0, // transformed loca
        ]
    } else {
        // Both glyphs are empty.
        raw.extend_from_slice(&[3, 1, 244, 2, 88]);
        raw.extend_from_slice(&[0, 0, 0, 0, 0, 0]);

        vec![
            1, 54, // head
            4, 6, // maxp
            2, 36, // hhea
            0xca, 0, // untransformed empty glyf
            0x43, 8, 5, // transformed hmtx
            0xcb, 6, // untransformed loca
        ]
    };

    minimal_woff2(6, &directory, &uncompressed_brotli(&raw))
}

#[test]
fn decodes_hmtx_with_transformed_and_untransformed_glyf() {
    use crate::{hmtx::HorizontalMetrics, sfnt::SfntFont};

    for (transformed, expected_lsb) in [(true, 10), (false, 0)] {
        let data = transformed_hmtx_fixture(transformed);
        let sfnt = decode_woff2(&data).unwrap();
        let font = SfntFont::parse(&sfnt).unwrap();

        let metrics = HorizontalMetrics::parse(&font, 2).unwrap();

        assert_eq!(metrics.metric(0).unwrap().advance_width, 500);
        assert_eq!(metrics.metric(0).unwrap().left_side_bearing, 0);

        assert_eq!(metrics.metric(1).unwrap().advance_width, 600);
        assert_eq!(metrics.metric(1).unwrap().left_side_bearing, expected_lsb);
    }
}

#[test]
fn reconstructs_real_noto_font_with_transformed_hmtx() {
    use crate::{sfnt::SfntFont, truetype::TrueTypeFont};

    let source_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/wpt/fonts/noto/",
        "NotoSansNko-regular-webfont.woff2",
    );

    let transformed_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/fonts/noto/nko-hmtx.woff2",
    );

    let source = std::fs::read(source_path).expect("original WPT Noto Sans NKo fixture must exist");

    let transformed = std::fs::read(transformed_path).expect("transformed hmtx fixture must exist");

    let directory = parse_woff2_directory(&transformed).unwrap();

    let record = directory
        .tables
        .iter()
        .find(|table| table.tag == TableTag::HMTX)
        .expect("fixture must contain hmtx");

    assert_eq!(record.transform, Woff2Transform::Hmtx);

    let decoded = decode_table_data(&transformed, &directory).unwrap();
    let tables = split_table_data(&decoded, &directory).unwrap();

    let encoded_hmtx = tables
        .iter()
        .find(|(tag, _)| *tag == TableTag::HMTX)
        .expect("decoded data must contain hmtx")
        .1;

    // Both bearing arrays are omitted in this fixture.
    assert_eq!(encoded_hmtx.first(), Some(&3));

    let source_sfnt = decode_woff2(&source).unwrap();
    let transformed_sfnt = decode_woff2(&transformed).unwrap();

    let reference = SfntFont::parse(&source_sfnt).unwrap();
    let actual = SfntFont::parse(&transformed_sfnt).unwrap();

    assert_eq!(reference.table(TableTag::HMTX).unwrap().len(), 756);
    assert_eq!(
        actual.table(TableTag::HMTX),
        reference.table(TableTag::HMTX)
    );

    let reference_font = TrueTypeFont::parse(&source_sfnt).unwrap();
    let actual_font = TrueTypeFont::parse(&transformed_sfnt).unwrap();

    assert_eq!(reference_font.maxp().num_glyphs, 190);
    assert_eq!(actual_font.maxp().num_glyphs, 190);

    for glyph_id in 0..190_u16 {
        assert_eq!(
            actual_font.metric(glyph_id),
            reference_font.metric(glyph_id),
            "metrics differ for glyph {glyph_id}"
        );

        let actual_path = actual_font
            .glyph_path(glyph_id)
            .unwrap_or_else(|error| panic!("cannot read glyph {glyph_id}: {error:?}"));

        let reference_path = reference_font
            .glyph_path(glyph_id)
            .unwrap_or_else(|error| panic!("cannot read reference glyph {glyph_id}: {error:?}"));

        assert_eq!(
            actual_path, reference_path,
            "outline differs for glyph {glyph_id}"
        );
    }

    // NKO LETTER A; independently checked against the original font.
    let metric = actual_font.metric(20).unwrap();

    assert_eq!(metric.advance_width, 543);
    assert_eq!(metric.left_side_bearing, 180);
}

fn append_bits(data: &mut Vec<u8>, bit_position: &mut usize, value: u32, bit_count: usize) {
    for bit_index in 0..bit_count {
        if *bit_position / 8 == data.len() {
            data.push(0);
        }

        let bit = (value >> bit_index) & 1;
        data[*bit_position / 8] |= (bit as u8) << (*bit_position % 8);
        *bit_position += 1;
    }
}
