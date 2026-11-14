//! Shared partial inline text layout and font resolution.
//!
//! This is a partial layout model, not a full CSS inline formatting context.
//! CSS Text Level 3 whitespace processing:
//! <https://www.w3.org/TR/css-text-3/#white-space-processing>

use crate::{
    css::{FontStyle, FontWeight, TextStyle},
    graphics::font::{
        TrueTypeFont,
        render::{GLYPH_SUBPIXEL_STEPS, split_subpixel_position},
    },
    page::{Page, ResolvedFont, TextBlock},
    page_layout::{
        TextWrap, bitmap_text_advance, stack_ascent, stack_text_metrics, stack_word_width,
        text_metrics,
    },
    styled_tree::SourcePath,
};

pub(crate) struct PositionedTextRun {
    pub(crate) x: i32,
    pub(crate) offset_x: f32,
    pub(crate) y: i32,
    pub(crate) offset_y: f32,
    pub(crate) text: String,
    pub(crate) style: TextStyle,
    pub(crate) link: Option<String>,
}

pub(crate) fn same_font(left: &TextStyle, right: &TextStyle) -> bool {
    same_font_except_weight(left, right) && left.font_weight == right.font_weight
}

pub(crate) fn same_font_except_weight(left: &TextStyle, right: &TextStyle) -> bool {
    left.font_size.to_bits() == right.font_size.to_bits()
        && left.font_style == right.font_style
        && left.font_families == right.font_families
}

pub(crate) struct InlineFonts<'style, 'font> {
    entries: Vec<(&'style TextStyle, Vec<ResolvedFont<'font>>)>,
}

impl<'style, 'font> InlineFonts<'style, 'font> {
    pub(crate) fn from_styles(
        page: &'font Page,
        styles: impl IntoIterator<Item = &'style TextStyle>,
    ) -> Self {
        let mut entries: Vec<(&TextStyle, Vec<ResolvedFont<'font>>)> = Vec::new();

        for style in styles {
            if entries
                .iter()
                .any(|(existing, _)| same_font(existing, style))
            {
                continue;
            }

            entries.push((style, page.vector_fonts_for_style(style)));
        }

        Self { entries }
    }

    pub(crate) fn get(&self, style: &TextStyle) -> Option<&TrueTypeFont<'font>> {
        self.entries
            .iter()
            .find(|(existing, _)| same_font(existing, style))
            .and_then(|(_, fonts)| fonts.first().map(|entry| &entry.font))
    }

    pub(crate) fn stack(&self, style: &TextStyle) -> &[ResolvedFont<'font>] {
        self.entries
            .iter()
            .find(|(existing, _)| same_font(existing, style))
            .filter(|(_, fonts)| fonts.len() > 1)
            .map(|(_, fonts)| fonts.as_slice())
            .unwrap_or_default()
    }

    pub(crate) fn synthetic_weight(&self, style: &TextStyle) -> FontWeight {
        self.entries
            .iter()
            .find(|(existing, _)| same_font(existing, style))
            .and_then(|(_, fonts)| fonts.first())
            .map_or(style.font_weight, |entry| entry.synthesis.0)
    }

    pub(crate) fn synthetic_style(&self, style: &TextStyle) -> FontStyle {
        self.entries
            .iter()
            .find(|(existing, _)| same_font(existing, style))
            .and_then(|(_, fonts)| fonts.first())
            .map_or(style.font_style, |entry| entry.synthesis.1)
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

pub(crate) struct InlineTextFragment<'a> {
    pub(crate) text: &'a str,
    pub(crate) style: &'a TextStyle,
    pub(crate) style_source: Option<&'a SourcePath>,
    pub(crate) link: Option<&'a str>,
}

pub(crate) struct InlineWordPart<'a> {
    pub(crate) text: &'a str,
    pub(crate) style: &'a TextStyle,
    pub(crate) link: Option<&'a str>,
}

pub(crate) struct InlineWord<'a> {
    pub(crate) space_before: Option<&'a TextStyle>,
    pub(crate) parts: Vec<InlineWordPart<'a>>,
}

