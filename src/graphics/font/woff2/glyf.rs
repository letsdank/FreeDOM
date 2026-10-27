//! Parsing the WOFF2 transformed glyf table.
//!
//! Specification: <https://www.w3.org/TR/WOFF2/#glyf_table_format>

mod composite;
mod serialize;
mod triplet;

use super::super::{
    glyf::GlyphBounds,
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

/// A point encoded as a delta from the preceding point.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct PointDelta {
    pub dx: i32,
    pub dy: i32,
    pub on_curve: bool,
}

/// Component records belonging to one composite glyph.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct CompositeComponents<'a> {
    pub bytes: &'a [u8],
    pub has_instructions: bool,
    pub last_flags_offset: usize,
}

/// Current positions in the streams used to decode a simple outline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct OutlineCursors {
    pub points: usize,
    pub flags: usize,
    pub glyphs: usize,
}

/// Positions in the streams needed to reconstruct glyph records.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct GlyphCursors {
    pub outline: OutlineCursors,
    pub composites: usize,
    pub bbox: usize,
    pub instructions: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct OutlinePoint {
    pub x: i32,
    pub y: i32,
    pub on_curve: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct SimpleOutline {
    /// The index of the last point in each contour.
    pub contour_ends: Vec<u16>,
    pub points: Vec<OutlinePoint>,
}

impl SimpleOutline {
    /// Reconstructs a TrueType simple-glyph record.
    pub(super) fn to_truetype(
        &self,
        bounds: GlyphBounds,
        instructions: &[u8],
        overlap: bool,
    ) -> Result<Vec<u8>, FontError> {
        serialize::simple_glyph(self, bounds, instructions, overlap)
    }
}

impl GlyfStreams<'_> {
    /// Reads the signed contour count for a glyph.
    pub(super) fn contour_count(&self, glyph_index: usize) -> Result<i16, FontError> {
        if glyph_index >= usize::from(self.num_glyphs) {
            return Err(error(glyph_index, "glyph index is out of bounds"));
        }

        let reader = FontReader::new(self.contours, 0);

        reader.i16(glyph_index * 2)
    }

    /// Reads one contour's point count and advances the points-stream cursor.
    ///
    /// The cursor remains unchanged if the value is truncated.
    pub(super) fn read_point_count(&self, cursor: &mut usize) -> Result<u16, FontError> {
        let reader = FontReader::new(self.points, 0);
        let (count, length) = reader.u255_u16(*cursor)?;

        *cursor += length;

        Ok(count)
    }

    /// Reads one point from the flags and glyph streams.
    ///
    /// Both cursors remain unchanged if the point data is truncated.
    pub(super) fn read_point(
        &self,
        flag_cursor: &mut usize,
        glyph_cursor: &mut usize,
    ) -> Result<PointDelta, FontError> {
        let flag = FontReader::new(self.flags, 0).u8(*flag_cursor)?;

        let (point, length) = triplet::decode(flag, self.glyphs, *glyph_cursor)?;

        *flag_cursor += 1;
        *glyph_cursor += length;

        Ok(point)
    }

    /// Reads the points of a simple glyph.
    ///
    /// Coordinates remain relative to the preceding point across contour
    /// boundaries. Cursors are updated only after the entire outline is read.
    pub(super) fn read_simple_outline(
        &self,
        contour_count: usize,
        cursors: &mut OutlineCursors,
    ) -> Result<SimpleOutline, FontError> {
        if contour_count == 0 || contour_count > i16::MAX as usize {
            return Err(error(cursors.points, "invalid simple glyph contour count"));
        }

        let mut next = *cursors;
        let mut contour_ends = Vec::with_capacity(contour_count);
        let mut total_points = 0_usize;

        for _ in 0..contour_count {
            let count = usize::from(self.read_point_count(&mut next.points)?);

            if count == 0 {
                return Err(error(next.points, "glyph contour has no points"));
            }

            total_points = total_points
                .checked_add(count)
                .ok_or_else(|| error(next.points, "glyph point count overflows"))?;

            // TrueType contour endpoints are UInt16 point indexes.
            if total_points > usize::from(u16::MAX) + 1 {
                return Err(error(next.points, "glyph has too many points"));
            }

            contour_ends.push((total_points - 1) as u16);
        }

        // Each point needs one flag byte and at least one coordinate byte.
        FontReader::new(self.flags, 0).bytes(next.flags, total_points)?;
        FontReader::new(self.glyphs, 0).bytes(next.glyphs, total_points)?;

        let mut points = Vec::with_capacity(total_points);
        let mut x = 0_i32;
        let mut y = 0_i32;

        for _ in 0..total_points {
            let delta = self.read_point(&mut next.flags, &mut next.glyphs)?;

            x = x
                .checked_add(delta.dx)
                .ok_or_else(|| error(next.glyphs, "glyph x coordinate overflows"))?;

            y = y
                .checked_add(delta.dy)
                .ok_or_else(|| error(next.glyphs, "glyph y coordinate overflows"))?;

            points.push(OutlinePoint {
                x,
                y,
                on_curve: delta.on_curve,
            });
        }

        *cursors = next;

        Ok(SimpleOutline {
            contour_ends,
            points,
        })
    }

    /// Reconstructs one simple glyph as a TrueType glyph record.
    ///
    /// Stream positions are updated only after reconstruction succeeds.
    pub(super) fn reconstruct_simple_glyph(
        &self,
        glyph_index: usize,
        cursors: &mut GlyphCursors,
    ) -> Result<Vec<u8>, FontError> {
        let contour_count = self.contour_count(glyph_index)?;

        if contour_count <= 0 {
            return Err(error(glyph_index, "glyph is not a simple glyph"));
        }

        let mut next = *cursors;

        let outline = self.read_simple_outline(contour_count as usize, &mut next.outline)?;

        // The instruction legnth follows the point data in glyphStream.
        let glyph_reader = FontReader::new(self.glyphs, 0);

        let (instruction_length, used) = glyph_reader.u255_u16(next.outline.glyphs)?;
        next.outline.glyphs += used;

        let instructions = FontReader::new(self.instructions, 0)
            .bytes(next.instructions, usize::from(instruction_length))?;
        next.instructions += usize::from(instruction_length);

        let bbox_explicit = bitmap_bit(self.bbox_bitmap, glyph_index)?;

        let bounds = if bbox_explicit {
            let bytes = FontReader::new(self.bbox_data, 0).bytes(next.bbox, 8)?;

            let reader = FontReader::new(bytes, next.bbox);

            let bounds = GlyphBounds {
                x_min: reader.i16(0)?,
                y_min: reader.i16(2)?,
                x_max: reader.i16(4)?,
                y_max: reader.i16(6)?,
            };

            next.bbox += 8;

            bounds
        } else {
            outline_bounds(&outline)?
        };

        let overlap = match self.overlap_bitmap {
            Some(bitmap) => bitmap_bit(bitmap, glyph_index)?,
            None => false,
        };

        let glyph = outline.to_truetype(bounds, instructions, overlap)?;

        *cursors = next;

        Ok(glyph)
    }

    /// Reconstructs an empty, simple, or composite glyph.
    pub(super) fn reconstruct_glyph(
        &self,
        glyph_index: usize,
        cursors: &mut GlyphCursors,
    ) -> Result<Vec<u8>, FontError> {
        let contours = self.contour_count(glyph_index)?;

        match contours {
            0 => {
                if bitmap_bit(self.bbox_bitmap, glyph_index)? {
                    return Err(error(
                        glyph_index,
                        "empty glyph has an explicit bounding box",
                    ));
                }

                // An empty glyph has no record in the reconstructed glyf table.
                Ok(Vec::new())
            }

            1.. => self.reconstruct_simple_glyph(glyph_index, cursors),

            _ => self.reconstruct_composite_glyph(glyph_index, cursors),
        }
    }

    /// Reads a composite glyph's component records.
    ///
    /// The cursor remains unchanged if any component is invalid.
    pub(super) fn read_components(
        &self,
        cursor: &mut usize,
    ) -> Result<CompositeComponents<'_>, FontError> {
        let (components, length) = composite::read(self.composites, *cursor, self.num_glyphs)?;

        *cursor += length;

        Ok(components)
    }

    /// Reconstructs one composite glyph as a TrueType glyph record.
    ///
    /// Stream positions are updated only after reconstruction succeeds.
    pub(super) fn reconstruct_composite_glyph(
        &self,
        glyph_index: usize,
        cursors: &mut GlyphCursors,
    ) -> Result<Vec<u8>, FontError> {
        let contours = self.contour_count(glyph_index)?;

        if contours >= 0 {
            return Err(error(glyph_index, "glyph is not a composite glyph"));
        }

        if !bitmap_bit(self.bbox_bitmap, glyph_index)? {
            return Err(error(
                glyph_index,
                "composite glyph is missing a bounding box",
            ));
        }

        let mut next = *cursors;

        let components = self.read_components(&mut next.composites)?;

        let bounds = FontReader::new(self.bbox_data, 0).bytes(next.bbox, 8)?;

        next.bbox += 8;

        let instructions = if components.has_instructions {
            let (length, used) = FontReader::new(self.glyphs, 0).u255_u16(next.outline.glyphs)?;

            next.outline.glyphs += used;

            let bytes = FontReader::new(self.instructions, 0)
                .bytes(next.instructions, usize::from(length))?;

            next.instructions += usize::from(length);

            Some(bytes)
        } else {
            None
        };

        let mut result = Vec::new();

        result.extend_from_slice(&contours.to_be_bytes());
        result.extend_from_slice(bounds);
        result.extend_from_slice(components.bytes);

        if let Some(bytes) = instructions {
            let length = u16::try_from(bytes.len())
                .map_err(|_| error(next.instructions, "glyph instructions are too long"))?;

            result.extend_from_slice(&length.to_be_bytes());
            result.extend_from_slice(bytes);
        }

        *cursors = next;

        Ok(result)
    }

    /// Reconstructs the TrueType glyf and loca tables.
    ///
    /// The loca length comes from the original-length field in the WOFF2
    /// table directory, not from the reconstructured glyf table size.
    pub(super) fn reconstruct_tables(
        &self,
        original_loca_length: usize,
    ) -> Result<(Vec<u8>, Vec<u8>), FontError> {
        let entry_size = match self.index_format {
            0 => 2,
            1 => 4,
            _ => {
                return Err(error(0, "invalid transformed glyf index format"));
            }
        };

        let entry_count = usize::from(self.num_glyphs) + 1;
        let expected_loca_length = entry_count * entry_size;

        if original_loca_length != expected_loca_length {
            return Err(error(
                0,
                "WOFF2 loca original length does not match glyph count",
            ));
        }

        let mut glyf = Vec::new();
        let mut loca = Vec::with_capacity(expected_loca_length);
        let mut cursors = GlyphCursors::default();

        for glyph_index in 0..usize::from(self.num_glyphs) {
            write_loca_offset(&mut loca, self.index_format, glyf.len())?;
            let glyph = self.reconstruct_glyph(glyph_index, &mut cursors)?;

            let new_length = glyf
                .len()
                .checked_add(glyph.len())
                .ok_or_else(|| error(glyph_index, "reconstructed glyf table is too large"))?;

            // Short loca stores offsets divided by two.
            let padding = usize::from(self.index_format == 0 && new_length % 2 != 0);

            let final_length = new_length
                .checked_add(padding)
                .ok_or_else(|| error(glyph_index, "reconstructed glyf table is too large"))?;

            u32::try_from(final_length)
                .map_err(|_| error(glyph_index, "reconstructed glyf table is too large"))?;

            let additional = glyph
                .len()
                .checked_add(padding)
                .ok_or_else(|| error(glyph_index, "reconstructed glyf table is too large"))?;

            glyf.try_reserve(additional)
                .map_err(|_| error(glyph_index, "cannot allocate reconstructed glyf table"))?;

            glyf.extend_from_slice(&glyph);

            if padding != 0 {
                glyf.push(0);
            }
        }

        // loca contains one extra offset marking the end of the last glyph.
        write_loca_offset(&mut loca, self.index_format, glyf.len())?;

        Ok((glyf, loca))
    }
}

