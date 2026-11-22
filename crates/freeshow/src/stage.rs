// SPDX-License-Identifier: GPL-3.0-only

use std::collections::{HashMap, HashSet};
use std::path::Path;

use freeshow_data::{StageClockOptions, StageItemKind, StageItemSpec, StageLayout, fill_variables};
use freeshow_output::{
    Blend, Fit, OUTPUT_HEIGHT, OUTPUT_WIDTH, Raster, Rgba, SOURCE_HEIGHT, SOURCE_WIDTH, StyledItem,
    StyledLine, StyledSlide, TextRun, parse_color, render_styled,
};

pub struct StageSlide {
    pub text_boxes: Vec<Vec<String>>,
    pub notes: String,
}

#[derive(Default)]
pub struct StageMedia {
    files: HashMap<String, StageFile>,
}

#[derive(Default)]
struct StageFile {
    picture: Option<Raster>,
    video: Option<freeshow_media::VideoPlayer>,
}

impl StageMedia {
    pub fn update<'a>(&mut self, root: Option<&Path>, sources: impl Iterator<Item = &'a str>) {
        let sources: HashSet<_> = sources.collect();
        self.files.retain(|src, _| sources.contains(src.as_str()));
        for src in sources {
            let file = self.files.entry(src.to_owned()).or_insert_with(|| {
                let Some(path) = crate::live::resolve_media(root, src) else {
                    return StageFile::default();
                };
                if freeshow_media::media_kind(&path) == freeshow_media::MediaKind::Video {
                    let video =
                        freeshow_media::VideoPlayer::open_with_network(&path, &[], &[]).ok();
                    StageFile {
                        picture: None,
                        video,
                    }
                } else {
                    let picture = std::fs::read(path)
                        .ok()
                        .and_then(|bytes| freeshow_output::decode_image(&bytes));
                    StageFile {
                        picture,
                        video: None,
                    }
                }
            });
            if let Some(frame) = file
                .video
                .as_mut()
                .and_then(|player| player.try_next_frame())
            {
                file.picture = Some(frame);
            }
        }
    }

    fn picture(&self, src: &str) -> Option<Raster> {
        self.files.get(src)?.picture.clone()
    }
}