// Returns None when the legacy painter must handle this text block.
pub(crate) fn layout_text_block(
    page: &Page,
    block: &TextBlock,
    inherited: &TextStyle,
    width: i32,
    fallback: Option<&TrueTypeFont<'_>>,
) -> Option<(i32, Vec<PositionedTextRun>)> {
    // Keep the legacy behavior for non-CSS Unicode whitespace.
    if block
        .text
        .chars()
        .any(|ch| ch.is_whitespace() && !matches!(ch, ' ' | '\t' | '\n' | '\r' | '\x0c'))
    {
        return None;
    }

    if block.runs.is_empty() {
        return None;
    }

    let mut end = 0;
    let mut fragments = Vec::with_capacity(block.runs.len());

    for run in &block.runs {
        if run.range.start != end || !same_font_except_weight(inherited, &run.style) {
            return None;
        }

        let text = block.text.get(run.range.clone())?;
        end = run.range.end;

        fragments.push(InlineTextFragment {
            text,
            style: &run.style,
            style_source: None,
            link: run.link.as_deref(),
        });
    }

    if end != block.text.len()
        || !fragments.iter().any(|fragment| {
            fragment.style.color != inherited.color
                || fragment.style.font_weight != inherited.font_weight
                || fragment.link.is_some()
        })
    {
        return None;
    }

    let words = split_styled_words(&fragments);

    if words.is_empty() {
        return None;
    }

    let fonts = InlineFonts::from_styles(
        page,
        std::iter::once(inherited).chain(fragments.iter().map(|fragment| fragment.style)),
    );

    let has_vector = fragments
        .iter()
        .any(|fragment| fonts.get(fragment.style).or(fallback).is_some());

    let has_bitmap = fragments
        .iter()
        .any(|fragment| fonts.get(fragment.style).or(fallback).is_none());

    if has_vector && has_bitmap {
        return None;
    }

    if has_vector
        && words.iter().any(|word| {
            word.parts.first().is_some_and(|first| {
                word.parts.iter().any(|part| {
                    part.style.font_weight != first.style.font_weight
                        || part.style.color != first.style.color
                        || part.link != first.link
                }) && fonts.measure_vector_word(word, fallback).is_none()
            })
        })
    {
        return None;
    }

    Some(layout_inline_words(
        &fonts, &words, inherited, width, fallback, true,
    ))
}

pub(crate) fn layout_inline_words(
    fonts: &InlineFonts<'_, '_>,
    words: &[InlineWord<'_>],
    inherited: &TextStyle,
    width: i32,
    fallback: Option<&TrueTypeFont<'_>>,
    positioned: bool,
) -> (i32, Vec<PositionedTextRun>) {
    let primary = fonts.get(inherited).or(fallback);

    let base = InlineLineMetrics::from_stack(primary, fonts.stack(inherited), inherited.font_size);

    let mut wrap = TextWrap::new(0, width);
    let mut metrics = base;
    let mut line_top = 0_i32;
    let mut output = Vec::new();
    let mut pending = Vec::new();

    for word in words {
        let measured = fonts.measure_word(word, fallback);
        let placement = wrap.place_word(fonts.space_width(word, fallback), measured.width());

        if placement.wrapped {
            finish_inline_line(&mut pending, &mut output, metrics, line_top);
            line_top = line_top.saturating_add(metrics.height());
            metrics = base;
        }

        for run in measured.runs {
            let font = fonts.get(run.style).or(fallback);

            let run_metrics =
                InlineLineMetrics::from_stack(font, fonts.stack(run.style), run.style.font_size);

            metrics.include(run_metrics);

            if positioned {
                pending.push(PendingInlineRun {
                    x: placement.x.saturating_add(run.x),
                    offset_x: run.offset_x,
                    above_subpixels: run_metrics.above_subpixels,
                    text: run.text.into_owned(),
                    style: run.style.clone(),
                    link: run.link.map(str::to_owned),
                });
            }
        }
    }

    finish_inline_line(&mut pending, &mut output, metrics, line_top);

    (line_top.saturating_add(metrics.height()), output)
}

// Quantized approximation; this is not CSS baseline alignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct InlineLineMetrics {
    above_subpixels: i64,
    below_subpixels: i64,
}

impl InlineLineMetrics {
    #[cfg(test)]
    fn from_font(font: Option<&TrueTypeFont<'_>>, font_size: f32) -> Self {
        Self::from_stack(font, &[], font_size)
    }

