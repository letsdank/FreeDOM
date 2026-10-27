//! Parsing the WOFF2 transformed glyf table.
//!
//! Specification: <https://www.w3.org/TR/WOFF2/#glyf_table_format>

use super::super::{
    reader::{FontReader, to_usize},
    sfnt::FontError,
};

/// The streams that make up a transformed WOFF2 glyf table.
///
/// These are borrowed slices of the decompressed table data. Parsing this
/// structure does not reconstruct TrueType glyphs.
#[derive(Debug)]
pub(super) struct GlyfStreams<'a> {
    pub num_glyphs: u16,
    pub index_format: u16,
    pub contours: &'a [u8],
    pub points: &'a [u8],
    pub flags: &'a [u8],
    pub glyphs: &'a [u8],
    pub composites: &'a [u8],
    pub bbox_bitmap: &'a [u8],
    pub bbox_data: &'a [u8],
    pub instructions: &'a [u8],
    pub overlap_bitmap: Option<&'a [u8]>,
}

pub(super) fn parse(data: &[u8]) -> Result<GlyfStreams<'_>, FontError> {
    let reader = FontReader::new(data, 0);

    let version = reader.u16(0)?;
    let option_flags = reader.u16(2)?;
    let num_glyphs = reader.u16(4)?;
    let index_format = reader.u16(6)?;

    if version != 0 {
        return Err(error(0, "invalid transformed glyf version"));
    }

    if index_format > 1 {
        return Err(error(6, "invalid transformed glyf index format"));
    }

    // The header stores seven consecutive UInt32 stream sizes.
    let mut sizes = [0_usize; 7];

    for (index, size) in sizes.iter_mut().enumerate() {
        let offset = 8 + index * 4;

        *size = to_usize(reader.u32(offset)?, offset)?;
    }

    // The contour stream contains one Int16 value per glyph.
    if sizes[0] != usize::from(num_glyphs) * 2 {
        return Err(error(8, "invalid transformed glyf contour stream size"));
    }

    // The bbox bitmap is padded to a multiple of four bytes.
    let bbox_bitmap_size = (usize::from(num_glyphs) + 31) / 32 * 4;

    if sizes[5] < bbox_bitmap_size {
        return Err(error(28, "transformed glyf bbox stream is too short"));
    }

    let mut cursor = 36_usize;

    let contours = take(&reader, &mut cursor, sizes[0])?;
    let points = take(&reader, &mut cursor, sizes[1])?;
    let flags = take(&reader, &mut cursor, sizes[2])?;
    let glyphs = take(&reader, &mut cursor, sizes[3])?;
    let composites = take(&reader, &mut cursor, sizes[4])?;

    let bbox = take(&reader, &mut cursor, sizes[5])?;
    let (bbox_bitmap, bbox_data) = bbox.split_at(bbox_bitmap_size);

    let instructions = take(&reader, &mut cursor, sizes[6])?;

    let overlap_bitmap = if option_flags & 1 != 0 {
        let size = (usize::from(num_glyphs) + 7) / 8;

        Some(take(&reader, &mut cursor, size)?)
    } else {
        None
    };

    if cursor != data.len() {
        return Err(error(cursor, "transformed glyf table has trailing bytes"));
    }

    Ok(GlyfStreams {
        num_glyphs,
        index_format,
        contours,
        points,
        flags,
        glyphs,
        composites,
        bbox_bitmap,
        bbox_data,
        instructions,
        overlap_bitmap,
    })
}

fn take<'a>(
    reader: &FontReader<'a>,
    cursor: &mut usize,
    length: usize,
) -> Result<&'a [u8], FontError> {
    let bytes = reader.bytes(*cursor, length)?;

    // FontReader::bytes has already checked this addition.
    *cursor += length;

    Ok(bytes)
}

fn error(offset: usize, message: &str) -> FontError {
    FontError {
        offset,
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::reader::{write_u16, write_u32};
    use super::*;

    fn sample(overlap: bool) -> Vec<u8> {
        let mut data = vec![0_u8; 36];

        write_u16(&mut data, 2, u16::from(overlap)).unwrap();
        write_u16(&mut data, 4, 2).unwrap();
        write_u16(&mut data, 6, 1).unwrap();

        let sizes = [4_u32, 1, 2, 1, 1, 6, 1];

        for (index, size) in sizes.into_iter().enumerate() {
            write_u32(&mut data, 8 + index * 4, size).unwrap();
        }

        data.extend_from_slice(&[
            0, 1, 0, 0,  // contours
            42, // points
            1, 2, // flags
            3, // glyphs
            4, // composites
            0x80, 0, 0, 0, // bbox bitmap
            5, 6, // bbox data
            7, // instructions
        ]);

        if overlap {
            data.push(0x80);
        }

        data
    }

    #[test]
    fn splits_transformed_glyf_streams() {
        let data = sample(true);
        let streams = parse(&data).unwrap();

        assert_eq!(streams.num_glyphs, 2);
        assert_eq!(streams.index_format, 1);
        assert_eq!(streams.contours, &[0, 1, 0, 0]);
        assert_eq!(streams.points, &[42]);
        assert_eq!(streams.flags, &[1, 2]);
        assert_eq!(streams.glyphs, &[3]);
        assert_eq!(streams.composites, &[4]);
        assert_eq!(streams.bbox_bitmap, &[0x80, 0, 0, 0]);
        assert_eq!(streams.bbox_data, &[5, 6]);
        assert_eq!(streams.instructions, &[7]);
        assert_eq!(streams.overlap_bitmap, Some(&[0x80][..]));

        assert!(parse(&sample(false)).unwrap().overlap_bitmap.is_none());
    }

    #[test]
    fn rejects_bad_lengths_and_trailing_data() {
        let data = sample(true);

        // Every truncated prefix must fail without panicking.
        for end in 0..data.len() {
            assert!(parse(&data[..end]).is_err(), "truncated at {end}");
        }

        let mut extra = data.clone();
        extra.push(0);

        assert_eq!(
            parse(&extra).unwrap_err().message,
            "transformed glyf table has trailing bytes"
        );

        let mut bad_contours = data.clone();
        write_u32(&mut bad_contours, 8, 3).unwrap();

        assert_eq!(
            parse(&bad_contours).unwrap_err().message,
            "invalid transformed glyf contour stream size"
        );

        let mut bad_bbox = data.clone();
        write_u32(&mut bad_bbox, 28, 3).unwrap();

        assert_eq!(
            parse(&bad_bbox).unwrap_err().message,
            "transformed glyf bbox stream is too short"
        );

        let mut bad_index_format = data;
        write_u16(&mut bad_index_format, 6, 2).unwrap();

        assert_eq!(
            parse(&bad_index_format).unwrap_err().message,
            "invalid transformed glyf index format",
        );
    }
}
