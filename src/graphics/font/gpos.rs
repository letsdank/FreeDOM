use super::{
    kern::KerningTable,
    reader::{read_bytes, read_i16, read_u16},
    sfnt::{FontError, SfntFont, TableTag},
};
use std::collections::{BTreeMap, BTreeSet};

const X_ADVANCE: u16 = 0x0004;
const SUPPORTED_VALUE_FORMAT: u16 = 0x00ff;

pub fn apply_kerning(font: &SfntFont<'_>, kerning: &mut KerningTable) -> Result<(), FontError> {
    let Some(record) = font.table_record(TableTag::GPOS) else {
        return Ok(());
    };

    let table = font.table(TableTag::GPOS).ok_or_else(|| FontError {
        offset: record.offset,
        message: "cannot read GPOS table".to_owned(),
    })?;

    if table.len() < 10 {
        return Err(error(
            record.offset,
            0,
            "GPOS table is shorter than 10 bytes",
        ));
    }

    let major_version = read_u16(table, 0, record.offset)?;
    let minor_version = read_u16(table, 2, record.offset)?;

    if major_version != 1 || minor_version > 1 {
        return Err(error(record.offset, 0, "unsupported GPOS table version"));
    }

    if minor_version == 1 && table.len() < 14 {
        return Err(error(
            record.offset,
            0,
            "GPOS version 1.1 table is shorter than 14 bytes",
        ));
    }

    let feature_list_offset = child_offset(
        table,
        record.offset,
        0,
        usize::from(read_u16(table, 6, record.offset)?),
        "feature list",
    )?;

    let lookup_list_offset = child_offset(
        table,
        record.offset,
        0,
        usize::from(read_u16(table, 8, record.offset)?),
        "lookup list",
    )?;

    let kerning_lookups = kerning_lookup_indices(table, record.offset, feature_list_offset)?;
    let lookup_offsets = lookup_offsets(table, record.offset, lookup_list_offset)?;

    for lookup_index in kerning_lookups {
        let lookup_offset = *lookup_offsets
            .get(usize::from(lookup_index))
            .ok_or_else(|| {
                error(
                    record.offset,
                    lookup_list_offset,
                    "GPOS kern feature refers to a missing lookup",
                )
            })?;

        apply_lookup(table, record.offset, lookup_offset, kerning)?;
    }

    Ok(())
}

fn kerning_lookup_indices(
    table: &[u8],
    table_offset: usize,
    feature_list_offset: usize,
) -> Result<Vec<u16>, FontError> {
    let feature_count = usize::from(read_u16(table, feature_list_offset, table_offset)?);
    let records_offset = checked_add(
        table_offset,
        feature_list_offset,
        2,
        "GPOS feature list offset overflows",
    )?;

    let records_length = feature_count
        .checked_mul(6)
        .ok_or_else(|| error(table_offset, records_offset, "too many GPOS features"))?;

    read_bytes(table, records_offset, records_length, table_offset)?;

    let mut lookup_indices = Vec::new();

    for index in 0..feature_count {
        let record_offset = records_offset + index * 6;
        let tag = read_bytes(table, record_offset, 4, table_offset)?;
        let feature_offset = usize::from(read_u16(table, record_offset + 4, table_offset)?);

        if tag != b"kern" {
            continue;
        }

        let feature_table_offset = child_offset(
            table,
            table_offset,
            feature_list_offset,
            feature_offset,
            "feature table",
        )?;

        let lookup_count = usize::from(read_u16(
            table,
            checked_add(
                table_offset,
                feature_table_offset,
                2,
                "GPOS feature table offset overflows",
            )?,
            table_offset,
        )?);

        let indices_offset = checked_add(
            table_offset,
            feature_table_offset,
            4,
            "GPOS lookup index offset overflows",
        )?;

        let indices_length = lookup_count
            .checked_mul(2)
            .ok_or_else(|| error(table_offset, indices_offset, "too many GPOS lookup indices"))?;

        read_bytes(table, indices_offset, indices_length, table_offset)?;

        for lookup_index in 0..lookup_count {
            lookup_indices.push(read_u16(
                table,
                indices_offset + lookup_index * 2,
                table_offset,
            )?);
        }
    }

    Ok(lookup_indices)
}