    fn from_stack(
        font: Option<&TrueTypeFont<'_>>,
        stack: &[ResolvedFont<'_>],
        font_size: f32,
    ) -> Self {
        let metrics = stack_text_metrics(font, stack, font_size);
        let height = metrics.line_height.max(0);

        let steps = i64::from(GLYPH_SUBPIXEL_STEPS);
        let height_subpixels = i64::from(height) * steps;

        let above = stack_ascent(stack, font_size)
            .or_else(|| {
                font.and_then(|font| font.line_metrics(font_size))
                    .map(|metrics| metrics.ascent)
            })
            .and_then(split_subpixel_position)
            .map(|(base, phase)| i64::from(base) * steps + i64::from(phase))
            .unwrap_or(i64::from(metrics.glyph_height) * steps)
            .clamp(0, height_subpixels);

        Self {
            above_subpixels: above,
            below_subpixels: height_subpixels - above,
        }
    }

    fn include(&mut self, other: Self) {
        self.above_subpixels = self.above_subpixels.max(other.above_subpixels);
        self.below_subpixels = self.below_subpixels.max(other.below_subpixels);
    }

    fn height(self) -> i32 {
        let steps = i64::from(GLYPH_SUBPIXEL_STEPS);
        let height = (self.above_subpixels + self.below_subpixels + steps - 1) / steps;

        i32::try_from(height).unwrap_or(i32::MAX)
    }
}

struct PendingInlineRun {
    x: i32,
    offset_x: f32,
    above_subpixels: i64,
    text: String,
    style: TextStyle,
    link: Option<String>,
}

fn finish_inline_line(
    pending: &mut Vec<PendingInlineRun>,
    output: &mut Vec<PositionedTextRun>,
    metrics: InlineLineMetrics,
    line_top: i32,
) {
    let steps = i64::from(GLYPH_SUBPIXEL_STEPS);

    for run in pending.drain(..) {
        let offset = metrics
            .above_subpixels
            .saturating_sub(run.above_subpixels)
            .max(0);

        let whole = i32::try_from(offset / steps).unwrap_or(i32::MAX);
        let fraction = (offset % steps) as f32 / GLYPH_SUBPIXEL_STEPS as f32;

        output.push(PositionedTextRun {
            x: run.x,
            offset_x: run.offset_x,
            y: line_top.saturating_add(whole),
            offset_y: fraction,
            text: run.text,
            style: run.style,
            link: run.link,
        });
    }
}

impl<'style, 'font> InlineFonts<'style, 'font> {
    pub(crate) fn new(page: &'font Page, fragments: &[InlineTextFragment<'style>]) -> Self {
        Self::from_styles(page, fragments.iter().map(|fragment| fragment.style))
    }

    pub(crate) fn can_measure_vector_word(
        &self,
        word: &InlineWord<'_>,
        fallback: Option<&TrueTypeFont<'_>>,
    ) -> bool {
        self.measure_vector_word(word, fallback).is_some()
    }

    fn measure_word<'a>(
        &self,
        word: &InlineWord<'a>,
        fallback: Option<&TrueTypeFont<'_>>,
    ) -> MeasuredWord<'a> {
        if word.parts.first().is_some_and(|first| {
            word.parts.iter().any(|part| {
                part.style.font_weight != first.style.font_weight
                    || part.style.color != first.style.color
            })
        }) {
            if let Some(measured) = self.measure_vector_word(word, fallback) {
                return measured;
            }
        }

        measure_styled_word(word, |text, style| {
            let font = self.get(style).or(fallback);
            let stack = self.stack(style);

            let width = stack_word_width(
                font,
                stack,
                text,
                style.font_size,
                self.synthetic_weight(style),
                self.synthetic_style(style),
            );

            let advance = if font.is_none() {
                bitmap_text_advance(text, style.font_size)
            } else {
                width
            };

            (width, advance)
        })
    }

    fn measure_vector_word<'a>(
        &self,
        word: &InlineWord<'a>,
        fallback: Option<&TrueTypeFont<'_>>,
    ) -> Option<MeasuredWord<'a>> {
        if word
            .parts
            .iter()
            .any(|part| self.get(part.style).or(fallback).is_none())
        {
            return None;
        }

        let mut measured = measure_styled_word(word, |text, style| {
            let font = self.get(style).or(fallback).expect("font checked above");
            let stack = self.stack(style);

            let width = stack_word_width(
                Some(font),
                stack,
                text,
                style.font_size,
                self.synthetic_weight(style),
                self.synthetic_style(style),
            );

            (width, 0)
        });

        let mut pen = 0.0_f32;
        let mut previous: Option<(usize, u16)> = None;

        for run in &mut measured.runs {
            let font = self.get(run.style).or(fallback)?;
            let stack = self.stack(run.style);

            let first_character = run.text.chars().next()?;

            let first_font = stack
                .iter()
                .map(|entry| &entry.font)
                .find(|candidate| candidate.glyph_id(first_character) != 0)
                .unwrap_or(font);

            let identity = first_font.glyph_cache_key();
            let first_glyph = first_font.glyph_id(first_character);

            if let Some((previous_font, left)) = previous {
                if previous_font == identity {
                    let scale = run.style.font_size / f32::from(first_font.head().units_per_em);

                    pen += f32::from(first_font.kerning(left, first_glyph)) * scale;
                }
            }

            let (x, phase) = split_subpixel_position(pen)?;

            run.x = x;
            run.offset_x = f32::from(phase) / GLYPH_SUBPIXEL_STEPS as f32;

            pen += if stack.is_empty() {
                font.text_advance_width(&run.text, run.style.font_size)
                    .ok()?
            } else {
                crate::graphics::font::layout::visit_positioned_glyphs(
                    stack,
                    &run.text,
                    run.style.font_size,
                    |_, _, _| Ok(()),
                )
                .ok()?
            };

            let last_character = run.text.chars().last()?;

            let last_font = stack
                .iter()
                .map(|entry| &entry.font)
                .find(|candidate| candidate.glyph_id(last_character) != 0)
                .unwrap_or(font);

            previous = Some((
                last_font.glyph_cache_key(),
                last_font.glyph_id(last_character),
            ));
        }

        Some(measured)
    }

    fn space_width(&self, word: &InlineWord<'_>, fallback: Option<&TrueTypeFont<'_>>) -> i32 {
        word.space_before.map_or(0, |style| {
            let font = self.get(style).or(fallback);
            text_metrics(font, style.font_size).character_width
        })
    }
}