/// Appends one offset using the format specified by indexToLocFormat.
fn write_loca_offset(
    loca: &mut Vec<u8>,
    index_format: u16,
    offset: usize,
) -> Result<(), FontError> {
    match index_format {
        0 => {
            if offset % 2 != 0 {
                return Err(error(offset, "short loca offset is not aligned"));
            }

            let value = u16::try_from(offset / 2)
                .map_err(|_| error(offset, "short loca offset is too large"))?;

            loca.extend_from_slice(&value.to_be_bytes());
        }

        1 => {
            let value = u32::try_from(offset)
                .map_err(|_| error(offset, "long loca offset is too large"))?;

            loca.extend_from_slice(&value.to_be_bytes());
        }

        _ => {
            return Err(error(offset, "invalid transformed glyf index format"));
        }
    }

    Ok(())
}

/// Reads a most-significant-bit-first glyph bitmap.
fn bitmap_bit(bitmap: &[u8], glyph_index: usize) -> Result<bool, FontError> {
    let byte = FontReader::new(bitmap, 0).u8(glyph_index / 8)?;
    let mask = 0x80 >> (glyph_index % 8);

    Ok(byte & mask != 0)
}

/// Computes a bounding box from all on-curve and off-curve points.
fn outline_bounds(outline: &SimpleOutline) -> Result<GlyphBounds, FontError> {
    let first = outline
        .points
        .first()
        .ok_or_else(|| error(0, "simple glyph has no points"))?;

    let mut x_min = first.x;
    let mut y_min = first.y;
    let mut x_max = first.x;
    let mut y_max = first.y;

    for point in &outline.points[1..] {
        x_min = x_min.min(point.x);
        y_min = y_min.min(point.y);
        x_max = x_max.max(point.x);
        y_max = y_max.max(point.y);
    }

    let convert = |value| {
        i16::try_from(value).map_err(|_| error(0, "glyph bounding box coordinate out of range"))
    };

    Ok(GlyphBounds {
        x_min: convert(x_min)?,
        y_min: convert(y_min)?,
        x_max: convert(x_max)?,
        y_max: convert(y_max)?,
    })
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
    use crate::graphics::font::glyf::Glyph;

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

    #[test]
    fn reads_glyph_contours_and_point_counts() {
        let mut data = sample(false);

        // The second glyph is composite: its contour count is -1.
        data[38] = 0xff;
        data[39] = 0xff;

        let streams = parse(&data).unwrap();

        assert_eq!(streams.contour_count(0).unwrap(), 1);
        assert_eq!(streams.contour_count(1).unwrap(), -1);

        assert_eq!(
            streams.contour_count(2).unwrap_err().message,
            "glyph index is out of bounds"
        );

        let mut cursor = 0;

        assert_eq!(streams.read_point_count(&mut cursor).unwrap(), 42);
        assert_eq!(cursor, 1);

        assert!(streams.read_point_count(&mut cursor).is_err());
        assert_eq!(cursor, 1);
    }

    #[test]
    fn reads_point_from_streams_without_advancing_on_error() {
        let data = sample(false);
        let streams = parse(&data).unwrap();

        let mut flag_cursor = 0;
        let mut glyph_cursor = 0;

        let point = streams
            .read_point(&mut flag_cursor, &mut glyph_cursor)
            .unwrap();

        assert_eq!(
            point,
            PointDelta {
                dx: 0,
                dy: 3,
                on_curve: true,
            }
        );

        assert_eq!((flag_cursor, glyph_cursor), (1, 1));

        // The second flag exists, but its coordinate byte is missing.
        assert!(
            streams
                .read_point(&mut flag_cursor, &mut glyph_cursor)
                .is_err()
        );

        assert_eq!((flag_cursor, glyph_cursor), (1, 1));
    }

    #[test]
    fn builds_simple_outline_across_contours() {
        let streams = outline_streams(
            &[99, 2, 2],
            &[99, 23, 0x80 | 21, 1, 10],
            &[99, 0x12, 0x23, 4, 5],
        );

        let mut cursors = OutlineCursors {
            points: 1,
            flags: 1,
            glyphs: 1,
        };

        let outline = streams.read_simple_outline(2, &mut cursors).unwrap();

        assert_eq!(outline.contour_ends, [1, 3]);

        assert_eq!(
            outline.points,
            vec![
                OutlinePoint {
                    x: 2,
                    y: 3,
                    on_curve: true,
                },
                OutlinePoint {
                    x: 5,
                    y: -1,
                    on_curve: false,
                },
                OutlinePoint {
                    x: 5,
                    y: 3,
                    on_curve: true,
                },
                OutlinePoint {
                    x: 0,
                    y: 3,
                    on_curve: true,
                }
            ]
        );

        assert_eq!(
            cursors,
            OutlineCursors {
                points: 3,
                flags: 5,
                glyphs: 5,
            }
        );
    }

    #[test]
    fn errors_do_not_advance_outline_cursors() {
        let starting = OutlineCursors::default();

        let cases: &[(&[u8], &[u8], &[u8], usize)] = &[
            // The second point requires three bytes but only one remains.
            (&[2], &[23, 123], &[0x12, 0], 1),
            // A contour cannot have zero points.
            (&[0], &[], &[], 1),
            // 65535 + 2 points exceed the UInt16 endpoint range.
            (&[253, 255, 255, 2], &[], &[], 2),
            // This method handles simple glyphs with at least one contour.
            (&[], &[], &[], 0),
        ];

        for &(points, flags, glyphs, contours) in cases {
            let streams = outline_streams(points, flags, glyphs);
            let mut cursors = starting;

            assert!(streams.read_simple_outline(contours, &mut cursors).is_err());

            assert_eq!(cursors, starting);
        }
    }

    #[test]
    fn rejects_coordinate_accumulation_overflow() {
        let point_count = 32769_usize;

        // 255UInt16 encoding of 32769.
        let points = [253, 0x80, 0x01];

        // Each point advances x by 65535.
        let flags = vec![125_u8; point_count];
        let glyphs = [0xff, 0xff, 0, 0].repeat(point_count);

        let streams = outline_streams(&points, &flags, &glyphs);
        let mut cursors = OutlineCursors::default();

        let error = streams.read_simple_outline(1, &mut cursors).unwrap_err();

        assert_eq!(error.message, "glyph x coordinate overflows");
        assert_eq!(cursors, OutlineCursors::default());
    }

    #[test]
    fn reconstructs_bounds_instructions_and_overlap() {
        use crate::graphics::font::glyf::{Glyph, parse_glyph};

        let cases = [
            (
                false,
                false,
                GlyphBounds {
                    x_min: 0,
                    y_min: 0,
                    x_max: 50,
                    y_max: 80,
                },
            ),
            (
                true,
                true,
                GlyphBounds {
                    x_min: -10,
                    y_min: -20,
                    x_max: 60,
                    y_max: 100,
                },
            ),
        ];

        for (explicit_bbox, overlap, expected_bounds) in cases {
            let data = simple_fixture(explicit_bbox, overlap);
            let streams = parse(&data).unwrap();
            let mut cursors = GlyphCursors::default();

            let bytes = streams.reconstruct_simple_glyph(0, &mut cursors).unwrap();

            // Instruction length followed by instruction bytes.
            assert_eq!(&bytes[12..16], &[0, 2, 0xaa, 0xbb]);

            // OVERLAP_SIMPLE is stored in the first point flag.
            assert_eq!(bytes[16] & 0x40 != 0, overlap);

            let Glyph::Simple(glyph) = parse_glyph(&bytes, 0).unwrap() else {
                panic!("expected a simple glyph");
            };

            assert_eq!(glyph.bounds, expected_bounds);

            assert_eq!(
                cursors,
                GlyphCursors {
                    outline: OutlineCursors {
                        points: 1,
                        flags: 3,
                        glyphs: 5,
                    },
                    composites: 0,
                    bbox: if explicit_bbox { 8 } else { 0 },
                    instructions: 2,
                }
            );
        }
    }

    #[test]
    fn reconstruction_does_not_advance_cursors_on_error() {
        let data = simple_fixture(true, true);
        let mut cursors = GlyphCursors::default();

        let mut missing_instructions = parse(&data).unwrap();
        missing_instructions.instructions = &[];

        assert!(
            missing_instructions
                .reconstruct_simple_glyph(0, &mut cursors)
                .is_err(),
        );

        assert_eq!(cursors, GlyphCursors::default());

        let mut missing_bbox = parse(&data).unwrap();
        missing_bbox.bbox_data = &[];

        assert!(
            missing_bbox
                .reconstruct_simple_glyph(0, &mut cursors)
                .is_err()
        );

        assert_eq!(cursors, GlyphCursors::default());

        assert!(
            missing_bbox
                .reconstruct_simple_glyph(1, &mut cursors)
                .is_err()
        );

        assert_eq!(cursors, GlyphCursors::default());
    }

    #[test]
    fn reconstructs_empty_then_simple_glyph_with_shared_cursors() {
        use crate::graphics::font::glyf::parse_glyph;

        let mut data = simple_fixture(false, false);

        // Add an empty glyph before the existing simple glyph.
        write_u16(&mut data, 4, 2).unwrap();
        write_u32(&mut data, 8, 4).unwrap();
        data.splice(36..36, [0, 0]);

        let streams = parse(&data).unwrap();
        let mut cursors = GlyphCursors::default();

        let empty = streams.reconstruct_glyph(0, &mut cursors).unwrap();

        assert!(empty.is_empty());
        assert_eq!(cursors, GlyphCursors::default());
        assert!(matches!(parse_glyph(&empty, 0).unwrap(), Glyph::Empty));

        let simple = streams.reconstruct_glyph(1, &mut cursors).unwrap();

        assert!(matches!(parse_glyph(&simple, 0).unwrap(), Glyph::Simple(_)));
        assert_eq!(cursors.outline.glyphs, 5);
        assert_eq!(cursors.instructions, 2);
    }

    #[test]
    fn rejects_empty_bbox_and_incomplete_composite() {
        let mut data = simple_fixture(true, false);

        // The first glyph is empty, but its bbox bitmap bit is set.
        data[36..38].copy_from_slice(&0_i16.to_be_bytes());

        let streams = parse(&data).unwrap();
        let mut cursors = GlyphCursors::default();

        let error = streams.reconstruct_glyph(0, &mut cursors).unwrap_err();

        assert_eq!(error.message, "empty glyph has an explicit bounding box");
        assert_eq!(cursors, GlyphCursors::default());

        // A negative contour count identifies a composite glyph.
        data[36..38].copy_from_slice(&(-1_i16).to_be_bytes());

        let streams = parse(&data).unwrap();

        let error = streams.reconstruct_glyph(0, &mut cursors).unwrap_err();

        assert_eq!(error.message, "truncated font data");
        assert_eq!(cursors, GlyphCursors::default());
    }

    #[test]
    fn reads_composite_stream_transactionally() {
        let data = simple_fixture(false, false);
        let mut streams = parse(&data).unwrap();

        let components = [
            0xee, // unrelated prefix
            0, 2, // flags
            0, 0, // glyph ID
            0, 0, // arguments
        ];

        streams.composites = &components;

        let mut cursor = 1;
        let result = streams.read_components(&mut cursor).unwrap();

        assert_eq!(result.bytes, &components[1..]);
        assert!(!result.has_instructions);
        assert_eq!(cursor, 7);

        assert!(streams.read_components(&mut cursor).is_err());
        assert_eq!(cursor, 7);
    }

    #[test]
    fn reconstructs_composite_with_instructions_on_first_component() {
        use crate::graphics::font::glyf::parse_glyph;

        let streams = composite_fixture();
        let mut cursors = GlyphCursors::default();

        let glyph = streams.reconstruct_glyph(0, &mut cursors).unwrap();

        assert_eq!(&glyph[10..22], streams.composites);
        assert_eq!(&glyph[22..], &[0, 2, 0xaa, 0xbb]);

        let Glyph::Compound(parsed) = parse_glyph(&glyph, 0).unwrap() else {
            panic!("expected a composite glyph");
        };

        assert_eq!(parsed.components.len(), 2);

        assert_eq!(
            parsed.bounds,
            GlyphBounds {
                x_min: -10,
                y_min: -20,
                x_max: 60,
                y_max: 100,
            }
        );

        assert!(parse_glyph(&glyph[..glyph.len() - 1], 0).is_err());

        assert_eq!(
            cursors,
            GlyphCursors {
                outline: OutlineCursors {
                    points: 0,
                    flags: 0,
                    glyphs: 1,
                },
                composites: 12,
                bbox: 8,
                instructions: 2,
            }
        );
    }

    #[test]
    fn composite_reconstruction_failures_leave_cursors_unchanged() {
        let mut streams = composite_fixture();
        let mut cursors = GlyphCursors::default();

        streams.bbox_bitmap = &[0, 0, 0, 0];

        let error = streams.reconstruct_glyph(0, &mut cursors).unwrap_err();

        assert_eq!(error.message, "composite glyph is missing a bounding box");
        assert_eq!(cursors, GlyphCursors::default());

        streams.bbox_bitmap = &[0x80, 0, 0, 0];
        streams.instructions = &[0xaa];

        assert!(streams.reconstruct_glyph(0, &mut cursors).is_err());
        assert_eq!(cursors, GlyphCursors::default());

        streams.instructions = &[0xaa, 0xbb];
        streams.composites = &[0, 2, 0];

        assert!(streams.reconstruct_glyph(0, &mut cursors).is_err());
        assert_eq!(cursors, GlyphCursors::default());
    }

    #[test]
    fn composite_without_instructions_leaves_streams_untouched() {
        let mut streams = composite_fixture();

        streams.composites = &[0, 2, 0, 1, 0, 0];

        let mut cursors = GlyphCursors::default();
        let glyph = streams.reconstruct_glyph(0, &mut cursors).unwrap();

        assert_eq!(glyph.len(), 16);
        assert_eq!(cursors.outline.glyphs, 0);
        assert_eq!(cursors.instructions, 0);
        assert_eq!(cursors.composites, 6);
    }

    fn outline_streams<'a>(points: &'a [u8], flags: &'a [u8], glyphs: &'a [u8]) -> GlyfStreams<'a> {
        GlyfStreams {
            num_glyphs: 0,
            index_format: 0,
            contours: &[],
            points,
            flags,
            glyphs,
            composites: &[],
            bbox_bitmap: &[],
            bbox_data: &[],
            instructions: &[],
            overlap_bitmap: None,
        }
    }

    fn simple_fixture(explicit_bbox: bool, overlap: bool) -> Vec<u8> {
        let mut data = vec![0_u8; 36];

        write_u16(&mut data, 2, u16::from(overlap)).unwrap();
        write_u16(&mut data, 4, 1).unwrap();

        let bbox_size = if explicit_bbox { 12 } else { 4 };
        let sizes = [2_u32, 1, 3, 5, 0, bbox_size, 2];

        for (index, size) in sizes.into_iter().enumerate() {
            write_u32(&mut data, 8 + index * 4, size).unwrap();
        }

        data.extend_from_slice(&[
            0, 1, // one contour
            3, // three points
            1, 11, 86, // point flags
            0, 50, 24, 79, // coordinates
            2,  // instruction length
        ]);

        data.extend_from_slice(&[if explicit_bbox { 0x80 } else { 0 }, 0, 0, 0]);

        if explicit_bbox {
            // xMin = -10, yMin = -20, xMax = 60, yMax = 100.
            data.extend_from_slice(&[0xff, 0xf6, 0xff, 0xec, 0x00, 60, 0x00, 100]);
        }

        data.extend_from_slice(&[0xaa, 0xbb]);

        if overlap {
            data.push(0x80);
        }

        data
    }

    fn composite_fixture() -> GlyfStreams<'static> {
        GlyfStreams {
            num_glyphs: 3,
            index_format: 0,

            // Glyph 0 is composite; glyphs 1 and 2 are empty.
            contours: &[0xff, 0xff, 0, 0, 0, 0],

            points: &[],
            flags: &[],

            // Composite instruction length.
            glyphs: &[2],

            composites: &[
                // First component: instructions + another component.
                0x01, 0x22, 0, 1, 10, 20, // Last component: no instructions flag.
                0, 2, 0, 2, 0xfe, 4,
            ],

            bbox_bitmap: &[0x80, 0, 0, 0],

            // xMin = -10, yMin = -20, xMax = 60, yMax = 100.
            bbox_data: &[0xff, 0xf6, 0xff, 0xec, 0, 60, 0, 100],

            instructions: &[0xaa, 0xbb],
            overlap_bitmap: None,
        }
    }

    fn table_fixture(format: u16) -> GlyfStreams<'static> {
        GlyfStreams {
            num_glyphs: 3,
            index_format: format,

            // Empty, simple, composite.
            contours: &[0, 0, 0, 1, 0xff, 0xff],

            points: &[3],
            flags: &[1, 11, 86],

            // Simple point data, simple instruction length,
            // composite instruction length.
            glyphs: &[0, 50, 24, 79, 0, 2],

            // Composite references glyph 1 and has instructions.
            composites: &[1, 2, 0, 1, 0, 0],

            // Explicit bbox for glyph 2.
            bbox_bitmap: &[0x20, 0, 0, 0],
            bbox_data: &[0, 0, 0, 0, 0, 50, 0, 80],

            instructions: &[0xaa, 0xbb],
            overlap_bitmap: None,
        }
    }

    #[test]
    fn reconstructs_short_and_long_glyf_loca_tables() {
        use crate::graphics::font::glyf::parse_glyph;

        let cases = [(0, 8, vec![0, 0, 30, 50]), (1, 16, vec![0, 0, 29, 49])];

        for (format, loca_size, expected_offsets) in cases {
            let (glyf, loca) = table_fixture(format).reconstruct_tables(loca_size).unwrap();

            let offsets: Vec<usize> = if format == 0 {
                loca.chunks_exact(2)
                    .map(|bytes| usize::from(u16::from_be_bytes([bytes[0], bytes[1]])) * 2)
                    .collect()
            } else {
                loca.chunks_exact(4)
                    .map(|bytes| {
                        u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize
                    })
                    .collect()
            };

            assert_eq!(offsets, expected_offsets);
            assert_eq!(loca.len(), loca_size);
            assert_eq!(glyf.len(), *offsets.last().unwrap());

            assert!(matches!(
                parse_glyph(&glyf[offsets[0]..offsets[1]], 0).unwrap(),
                Glyph::Empty
            ));

            assert!(matches!(
                parse_glyph(&glyf[offsets[1]..offsets[2]], 0).unwrap(),
                Glyph::Simple(_)
            ));

            assert!(matches!(
                parse_glyph(&glyf[offsets[2]..offsets[3]], 0).unwrap(),
                Glyph::Compound(_)
            ));
        }
    }

    #[test]
    fn validates_loca_format_length_and_offset_limits() {
        let error = table_fixture(0).reconstruct_tables(16).unwrap_err();

        assert_eq!(
            error.message,
            "WOFF2 loca original length does not match glyph count"
        );

        let mut loca = Vec::new();

        write_loca_offset(&mut loca, 0, 131070).unwrap();

        assert_eq!(loca, [0xff, 0xff]);

        assert!(write_loca_offset(&mut loca, 0, 131072).is_err());
        assert!(write_loca_offset(&mut loca, 0, 1).is_err());

        // Failed writes must not modify the output.
        assert_eq!(loca, [0xff, 0xff]);
    }
}