fn lookup_offsets(
    table: &[u8],
    table_offset: usize,
    lookup_list_offset: usize,
) -> Result<Vec<usize>, FontError> {
    let lookup_count = usize::from(read_u16(table, lookup_list_offset, table_offset)?);
    let offsets_start = checked_add(
        table_offset,
        lookup_list_offset,
        2,
        "GPOS lookup list offset overflows",
    )?;

    let offsets_length = lookup_count
        .checked_mul(2)
        .ok_or_else(|| error(table_offset, offsets_start, "too many GPOS lookups"))?;

    read_bytes(table, offsets_start, offsets_length, table_offset)?;

    let mut offsets = Vec::with_capacity(lookup_count);

    for index in 0..lookup_count {
        let relative_offset =
            usize::from(read_u16(table, offsets_start + index * 2, table_offset)?);

        offsets.push(child_offset(
            table,
            table_offset,
            lookup_list_offset,
            relative_offset,
            "lookup table",
        )?);
    }

    Ok(offsets)
}

fn apply_lookup(
    table: &[u8],
    table_offset: usize,
    lookup_offset: usize,
    kerning: &mut KerningTable,
) -> Result<(), FontError> {
    let lookup_type = read_u16(table, lookup_offset, table_offset)?;

    if lookup_type != 2 {
        return Ok(());
    }

    let subtable_count = usize::from(read_u16(
        table,
        checked_add(
            table_offset,
            lookup_offset,
            4,
            "GPOS lookup offset overflows",
        )?,
        table_offset,
    )?);

    let offsets_start = checked_add(
        table_offset,
        lookup_offset,
        6,
        "GPOS lookup subtable offset overflows",
    )?;

    let offsets_length = subtable_count
        .checked_mul(2)
        .ok_or_else(|| error(table_offset, offsets_start, "too many GPOS subtables"))?;

    read_bytes(table, offsets_start, offsets_length, table_offset)?;

    for index in 0..subtable_count {
        let relative_offset =
            usize::from(read_u16(table, offsets_start + index * 2, table_offset)?);

        let subtable_offset = child_offset(
            table,
            table_offset,
            lookup_offset,
            relative_offset,
            "pair positioning subtable",
        )?;

        apply_pair_positioning(table, table_offset, subtable_offset, kerning)?;
    }

    Ok(())
}

fn apply_pair_positioning(
    table: &[u8],
    table_offset: usize,
    subtable_offset: usize,
    kerning: &mut KerningTable,
) -> Result<(), FontError> {
    match read_u16(table, subtable_offset, table_offset)? {
        1 => apply_pair_positioning_format_1(table, table_offset, subtable_offset, kerning),
        2 => apply_pair_positioning_format_2(table, table_offset, subtable_offset, kerning),
        _ => Ok(()),
    }
}

fn apply_pair_positioning_format_1(
    table: &[u8],
    table_offset: usize,
    subtable_offset: usize,
    kerning: &mut KerningTable,
) -> Result<(), FontError> {
    let coverage_offset = child_offset(
        table,
        table_offset,
        subtable_offset,
        usize::from(read_u16(table, subtable_offset + 2, table_offset)?),
        "pair positioning coverage table",
    )?;

    let value_format_1 = read_u16(table, subtable_offset + 4, table_offset)?;
    let value_format_2 = read_u16(table, subtable_offset + 6, table_offset)?;

    if value_format_1 & !SUPPORTED_VALUE_FORMAT != 0
        || value_format_2 & !SUPPORTED_VALUE_FORMAT != 0
    {
        return Err(error(
            table_offset,
            subtable_offset,
            "unsupported GPOS value record format",
        ));
    }

    let pair_set_count = usize::from(read_u16(table, subtable_offset + 8, table_offset)?);
    let offsets_start = subtable_offset + 10;
    let offsets_length = pair_set_count
        .checked_mul(2)
        .ok_or_else(|| error(table_offset, offsets_start, "too many GPOS pair sets"))?;

    read_bytes(table, offsets_start, offsets_length, table_offset)?;

    for pair_set_index in 0..pair_set_count {
        let left_glyph = coverage_glyph_at(table, table_offset, coverage_offset, pair_set_index)?
            .ok_or_else(|| {
            error(
                table_offset,
                coverage_offset,
                "GPOS pair set has no matching coverage glyph",
            )
        })?;

        let pair_set_offset = child_offset(
            table,
            table_offset,
            subtable_offset,
            usize::from(read_u16(
                table,
                offsets_start + pair_set_index * 2,
                table_offset,
            )?),
            "pair set",
        )?;

        apply_pair_set(
            table,
            table_offset,
            pair_set_offset,
            left_glyph,
            value_format_1,
            value_format_2,
            kerning,
        )?;
    }

    Ok(())
}