struct MeasuredRun<'a> {
    x: i32,
    offset_x: f32,
    width: i32,
    text: std::borrow::Cow<'a, str>,
    style: &'a TextStyle,
    link: Option<&'a str>,
}

struct MeasuredWord<'a> {
    runs: Vec<MeasuredRun<'a>>,
}

impl MeasuredWord<'_> {
    fn width(&self) -> i32 {
        self.runs
            .iter()
            .map(|run| {
                run.x
                    .saturating_add(run.width)
                    .saturating_add(i32::from(run.offset_x > 0.0))
            })
            .max()
            .unwrap_or(0)
    }
}

// Adjacent fragments with the same font and color are measured together so
// that element boundaries do not change kerning within a word.
fn measure_styled_word<'a>(
    word: &InlineWord<'a>,
    mut measure: impl FnMut(&str, &TextStyle) -> (i32, i32),
) -> MeasuredWord<'a> {
    let mut runs = Vec::new();
    let mut index = 0;
    let mut x = 0_i32;

    while index < word.parts.len() {
        let start = index;
        let style = word.parts[index].style;
        let link = word.parts[index].link;

        while index < word.parts.len()
            && same_font(style, word.parts[index].style)
            && style.color == word.parts[index].style.color
            && link == word.parts[index].link
        {
            index += 1;
        }

        let text = if index == start + 1 {
            std::borrow::Cow::Borrowed(word.parts[start].text)
        } else {
            std::borrow::Cow::Owned(
                word.parts[start..index]
                    .iter()
                    .map(|part| part.text)
                    .collect::<String>(),
            )
        };

        let (width, advance) = measure(text.as_ref(), style);

        runs.push(MeasuredRun {
            x,
            offset_x: 0.0,
            width,
            text,
            style,
            link,
        });

        x = x.saturating_add(advance);
    }

    MeasuredWord { runs }
}

// A partial white-space: normal model for the experimental preview.
// https://www.w3.org/TR/css-text-3/#white-space-processing
pub(crate) fn split_styled_words<'a>(fragments: &[InlineTextFragment<'a>]) -> Vec<InlineWord<'a>> {
    let mut words = Vec::new();
    let mut in_word = false;
    let mut pending_space = None;

    for fragment in fragments {
        let mut start = 0;

        for (index, character) in fragment.text.char_indices() {
            if matches!(character, ' ' | '\t' | '\n' | '\r' | '\x0c') {
                if start < index {
                    push_word_part(
                        &mut words,
                        &mut in_word,
                        &mut pending_space,
                        &fragment.text[start..index],
                        fragment.style,
                        fragment.link,
                    );
                }

                if in_word {
                    in_word = false;
                    pending_space = Some(fragment.style);
                }

                start = index + character.len_utf8();
            }
        }

        if start < fragment.text.len() {
            push_word_part(
                &mut words,
                &mut in_word,
                &mut pending_space,
                &fragment.text[start..],
                fragment.style,
                fragment.link,
            );
        }
    }

    words
}

