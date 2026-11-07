// SPDX-License-Identifier: GPL-3.0-only

use freeshow_data::{StageItemKind, StageLayout};
use freeshow_output::{
    Blend, Fit, OUTPUT_HEIGHT, OUTPUT_WIDTH, Rgba, SOURCE_HEIGHT, SOURCE_WIDTH, StyledItem,
    StyledLine, StyledSlide, TextRun, parse_color, render_styled,
};

pub struct StageSlide {
    pub lines: Vec<String>,
    pub notes: String,
}

pub fn paint_stage(
    slides: &[StageSlide],
    index: Option<usize>,
    layout: &StageLayout,
    clock: &str,
    timer: &str,
) -> freeshow_output::Frame {
    let background = parse_color(&layout.background).unwrap_or(Rgba {
        r: 0x12,
        g: 0x12,
        b: 0x1c,
        a: 255,
    });
    let items = layout
        .items
        .iter()
        .filter_map(|item| {
            let lines = match item.kind {
                StageItemKind::Slide => slide_lines(slides, index, item.offset, item.line_count),
                StageItemKind::Notes => {
                    let notes = slide_at(slides, index, item.offset)
                        .map(|slide| slide.notes.clone())
                        .unwrap_or_default();
                    if notes.trim().is_empty() {
                        Vec::new()
                    } else {
                        notes.lines().map(str::to_string).collect()
                    }
                }
                StageItemKind::Clock => vec![clock.to_string()],
                StageItemKind::Timer => vec![timer.to_string()],
            };
            if lines.iter().all(|line| line.is_empty()) {
                return None;
            }
            Some(StyledItem {
                style: item.style.clone(),
                align: String::new(),
                lines: lines
                    .into_iter()
                    .map(|line| StyledLine {
                        align: String::new(),
                        line_bar: true,
                        wrapped: false,
                        shift_y: 0,
                        runs: vec![TextRun {
                            text: line,
                            style: String::new(),
                            place_on: None,
                            paint: true,
                        }],
                    })
                    .collect(),
                image: None,
                fit: Fit::Fill,
                line_bg: None,
                line_gap: 0.0,
                line_radius: 0.0,
                flip_x: false,
                flip_y: false,
                opacity: 1.0,
                grayscale: 0.0,
                invert: 0.0,
                sepia: 0.0,
                brightness: 1.0,
                contrast: 1.0,
                saturate: 1.0,
                hue_rotate: 0.0,
                blur: 0.0,
                blend: Blend::Normal,
                crop: freeshow_output::Crop::default(),
            })
        })
        .collect();
    render_styled(&StyledSlide {
        output_width: OUTPUT_WIDTH,
        output_height: OUTPUT_HEIGHT,
        source_width: SOURCE_WIDTH,
        source_height: SOURCE_HEIGHT,
        background,
        items,
    })
}

fn slide_lines(
    slides: &[StageSlide],
    index: Option<usize>,
    offset: i64,
    line_count: Option<usize>,
) -> Vec<String> {
    let Some(slide) = slide_at(slides, index, offset) else {
        return Vec::new();
    };
    let mut lines = slide.lines.clone();
    if let Some(count) = line_count {
        lines.truncate(count);
    }
    lines
}

fn slide_at(slides: &[StageSlide], index: Option<usize>, offset: i64) -> Option<&StageSlide> {
    let index = index?;
    let position = index as i64 + offset;
    if position < 0 {
        return None;
    }
    slides.get(position as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use freeshow_data::default_stage;
    use freeshow_output::fingerprint;

    #[test]
    fn next_slide_notes_clock_and_timer_are_on_the_stage() {
        let slides = vec![
            StageSlide {
                lines: vec!["Amazing".into()],
                notes: "Stand".into(),
            },
            StageSlide {
                lines: vec!["one".into(), "two".into(), "three".into()],
                notes: String::new(),
            },
        ];
        let layout = default_stage();
        let first = paint_stage(&slides, Some(0), &layout, "12:00:00", "05:00");
        let later = paint_stage(&slides, Some(0), &layout, "12:00:01", "04:59");
        assert_ne!(fingerprint(&first.pixels), fingerprint(&later.pixels));
        assert_eq!(
            slide_lines(&slides, Some(0), 1, Some(2)),
            vec!["one".to_string(), "two".to_string()]
        );
        assert_eq!(slide_at(&slides, Some(0), 0).unwrap().notes, "Stand");
        assert!(first.pixels.iter().any(|byte| *byte != 0x12));
    }
}