fn apply_pair_positioning_format_2(
    table: &[u8],
    table_offset: usize,
    subtable_offset: usize,
    kerning: &mut KerningTable,
) -> Result<(), FontError> {
    let coverage_offset = child_offset(
        table,
        table_offset,
        subtable_offset,
        usize::from(read_u16(table, subtable_offset + 2, table_offset)?),
        "class pair positioning coverage table",
    )?;

    let value_format_1 = read_u16(table, subtable_offset + 4, table_offset)?;
    let value_format_2 = read_u16(table, subtable_offset + 6, table_offset)?;

    if value_format_1 & !SUPPORTED_VALUE_FORMAT != 0
        || value_format_2 & !SUPPORTED_VALUE_FORMAT != 0
    {
        return Err(error(
            table_offset,
            subtable_offset,
            "unsupported GPOS value record format",
        ));
    }

    let class_def_1_offset = child_offset(
        table,
        table_offset,
        subtable_offset,
        usize::from(read_u16(table, subtable_offset + 8, table_offset)?),
        "first class definition",
    )?;

    let class_def_2_offset = child_offset(
        table,
        table_offset,
        subtable_offset,
        usize::from(read_u16(table, subtable_offset + 10, table_offset)?),
        "second class definition",
    )?;

    let class1_count = usize::from(read_u16(table, subtable_offset + 12, table_offset)?);
    let class2_count = usize::from(read_u16(table, subtable_offset + 14, table_offset)?);

    if class1_count == 0 || class2_count == 0 {
        return Err(error(
            table_offset,
            subtable_offset,
            "GPOS class pair positioning has an empty class matrix",
        ));
    }

    let first_coverage = coverage_glyphs(table, table_offset, coverage_offset)?;
    let first_classes = class_definitions(table, table_offset, class_def_1_offset)?;
    let second_classes = class_definitions(table, table_offset, class_def_2_offset)?;

    validate_class_values(
        &first_classes,
        class1_count,
        table_offset,
        class_def_1_offset,
        "first",
    )?;

    validate_class_values(
        &second_classes,
        class2_count,
        table_offset,
        class_def_2_offset,
        "second",
    )?;

    let record_count = class1_count.checked_mul(class2_count).ok_or_else(|| {
        error(
            table_offset,
            subtable_offset,
            "GPOS class matrix is too large",
        )
    })?;

    let value_field_count = (value_format_1.count_ones() + value_format_2.count_ones()) as usize;

    let record_size = value_field_count.checked_mul(2).ok_or_else(|| {
        error(
            table_offset,
            subtable_offset,
            "GPOS value record is too large",
        )
    })?;

    if record_size == 0 {
        return Ok(());
    }

    let values_offset = subtable_offset + 16;
    let values_length = record_count.checked_mul(record_size).ok_or_else(|| {
        error(
            table_offset,
            values_offset,
            "GPOS class matrix is too large",
        )
    })?;

    read_bytes(table, values_offset, values_length, table_offset)?;

    let has_x_advance = value_format_1 & X_ADVANCE != 0;
    let mut adjustments = if has_x_advance {
        Vec::with_capacity(record_count)
    } else {
        Vec::new()
    };

    let mut cursor = values_offset;

    for _ in 0..record_count {
        let (x_advance, next_cursor) =
            read_value_record(table, table_offset, cursor, value_format_1)?;
        cursor = next_cursor;

        let (_, next_cursor) = read_value_record(table, table_offset, cursor, value_format_2)?;
        cursor = next_cursor;

        if has_x_advance {
            adjustments.push(x_advance);
        }
    }

    if has_x_advance {
        kerning.add_class_adjustment(
            first_coverage,
            first_classes,
            second_classes,
            class2_count,
            adjustments,
        );
    }

    Ok(())
}