fn push_word_part<'a>(
    words: &mut Vec<InlineWord<'a>>,
    in_word: &mut bool,
    pending_space: &mut Option<&'a TextStyle>,
    text: &'a str,
    style: &'a TextStyle,
    link: Option<&'a str>,
) {
    let part = InlineWordPart { text, style, link };

    if *in_word {
        words
            .last_mut()
            .expect("current word exists")
            .parts
            .push(part);
    } else {
        words.push(InlineWord {
            space_before: pending_space.take(),
            parts: vec![part],
        });

        *in_word = true;
    }
}

#[cfg(test)]
mod tests {
    use crate::css::{FontWeight, TextStyle};

    #[test]
    fn splits_words_across_styled_fragments() {
        use super::{InlineTextFragment, split_styled_words};

        let normal = TextStyle::initial();

        let mut bold = normal.clone();
        bold.font_weight = FontWeight::Bold;

        let fragments = vec![
            InlineTextFragment {
                text: "  hel",
                style: &normal,
                style_source: None,
                link: None,
            },
            InlineTextFragment {
                text: "lo \t",
                style: &bold,
                style_source: None,
                link: None,
            },
            InlineTextFragment {
                text: "\nмир  ",
                style: &normal,
                style_source: None,
                link: None,
            },
        ];

        let words = split_styled_words(&fragments);

        assert_eq!(words.len(), 2);
        assert!(words[0].space_before.is_none());

        assert_eq!(
            words[0]
                .parts
                .iter()
                .map(|part| part.text)
                .collect::<Vec<_>>(),
            vec!["hel", "lo"]
        );

        assert_eq!(words[0].parts[0].style.font_weight, FontWeight::Normal);
        assert_eq!(words[0].parts[1].style.font_weight, FontWeight::Bold);

        assert_eq!(words[1].parts[0].text, "мир");
        assert_eq!(words[1].space_before.unwrap().font_weight, FontWeight::Bold);
    }

    #[test]
    fn measures_styled_runs_with_offsets_and_combines_equal_styles() {
        use super::{InlineWord, InlineWordPart, measure_styled_word};

        let normal = TextStyle::initial();

        let mut bold = normal.clone();
        bold.font_weight = FontWeight::Bold;

        let word = InlineWord {
            space_before: None,
            parts: vec![
                InlineWordPart {
                    text: "h",
                    style: &normal,
                    link: None,
                },
                InlineWordPart {
                    text: "el",
                    style: &normal,
                    link: None,
                },
                InlineWordPart {
                    text: "lo",
                    style: &bold,
                    link: None,
                },
            ],
        };

        let mut calls = Vec::new();

        let measured = measure_styled_word(&word, |text, style| {
            calls.push((text.to_owned(), style.font_weight));

            if style.font_weight == FontWeight::Bold {
                (20, 20)
            } else {
                (21, 21)
            }
        });

        assert_eq!(
            calls,
            vec![
                ("hel".into(), FontWeight::Normal),
                ("lo".into(), FontWeight::Bold),
            ]
        );

        assert_eq!(measured.runs.len(), 2);
        assert_eq!((measured.runs[0].x, measured.runs[0].width), (0, 21));
        assert_eq!((measured.runs[1].x, measured.runs[1].width), (21, 20));
        assert_eq!(measured.runs[0].text, "hel");
        assert_eq!(measured.runs[1].text, "lo");
        assert_eq!(measured.runs[0].style.font_weight, FontWeight::Normal);
        assert_eq!(measured.runs[1].style.font_weight, FontWeight::Bold);
        assert_eq!(measured.width(), 41);
    }

    #[test]
    fn measures_empty_and_very_wide_words_safely() {
        use super::{InlineWord, InlineWordPart, measure_styled_word};

        let normal = TextStyle::initial();

        let mut bold = normal.clone();
        bold.font_weight = FontWeight::Bold;

        let empty = InlineWord {
            space_before: None,
            parts: Vec::new(),
        };

        assert_eq!(
            measure_styled_word(&empty, |_, _| unreachable!()).width(),
            0
        );

        let word = InlineWord {
            space_before: None,
            parts: vec![
                InlineWordPart {
                    text: "a",
                    style: &normal,
                    link: None,
                },
                InlineWordPart {
                    text: "b",
                    style: &bold,
                    link: None,
                },
            ],
        };

        let measured = measure_styled_word(&word, |_, _| (i32::MAX, i32::MAX));

        assert_eq!(measured.runs[1].x, i32::MAX);
        assert_eq!(measured.width(), i32::MAX);
    }