/// Values the stage boxes need that are not part of the layout.
#[derive(Default)]
pub struct StageContext<'a> {
    /// `(timer id, label)` for stage boxes that name a timer.
    pub timers: &'a [(String, String)],
    /// Variable values, keyed by raw id and by name.
    pub variables: &'a [(String, String)],
    /// The active audience output id, saved name, and composited picture.
    pub audience: Option<(&'a str, &'a str, &'a Raster)>,
    pub media: Option<&'a StageMedia>,
    pub current_background: Option<(&'a Raster, Fit, bool, bool)>,
    pub still_backgrounds: Option<&'a HashMap<i64, String>>,
    /// When true, clock boxes use FreeShow's 12-hour `timeFormat`.
    pub twelve_hour: bool,
}

pub fn paint_stage(
    slides: &[StageSlide],
    index: Option<usize>,
    layout: &StageLayout,
    clock: &str,
    timer: &str,
    context: &StageContext,
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
            let mut lines = lines_for(item, slides, index, clock, timer, context);
            let output_preview = item.kind == StageItemKind::CurrentOutput;
            let mut fit = item.fit.as_deref().map(Fit::named).unwrap_or(Fit::Contain);
            let mut flip_x = item.flip_x;
            let mut flip_y = item.flip_y;
            let image = if output_preview {
                let (id, name, image) = context.audience?;
                if item
                    .output_source
                    .as_deref()
                    .is_some_and(|source| source != id)
                {
                    return None;
                }
                if item.show_output_label && !name.is_empty() {
                    lines.push(name.to_owned());
                }
                fit = Fit::Contain;
                Some(image.clone())
            } else {
                match item.kind {
                    StageItemKind::Media => item
                        .src
                        .as_deref()
                        .and_then(|src| context.media?.picture(src)),
                    StageItemKind::QrCode => {
                        fit = Fit::Contain;
                        item.qr.as_ref().and_then(|qr| {
                            freeshow_output::qr_image(&qr.text, &qr.color, &qr.background)
                        })
                    }
                    StageItemKind::Slide if item.include_media && item.offset == 0 => context
                        .current_background
                        .map(|(image, audience_fit, x, y)| {
                            fit = audience_fit;
                            flip_x = x;
                            flip_y = y;
                            image.clone()
                        }),
                    StageItemKind::Slide if item.include_media => context
                        .still_backgrounds
                        .and_then(|backgrounds| backgrounds.get(&item.offset))
                        .and_then(|src| context.media?.picture(src)),
                    _ => None,
                }
            };
            if image.is_some() && !output_preview {
                lines.clear();
            } else if image.is_none() && item.kind == StageItemKind::Media {
                lines = vec![media_label(item)];
            }
            if lines.iter().all(|line| line.is_empty()) && image.is_none() {
                return None;
            }
            Some(StyledItem {
                style: item.style.clone(),
                align: if output_preview {
                    "align-items:end".into()
                } else {
                    item.align.clone()
                },
                lines: lines
                    .into_iter()
                    .map(|line| StyledLine {
                        align: if output_preview {
                            "center".into()
                        } else {
                            String::new()
                        },
                        line_bar: true,
                        wrapped: false,
                        shift_y: 0,
                        runs: vec![TextRun {
                            text: line,
                            style: if output_preview {
                                "font-size:28px;color:#ffffff;".into()
                            } else {
                                String::new()
                            },
                            place_on: None,
                            paint: true,
                        }],
                    })
                    .collect(),
                image,
                fit,
                line_bg: output_preview.then_some(Rgba {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 160,
                }),
                line_gap: 0.0,
                line_radius: 0.0,
                flip_x,
                flip_y,
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

fn lines_for(
    item: &StageItemSpec,
    slides: &[StageSlide],
    index: Option<usize>,
    clock: &str,
    timer: &str,
    context: &StageContext,
) -> Vec<String> {
    match item.kind {
        StageItemKind::Slide => slide_lines(slides, index, item),
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
        StageItemKind::Clock => vec![clock_text(clock, &item.clock, context.twelve_hour)],
        StageItemKind::Timer => vec![timer_text(item, timer, context)],
        StageItemKind::Text => item
            .lines
            .iter()
            .map(|line| fill_variables(line, context.variables))
            .collect(),
        StageItemKind::Variable => item
            .variable_id
            .as_deref()
            .and_then(|id| variable_value(id, context.variables))
            .map(|value| vec![value])
            .unwrap_or_default(),
        StageItemKind::SlideTracker => tracker_text(slides, index),
        StageItemKind::Media | StageItemKind::QrCode | StageItemKind::CurrentOutput => Vec::new(),
        StageItemKind::Camera | StageItemKind::Custom => vec![if !item.label.is_empty() {
            item.label.clone()
        } else if let Some(name) = item.device_name.as_deref().filter(|name| !name.is_empty()) {
            name.to_string()
        } else {
            item.id.clone()
        }],
    }
}

fn slide_lines(slides: &[StageSlide], index: Option<usize>, item: &StageItemSpec) -> Vec<String> {
    let Some(slide) = slide_at(slides, index, item.offset) else {
        return Vec::new();
    };
    let mut lines = if let Some(number) = item.text_box {
        slide
            .text_boxes
            .iter()
            .rev()
            .nth(number)
            .cloned()
            .unwrap_or_default()
    } else {
        let mut boxes = slide.text_boxes.iter().collect::<Vec<_>>();
        if !item.invert_items {
            boxes.reverse();
        }
        let mut lines = Vec::new();
        for group in boxes.into_iter().filter(|group| !group.is_empty()) {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            lines.extend(group.iter().cloned());
        }
        lines
    };
    if let Some(count) = item.line_count {
        lines.truncate(count);
    }
    lines
}

fn slide_at(slides: &[StageSlide], index: Option<usize>, offset: i64) -> Option<&StageSlide> {
    slides.get(index?.checked_add_signed(isize::try_from(offset).ok()?)?)
}

fn tracker_text(slides: &[StageSlide], index: Option<usize>) -> Vec<String> {
    let Some(index) = index.filter(|_| !slides.is_empty()) else {
        return Vec::new();
    };
    vec![format!("{} / {}", index + 1, slides.len())]
}

fn timer_text(item: &StageItemSpec, timer: &str, context: &StageContext) -> String {
    item.timer_id
        .as_deref()
        .and_then(|id| {
            context
                .timers
                .iter()
                .find(|(timer_id, _)| timer_id == id)
                .map(|(_, label)| label.clone())
        })
        .unwrap_or_else(|| timer.to_string())
}

fn variable_value(id: &str, variables: &[(String, String)]) -> Option<String> {
    let id = id.trim();
    if id.is_empty() {
        return None;
    }
    let direct = variables
        .iter()
        .find(|(key, _)| key == id)
        .map(|(_, value)| value.clone());
    direct.or_else(|| {
        let name = id
            .strip_prefix("variable_")
            .or_else(|| id.strip_prefix('$'))
            .unwrap_or(id)
            .to_lowercase()
            .replace(' ', "_");
        variables
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.clone())
    })
}

fn media_label(item: &StageItemSpec) -> String {
    if !item.label.is_empty() {
        return item.label.clone();
    }
    item.src
        .as_deref()
        .and_then(|src| Path::new(src).file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("Media")
        .to_string()
}

fn clock_text(clock: &str, options: &StageClockOptions, twelve_hour: bool) -> String {
    let mut text = freeshow_data::format_clock(clock, twelve_hour, options.seconds);
    let date = format_date(&options.date_format, &chrono::Local::now());
    if !date.is_empty() {
        text.push(' ');
        text.push_str(&date);
    }
    text
}

fn format_date(format: &str, now: &chrono::DateTime<chrono::Local>) -> String {
    if format.is_empty() || format.eq_ignore_ascii_case("none") {
        return String::new();
    }
    let mut out = String::new();
    let mut rest = format;
    while let Some(index) = rest.find(|character: char| character.is_ascii_alphabetic()) {
        out.push_str(&rest[..index]);
        rest = &rest[index..];
        let (token, value) = [
            ("YYYY", now.format("%Y").to_string()),
            ("DD", now.format("%d").to_string()),
            ("MM", now.format("%m").to_string()),
            ("HH", now.format("%H").to_string()),
            ("mm", now.format("%M").to_string()),
            ("ss", now.format("%S").to_string()),
        ]
        .into_iter()
        .find(|(token, _)| rest.starts_with(token))
        .unwrap_or_else(|| (&rest[..1], rest[..1].to_string()));
        out.push_str(&value);
        rest = &rest[token.len()..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use freeshow_data::{StageClockOptions, default_stage};
    use freeshow_output::fingerprint;

    fn spec(kind: StageItemKind) -> StageItemSpec {
        StageItemSpec {
            kind,
            id: "custom".into(),
            style: "left:0px;top:0px;width:600px;height:120px;font-size:48px;".into(),
            align: String::new(),
            offset: 0,
            line_count: None,
            text_box: None,
            invert_items: false,
            label: "Web".into(),
            lines: Vec::new(),
            variable_id: None,
            timer_id: None,
            src: None,
            device_name: None,
            device_type: None,
            output_source: None,
            show_output_label: false,
            include_media: false,
            fit: None,
            flip_x: false,
            flip_y: false,
            qr: None,
            clock: StageClockOptions::default(),
        }
    }

    #[test]
    fn next_slide_notes_clock_and_timer_are_on_the_stage() {
        let slides = vec![
            StageSlide {
                text_boxes: vec![vec!["Amazing".into()]],
                notes: "Stand".into(),
            },
            StageSlide {
                text_boxes: vec![vec!["one".into(), "two".into(), "three".into()]],
                notes: String::new(),
            },
        ];
        let layout = default_stage();
        let context = StageContext::default();
        let first = paint_stage(&slides, Some(0), &layout, "12:00:00", "05:00", &context);
        let later = paint_stage(&slides, Some(0), &layout, "12:00:01", "04:59", &context);
        assert_ne!(fingerprint(&first.pixels), fingerprint(&later.pixels));
        assert_eq!(
            slide_lines(&slides, Some(0), &layout.items[1]),
            vec!["one".to_string(), "two".to_string()]
        );
        assert_eq!(slide_at(&slides, Some(0), 0).unwrap().notes, "Stand");
        assert!(slide_at(&slides, Some(1), i64::MAX).is_none());
        assert!(first.pixels.iter().any(|byte| *byte != 0x12));
    }

    #[test]
    fn every_freeshow_item_kind_produces_lines() {
        let slides = vec![
            StageSlide {
                text_boxes: vec![vec!["current".into()]],
                notes: "note".into(),
            },
            StageSlide {
                text_boxes: vec![vec!["next".into()]],
                notes: String::new(),
            },
        ];
        let timers = vec![("sermon".to_string(), "03:00".to_string())];
        let variables = vec![
            ("v1".to_string(), "Ada".to_string()),
            ("speaker".to_string(), "Ada".to_string()),
        ];
        let context = StageContext {
            timers: &timers,
            variables: &variables,
            ..StageContext::default()
        };

        let mut text = spec(StageItemKind::Text);
        text.lines = vec!["Hello {variable_speaker}".into()];
        let mut variable = spec(StageItemKind::Variable);
        variable.variable_id = Some("v1".into());
        let mut timer = spec(StageItemKind::Timer);
        timer.timer_id = Some("sermon".into());
        let mut clock = spec(StageItemKind::Clock);
        clock.clock = StageClockOptions {
            seconds: false,
            date_format: "none".into(),
        };
        let mut unlabeled = spec(StageItemKind::Custom);
        unlabeled.id = "web".into();
        unlabeled.label = String::new();
        let mut camera = spec(StageItemKind::Camera);
        camera.label = String::new();
        camera.device_name = Some("PTZ Front".into());

        let cases = [
            (spec(StageItemKind::Slide), vec!["current".to_string()]),
            (spec(StageItemKind::Notes), vec!["note".to_string()]),
            (clock, vec!["12:00".to_string()]),
            (timer, vec!["03:00".to_string()]),
            (text, vec!["Hello Ada".to_string()]),
            (variable, vec!["Ada".to_string()]),
            (spec(StageItemKind::SlideTracker), vec!["1 / 2".to_string()]),
            (spec(StageItemKind::Media), Vec::new()),
            (spec(StageItemKind::Custom), vec!["Web".to_string()]),
            (unlabeled, vec!["web".to_string()]),
            (camera, vec!["PTZ Front".to_string()]),
        ];
        for (item, expected) in cases {
            assert_eq!(
                lines_for(&item, &slides, Some(0), "12:00:00", "05:00", &context),
                expected,
                "{:?}",
                item.kind
            );
        }
    }

    #[test]
    fn stage_text_can_select_a_box_or_reverse_its_order() {
        let mut show = freeshow_data::ShowFile::blank("bilingual", "Bilingual");
        show.show["slides"]["s1"]["items"] = serde_json::json!([
            {"lines":[{"text":[{"value":"First language"}]}]},
            {"type":"media","lines":[{"text":[{"value":"Ignore this image"}]}]},
            {"bindings":["audience"],"lines":[{"text":[{"value":"Room only"}]}]},
            {"bindings":["stage"],"clickReveal":true,
                "lines":[{"text":[{"value":"Second language"}]}]}
        ]);

        let mut item = spec(StageItemKind::Slide);
        item.text_box = Some(0);
        let mut slides = [StageSlide {
            text_boxes: show.stage_text_boxes(0, true, 0, false, None),
            notes: String::new(),
        }];
        assert!(slide_lines(&slides, Some(0), &item).is_empty());
        item.text_box = Some(1);
        assert_eq!(slide_lines(&slides, Some(0), &item), ["First language"]);

        slides[0].text_boxes = show.stage_text_boxes(0, true, 0, true, None);
        item.text_box = None;
        assert_eq!(
            slide_lines(&slides, Some(0), &item),
            ["Second language", "", "First language"]
        );
        item.invert_items = true;
        assert_eq!(
            slide_lines(&slides, Some(0), &item),
            ["First language", "", "Second language"]
        );
        item.text_box = Some(0);
        assert_eq!(slide_lines(&slides, Some(0), &item), ["Second language"]);
    }

    #[test]
    fn a_custom_box_and_a_missing_media_file_are_still_painted() {
        let slides = vec![StageSlide {
            text_boxes: vec![vec!["line".into()]],
            notes: String::new(),
        }];
        let mut custom = spec(StageItemKind::Custom);
        custom.label = "Web".into();
        let mut media = spec(StageItemKind::Media);
        media.label = String::new();
        media.src = Some("Media/missing.png".into());
        media.style = "left:0px;top:200px;width:600px;height:120px;".into();
        let layout = StageLayout {
            name: "Custom".into(),
            background: "#000000".into(),
            items: vec![custom, media],
        };
        let context = StageContext::default();
        let frame = paint_stage(&slides, Some(0), &layout, "12:00:00", "05:00", &context);
        assert!(frame.pixels.iter().any(|byte| *byte != 0));
    }

    #[test]
    fn clock_dates_follow_the_stored_format() {
        let now = chrono::Local::now();
        assert_eq!(format_date("none", &now), "");
        assert_eq!(
            format_date("YYYY-MM-DD", &now),
            now.format("%Y-%m-%d").to_string()
        );
        assert_eq!(
            format_date("DD/MM/YYYY HH:mm", &now),
            format!("{} {}", now.format("%d/%m/%Y"), now.format("%H:%M"))
        );
    }

    #[test]
    fn variables_fall_back_from_raw_id_to_name() {
        let values = vec![("speaker".to_string(), "Ada".to_string())];
        assert_eq!(variable_value("$Speaker", &values).as_deref(), Some("Ada"));
        assert_eq!(
            variable_value("variable_speaker", &values).as_deref(),
            Some("Ada")
        );
        assert_eq!(variable_value("missing", &values), None);
    }

    #[test]
    fn twelve_hour_clocks_follow_the_library_time_format() {
        let item = spec(StageItemKind::Clock);
        let context = StageContext {
            twelve_hour: true,
            ..StageContext::default()
        };
        assert_eq!(
            lines_for(&item, &[], None, "13:05:09", "05:00", &context),
            vec!["1:05:09 PM".to_string()]
        );
        let mut no_seconds = item.clone();
        no_seconds.clock.seconds = false;
        assert_eq!(
            lines_for(&no_seconds, &[], None, "00:05:09", "05:00", &context),
            vec!["12:05 AM".to_string()]
        );
    }
}