fn coverage_glyphs(
    table: &[u8],
    table_offset: usize,
    coverage_offset: usize,
) -> Result<BTreeSet<u16>, FontError> {
    let mut glyphs = BTreeSet::new();

    match read_u16(table, coverage_offset, table_offset)? {
        1 => {
            let glyph_count = usize::from(read_u16(table, coverage_offset + 2, table_offset)?);
            read_bytes(table, coverage_offset + 4, glyph_count * 2, table_offset)?;

            for index in 0..glyph_count {
                glyphs.insert(read_u16(
                    table,
                    coverage_offset + 4 + index * 2,
                    table_offset,
                )?);
            }
        }
        2 => {
            let range_count = usize::from(read_u16(table, coverage_offset + 2, table_offset)?);
            read_bytes(table, coverage_offset + 4, range_count * 6, table_offset)?;

            for index in 0..range_count {
                let range_offset = coverage_offset + 4 + index * 6;
                let first = read_u16(table, range_offset, table_offset)?;
                let last = read_u16(table, range_offset + 2, table_offset)?;

                if first > last {
                    return Err(error(
                        table_offset,
                        range_offset,
                        "GPOS coverage range is reversed",
                    ));
                }

                glyphs.extend(first..=last);
            }
        }
        _ => {
            return Err(error(
                table_offset,
                coverage_offset,
                "unsupported GPOS coverage format",
            ));
        }
    }

    Ok(glyphs)
}

fn class_definitions(
    table: &[u8],
    table_offset: usize,
    class_def_offset: usize,
) -> Result<BTreeMap<u16, u16>, FontError> {
    let mut classes = BTreeMap::new();

    match read_u16(table, class_def_offset, table_offset)? {
        1 => {
            let first_glyph = read_u16(table, class_def_offset + 2, table_offset)?;
            let glyph_count = usize::from(read_u16(table, class_def_offset + 4, table_offset)?);

            read_bytes(table, class_def_offset + 6, glyph_count * 2, table_offset)?;

            for index in 0..glyph_count {
                let class = read_u16(table, class_def_offset + 6 + index * 2, table_offset)?;

                if class != 0 {
                    let glyph = first_glyph.checked_add(index as u16).ok_or_else(|| {
                        error(
                            table_offset,
                            class_def_offset,
                            "GPOS class definition glyph ID overflows",
                        )
                    })?;

                    classes.insert(glyph, class);
                }
            }
        }
        2 => {
            let range_count = usize::from(read_u16(table, class_def_offset + 2, table_offset)?);
            read_bytes(table, class_def_offset + 4, range_count * 6, table_offset)?;

            let mut previous_last = None;

            for index in 0..range_count {
                let range_offset = class_def_offset + 4 + index * 6;
                let first = read_u16(table, range_offset, table_offset)?;
                let last = read_u16(table, range_offset + 2, table_offset)?;
                let class = read_u16(table, range_offset + 4, table_offset)?;

                if first > last || previous_last.is_some_and(|previous| first <= previous) {
                    return Err(error(
                        table_offset,
                        range_offset,
                        "GPOS class definition ranges overlap or are reversed",
                    ));
                }

                previous_last = Some(last);

                if class != 0 {
                    for glyph in first..=last {
                        classes.insert(glyph, class);
                    }
                }
            }
        }
        _ => {
            return Err(error(
                table_offset,
                class_def_offset,
                "unsupported GPOS class definition format",
            ));
        }
    }

    Ok(classes)
}

fn validate_class_values(
    classes: &BTreeMap<u16, u16>,
    class_count: usize,
    table_offset: usize,
    class_def_offset: usize,
    name: &str,
) -> Result<(), FontError> {
    if classes
        .values()
        .any(|class| usize::from(*class) >= class_count)
    {
        return Err(error(
            table_offset,
            class_def_offset,
            format!("GPOS {name} class definition refers to a missing class"),
        ));
    }

    Ok(())
}