    #[test]
    fn measures_words_and_spaces_using_their_styles() {
        use super::{InlineFonts, InlineTextFragment, split_styled_words};

        let page = crate::page::parse_xhtml_for_test(
            "https://example.org/",
            "<html xmlns='http://www.w3.org/1999/xhtml'><head/><body/></html>",
        )
        .unwrap();

        let normal = TextStyle::initial();

        let mut same_font_other_color = normal.clone();
        same_font_other_color.color = 0x00_FF_00_00;

        let mut bold = normal.clone();
        bold.font_size = 32.0;
        bold.font_weight = FontWeight::Bold;

        let fragments = vec![
            InlineTextFragment {
                text: "a",
                style: &normal,
                style_source: None,
                link: None,
            },
            InlineTextFragment {
                text: "b ",
                style: &bold,
                style_source: None,
                link: None,
            },
            InlineTextFragment {
                text: "c",
                style: &same_font_other_color,
                style_source: None,
                link: None,
            },
        ];

        let fonts = InlineFonts::new(&page, &fragments);

        // Color does not require a separate font.
        assert_eq!(fonts.len(), 2);

        let words = split_styled_words(&fragments);
        assert_eq!(words.len(), 2);

        // Without loaded fonts, measurement uses the bitmap fallback.
        let first = fonts.measure_word(&words[0], None);

        assert_eq!(first.runs[0].width, 8);
        assert_eq!(first.runs[1].x, 8);
        assert_eq!(first.runs[1].width, 17);
        assert_eq!(first.width(), 25);

        assert_eq!(fonts.space_width(&words[0], None), 0);
        assert_eq!(fonts.space_width(&words[1], None), 16);
        assert_eq!(fonts.measure_word(&words[1], None).width(), 8);
    }

    #[test]
    fn preview_line_metrics_merge_above_and_below_baseline() {
        use super::InlineLineMetrics;

        let mut line = InlineLineMetrics::from_font(None, 16.0);

        assert_eq!(
            line,
            InlineLineMetrics {
                above_subpixels: 64,
                below_subpixels: 16
            }
        );

        line.include(InlineLineMetrics::from_font(None, 32.0));

        assert_eq!(
            line,
            InlineLineMetrics {
                above_subpixels: 128,
                below_subpixels: 32,
            }
        );

        line.include(InlineLineMetrics {
            above_subpixels: 40,
            below_subpixels: 96,
        });

        assert_eq!(line.height(), 28);
        assert_eq!(InlineLineMetrics::from_font(None, 0.0).height(), 0);
    }

    #[test]
    fn preview_line_height_preserves_the_existing_vector_font_height() {
        use super::InlineLineMetrics;
        use crate::graphics::font::{TrueTypeFont, normalize_font_container};
        use crate::page_layout::text_metrics;

        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/fonts/noto/nko-hmtx.woff2",
        );

        let data =
            normalize_font_container(std::fs::read(path).expect("Nko font fixture must exist"))
                .unwrap();

        let font = TrueTypeFont::parse(&data).unwrap();

        for size in [0.0, 12.0, 16.0, 32.0, 64.0] {
            let preview = InlineLineMetrics::from_font(Some(&font), size);
            let existing = text_metrics(Some(&font), size);

            assert_eq!(preview.height(), existing.line_height, "size={size}");
        }
    }

    #[test]
    fn aligns_preview_runs_after_all_line_metrics_are_known() {
        use super::{InlineLineMetrics, PendingInlineRun, finish_inline_line};

        let small = TextStyle::initial();

        let mut large = small.clone();
        large.font_size = 32.0;

        let small_metrics = InlineLineMetrics::from_font(None, small.font_size);
        let large_metrics = InlineLineMetrics::from_font(None, large.font_size);

        let mut line_metrics = small_metrics;
        line_metrics.include(large_metrics);

        let mut pending = vec![
            PendingInlineRun {
                x: 4,
                offset_x: 0.0,
                above_subpixels: small_metrics.above_subpixels,
                text: "small".into(),
                style: small,
                link: None,
            },
            PendingInlineRun {
                x: 40,
                offset_x: 0.0,
                above_subpixels: large_metrics.above_subpixels,
                text: "large".into(),
                style: large,
                link: None,
            },
        ];

        let mut runs = Vec::new();

        finish_inline_line(&mut pending, &mut runs, line_metrics, 20);

        assert!(pending.is_empty());
        assert_eq!(runs.len(), 2);

        assert_eq!(
            (runs[0].x, runs[0].y, runs[0].text.as_str()),
            (4, 28, "small")
        );

        assert_eq!(
            (runs[1].x, runs[1].y, runs[1].text.as_str()),
            (40, 20, "large")
        );

        assert_eq!(
            i64::from(runs[0].y) * 8 + small_metrics.above_subpixels,
            i64::from(runs[1].y) * 8 + line_metrics.above_subpixels
        );

        assert_eq!(runs[0].style.font_size, 16.0);
        assert_eq!(runs[1].style.font_size, 32.0);
    }

    #[test]
    fn preview_vector_baseline_matches_rasterizer_integer_coordinate() {
        use super::InlineLineMetrics;
        use crate::graphics::font::{
            TrueTypeFont, normalize_font_container, render::split_subpixel_position,
        };

        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/fonts/noto/nko-hmtx.woff2",
        );

        let data =
            normalize_font_container(std::fs::read(path).expect("Nko font fixture must exist"))
                .unwrap();
        let font = TrueTypeFont::parse(&data).unwrap();

        let mut saw_fractional_phase = false;
        let mut saw_round_mismatch = false;

        for size in 1..=128 {
            let size = size as f32;
            let metrics = font.line_metrics(size).unwrap();

            let (base, phase) = split_subpixel_position(metrics.ascent).unwrap();

            let preview = InlineLineMetrics::from_font(Some(&font), size);

            assert_eq!(
                preview.above_subpixels,
                i64::from(base) * 8 + i64::from(phase),
                "size={size}",
            );

            saw_fractional_phase |= phase != 0;
            saw_round_mismatch |= metrics.ascent.round() as i32 != base;
        }

        assert!(saw_fractional_phase);
        assert!(saw_round_mismatch);
    }

    #[test]
    fn aligns_preview_runs_with_fractional_baselines() {
        use super::{InlineLineMetrics, PendingInlineRun, finish_inline_line};

        let style = TextStyle::initial();

        let mut line = InlineLineMetrics {
            above_subpixels: 85,
            below_subpixels: 11,
        };

        line.include(InlineLineMetrics {
            above_subpixels: 98,
            below_subpixels: 6,
        });

        let mut pending = vec![
            PendingInlineRun {
                x: 0,
                offset_x: 0.0,
                above_subpixels: 85,
                text: "a".into(),
                style: style.clone(),
                link: None,
            },
            PendingInlineRun {
                x: 20,
                offset_x: 0.0,
                above_subpixels: 98,
                text: "b".into(),
                style,
                link: None,
            },
        ];

        let mut runs = Vec::new();

        finish_inline_line(&mut pending, &mut runs, line, 10);

        assert_eq!(runs.len(), 2);
        assert_eq!((runs[0].y, runs[0].offset_y), (11, 0.625));
        assert_eq!((runs[1].y, runs[1].offset_y), (10, 0.0));

        let first_baseline = i64::from(runs[0].y) * 8 + (runs[0].offset_y * 8.0) as i64 + 85;
        let second_baseline = i64::from(runs[1].y) * 8 + (runs[1].offset_y * 8.0) as i64 + 98;

        assert_eq!(first_baseline, second_baseline);
        assert_eq!(line.height(), 14);
    }

    #[test]
    fn preview_fractional_baseline_reaches_vector_pixels() {
        use super::{InlineLineMetrics, PendingInlineRun, finish_inline_line};
        use crate::graphics::font::{TrueTypeFont, normalize_font_container};
        use crate::layout_preview::{PreviewCommand, PreviewLayout};
        use crate::page_layout::LayoutRect;
        use crate::render::{GlyphRasterCache, paint_preview_scene};

        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/fonts/noto/nko-hmtx.woff2",
        );

        let data =
            normalize_font_container(std::fs::read(path).expect("Nko font fixture must exist"))
                .unwrap();

        let font = TrueTypeFont::parse(&data).unwrap();
        let small_metrics = InlineLineMetrics::from_font(Some(&font), 24.0);

        // Find a second size that requires a half-pixel adjustment.
        let (large_size, large_metrics) = (25..=64)
            .find_map(|size| {
                let metrics = InlineLineMetrics::from_font(Some(&font), size as f32);
                let difference = metrics.above_subpixels - small_metrics.above_subpixels;

                (difference > 0 && difference % 8 == 4).then_some((size as f32, metrics))
            })
            .expect("fixture must produce a half-pixel baseline difference");

        let mut line = small_metrics;
        line.include(large_metrics);

        let mut small = TextStyle::initial();
        small.font_size = 24.0;
        small.color = 0x00_FF_00_00;

        let mut large = small.clone();
        large.font_size = large_size;
        large.color = 0x00_00_00_FF;

        let mut pending = vec![
            PendingInlineRun {
                x: 0,
                offset_x: 0.0,
                above_subpixels: small_metrics.above_subpixels,
                text: "ߊ".into(),
                style: small.clone(),
                link: None,
            },
            PendingInlineRun {
                x: 100,
                offset_x: 0.0,
                above_subpixels: large_metrics.above_subpixels,
                text: "ߊ".into(),
                style: large,
                link: None,
            },
        ];

        let mut runs = Vec::new();
        finish_inline_line(&mut pending, &mut runs, line, 0);

        assert_eq!(runs[0].offset_y, 0.5);
        assert_eq!(runs[1].offset_y, 0.0);

        let baseline0 = i64::from(runs[0].y) * 8
            + (runs[0].offset_y * 8.0) as i64
            + small_metrics.above_subpixels;

        let baseline1 = i64::from(runs[1].y) * 8
            + (runs[1].offset_y * 8.0) as i64
            + large_metrics.above_subpixels;

        assert_eq!(baseline0, baseline1);

        let page = crate::page::parse_xhtml_for_test(
            "https://example.org/",
            "<html xmlns='http://www.w3.org/1999/xhtml'><head/><body/></html>",
        )
        .unwrap();

        let mut layout = PreviewLayout {
            boxes: vec![],
            display_list: vec![PreviewCommand::Text {
                rect: LayoutRect {
                    x: 16,
                    y: 0,
                    width: 220,
                    height: line.height(),
                },
                text: String::new(),
                style: small,
                runs,
            }],
        };

        let (width, height) = (256, 220);
        let mut aligned = vec![0; width * height];

        paint_preview_scene(
            &mut aligned,
            width,
            height,
            &page,
            &layout,
            Some(&data),
            0,
            &mut GlyphRasterCache::new(),
        );

        assert!(
            aligned.iter().any(|&pixel| {
                let red = (pixel >> 16) & 0xff;
                let green = (pixel >> 8) & 0xff;
                red > green
            }),
            "first vector glyph must be visible"
        );

        assert!(
            aligned.iter().any(|&pixel| {
                let blue = pixel & 0xff;
                let green = (pixel >> 8) & 0xff;
                blue > green
            }),
            "second vector glyph must be visible"
        );

        let PreviewCommand::Text { runs, .. } = &mut layout.display_list[0] else {
            panic!("text command expected");
        };

        runs[0].offset_y = 0.0;

        let mut unaligned = vec![0; width * height];

        paint_preview_scene(
            &mut unaligned,
            width,
            height,
            &page,
            &layout,
            Some(&data),
            0,
            &mut GlyphRasterCache::new(),
        );

        assert_ne!(
            aligned, unaligned,
            "fractional alignment should change rasterization"
        );

        // The second glyph must not change when only the first run moves.
        for y in 90..height {
            assert_eq!(
                &aligned[y * width + 116..y * width + 180],
                &unaligned[y * width + 116..y * width + 180]
            );
        }
    }

    #[test]
    fn word_width_includes_earlier_run_overhang() {
        use super::{InlineWord, InlineWordPart, measure_styled_word};

        let normal = TextStyle::initial();

        let mut bold = normal.clone();
        bold.font_weight = FontWeight::Bold;

        let word = InlineWord {
            space_before: None,
            parts: vec![
                InlineWordPart {
                    text: "A",
                    style: &bold,
                    link: None,
                },
                InlineWordPart {
                    text: ".",
                    style: &normal,
                    link: None,
                },
            ],
        };

        let measured = measure_styled_word(&word, |_, style| {
            if style.font_weight == FontWeight::Bold {
                (30, 10)
            } else {
                (2, 2)
            }
        });

        assert_eq!((measured.runs[1].x, measured.runs[1].width), (10, 2));
        assert_eq!(measured.width(), 30);
    }
}