fn apply_pair_set(
    table: &[u8],
    table_offset: usize,
    pair_set_offset: usize,
    left_glyph: u16,
    value_format_1: u16,
    value_format_2: u16,
    kerning: &mut KerningTable,
) -> Result<(), FontError> {
    let pair_count = usize::from(read_u16(table, pair_set_offset, table_offset)?);
    let mut cursor = pair_set_offset + 2;
    let mut previous_right_glyph = None;

    for _ in 0..pair_count {
        let right_glyph = read_u16(table, cursor, table_offset)?;
        cursor = checked_add(table_offset, cursor, 2, "GPOS pair value offset overflows")?;

        if previous_right_glyph.is_some_and(|previous| right_glyph <= previous) {
            return Err(error(
                table_offset,
                cursor - 2,
                "GPOS pair values are not strictly ordered",
            ));
        }

        previous_right_glyph = Some(right_glyph);

        let (x_advance, next_cursor) =
            read_value_record(table, table_offset, cursor, value_format_1)?;
        cursor = next_cursor;

        let (_, next_cursor) = read_value_record(table, table_offset, cursor, value_format_2)?;
        cursor = next_cursor;

        if value_format_1 & X_ADVANCE != 0 {
            kerning.override_adjustment(left_glyph, right_glyph, x_advance);
        }
    }

    Ok(())
}

fn read_value_record(
    table: &[u8],
    table_offset: usize,
    mut cursor: usize,
    value_format: u16,
) -> Result<(i16, usize), FontError> {
    let mut x_advance = 0;

    for bit in 0..8 {
        if value_format & (1 << bit) == 0 {
            continue;
        }

        if bit == 2 {
            x_advance = read_i16(table, cursor, table_offset)?;
        } else {
            read_i16(table, cursor, table_offset)?;
        }

        cursor = checked_add(
            table_offset,
            cursor,
            2,
            "GPOS value record offset overflows",
        )?;
    }

    Ok((x_advance, cursor))
}

fn coverage_glyph_at(
    table: &[u8],
    table_offset: usize,
    coverage_offset: usize,
    target_index: usize,
) -> Result<Option<u16>, FontError> {
    match read_u16(table, coverage_offset, table_offset)? {
        1 => {
            let glyph_count = usize::from(read_u16(table, coverage_offset + 2, table_offset)?);

            if target_index >= glyph_count {
                return Ok(None);
            }

            Ok(Some(read_u16(
                table,
                coverage_offset + 4 + target_index * 2,
                table_offset,
            )?))
        }
        2 => {
            let range_count = usize::from(read_u16(table, coverage_offset + 2, table_offset)?);

            for range_index in 0..range_count {
                let range_offset = coverage_offset + 4 + range_index * 6;
                let first_glyph = read_u16(table, range_offset, table_offset)?;
                let last_glyph = read_u16(table, range_offset + 2, table_offset)?;
                let first_coverage_index =
                    usize::from(read_u16(table, range_offset + 4, table_offset)?);

                if first_glyph > last_glyph {
                    return Err(error(
                        table_offset,
                        range_offset,
                        "GPOS coverage range is reversed",
                    ));
                }

                let range_length = usize::from(last_glyph - first_glyph);
                let last_coverage_index = first_coverage_index
                    .checked_add(range_length)
                    .ok_or_else(|| {
                        error(table_offset, range_offset, "GPOS coverage index overflows")
                    })?;

                if (first_coverage_index..=last_coverage_index).contains(&target_index) {
                    let glyph_offset =
                        u16::try_from(target_index - first_coverage_index).map_err(|_| {
                            error(
                                table_offset,
                                range_offset,
                                "GPOS coverage glyph index is too large",
                            )
                        })?;

                    return Ok(Some(first_glyph + glyph_offset));
                }
            }

            Ok(None)
        }
        _ => Err(error(
            table_offset,
            coverage_offset,
            "unsupported GPOS coverage format",
        )),
    }
}

fn child_offset(
    table: &[u8],
    table_offset: usize,
    parent_offset: usize,
    relative_offset: usize,
    name: &str,
) -> Result<usize, FontError> {
    if relative_offset == 0 {
        return Err(error(
            table_offset,
            parent_offset,
            format!("GPOS {name} offset is zero"),
        ));
    }

    let offset = parent_offset.checked_add(relative_offset).ok_or_else(|| {
        error(
            table_offset,
            parent_offset,
            format!("GPOS {name} offset overflows"),
        )
    })?;

    if offset >= table.len() {
        return Err(error(
            table_offset,
            parent_offset,
            format!("GPOS {name} lies outside the table"),
        ));
    }

    Ok(offset)
}

fn checked_add(
    table_offset: usize,
    offset: usize,
    amount: usize,
    message: &str,
) -> Result<usize, FontError> {
    offset
        .checked_add(amount)
        .ok_or_else(|| error(table_offset, offset, message))
}

fn error(table_offset: usize, offset: usize, message: impl Into<String>) -> FontError {
    FontError {
        offset: table_offset.saturating_add(offset),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_pair_adjustment_kerning_from_gpos() {
        let data = font_with_gpos_pair_adjustment();
        let font = SfntFont::parse(&data).unwrap();
        let mut kerning = KerningTable::default();

        apply_kerning(&font, &mut kerning).unwrap();

        assert_eq!(kerning.adjustment(1, 2), -80);
        assert_eq!(kerning.adjustment(2, 1), 0);
    }

    fn font_with_gpos_pair_adjustment() -> Vec<u8> {
        let mut data = vec![0_u8; 90];
        let gpos_offset = 28;

        data[0..4].copy_from_slice(&0x0001_0000_u32.to_be_bytes());
        data[4..6].copy_from_slice(&1_u16.to_be_bytes());

        data[12..16].copy_from_slice(b"GPOS");
        data[20..24].copy_from_slice(&(gpos_offset as u32).to_be_bytes());
        data[24..28].copy_from_slice(&62_u32.to_be_bytes());

        data[gpos_offset..gpos_offset + 2].copy_from_slice(&1_u16.to_be_bytes());
        data[gpos_offset + 6..gpos_offset + 8].copy_from_slice(&10_u16.to_be_bytes());
        data[gpos_offset + 8..gpos_offset + 10].copy_from_slice(&24_u16.to_be_bytes());

        let feature_list = gpos_offset + 10;
        data[feature_list..feature_list + 2].copy_from_slice(&1_u16.to_be_bytes());
        data[feature_list + 2..feature_list + 6].copy_from_slice(b"kern");
        data[feature_list + 6..feature_list + 8].copy_from_slice(&8_u16.to_be_bytes());

        let feature_table = feature_list + 8;
        data[feature_table + 2..feature_table + 4].copy_from_slice(&1_u16.to_be_bytes());

        let lookup_list = gpos_offset + 24;
        data[lookup_list..lookup_list + 2].copy_from_slice(&1_u16.to_be_bytes());
        data[lookup_list + 2..lookup_list + 4].copy_from_slice(&4_u16.to_be_bytes());

        let lookup = lookup_list + 4;
        data[lookup..lookup + 2].copy_from_slice(&2_u16.to_be_bytes());
        data[lookup + 4..lookup + 6].copy_from_slice(&1_u16.to_be_bytes());
        data[lookup + 6..lookup + 8].copy_from_slice(&8_u16.to_be_bytes());

        let pair_positioning = lookup + 8;
        data[pair_positioning..pair_positioning + 2].copy_from_slice(&1_u16.to_be_bytes());
        data[pair_positioning + 2..pair_positioning + 4].copy_from_slice(&20_u16.to_be_bytes());
        data[pair_positioning + 4..pair_positioning + 6].copy_from_slice(&X_ADVANCE.to_be_bytes());
        data[pair_positioning + 8..pair_positioning + 10].copy_from_slice(&1_u16.to_be_bytes());
        data[pair_positioning + 10..pair_positioning + 12].copy_from_slice(&12_u16.to_be_bytes());

        let pair_set = pair_positioning + 12;
        data[pair_set..pair_set + 2].copy_from_slice(&1_u16.to_be_bytes());
        data[pair_set + 2..pair_set + 4].copy_from_slice(&2_u16.to_be_bytes());
        data[pair_set + 4..pair_set + 6].copy_from_slice(&(-80_i16).to_be_bytes());

        let coverage = pair_positioning + 20;
        data[coverage..coverage + 2].copy_from_slice(&1_u16.to_be_bytes());
        data[coverage + 2..coverage + 4].copy_from_slice(&1_u16.to_be_bytes());
        data[coverage + 4..coverage + 6].copy_from_slice(&1_u16.to_be_bytes());

        data
    }
}
