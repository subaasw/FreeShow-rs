// SPDX-License-Identifier: GPL-3.0-only

use std::path::{Path, PathBuf};

use freeshow_data::ShowFile;
use freeshow_output::{
    Fit, Frame, OUTPUT_HEIGHT, OUTPUT_WIDTH, Raster, Rgba, SOURCE_HEIGHT, SOURCE_WIDTH, StyledItem,
    StyledLine, StyledSlide, TextRun, Blend, blend_mode, blur_frames, clear_frame, crossfade, decode_image, ease_progress, over, parse_color, placed_image,
    render_styled, scale_frames, slide_frames, slide_way, spin_frames, filter_blur, filter_brightness, filter_contrast, filter_grayscale, filter_hue_rotate, filter_invert, filter_opacity, filter_saturate, filter_sepia,
};
use crate::session::Session;

/// One step of a slide or media transition. `t` is the raw progress from 0 to 1.
pub fn motion_frame(
    from: &Frame,
    to: &Frame,
    motion: &freeshow_data::TextTransition,
    t: f32,
) -> Frame {
    if motion.kind == "none" {
        return to.clone();
    }
    let t = ease_progress(&motion.easing, t.clamp(0.0, 1.0));
    match motion.kind.as_str() {
        "slide" => slide_frames(from, to, t, slide_way(&motion.direction)),
        "scale" => scale_frames(from, to, t),
        "blur" => blur_frames(from, to, t),
        "spin" => spin_frames(from, to, t),
        _ => crossfade(from, to, t),
    }
}

/// The previous picture, held while the first video frame fades in.
pub struct OpeningFade {
    from: Frame,
    motion: freeshow_data::TextTransition,
}

impl OpeningFade {
    pub fn new(from: Frame, motion: freeshow_data::TextTransition) -> Self {
        Self { from, motion }
    }

    /// The blended frame, or nothing once the transition has finished.
    pub fn frame(&self, to: &Frame, elapsed_ms: u128) -> Option<Frame> {
        if self.motion.kind == "none" || elapsed_ms >= u128::from(self.motion.duration_ms) {
            return None;
        }
        let t = elapsed_ms as f32 / self.motion.duration_ms.max(1) as f32;
        Some(motion_frame(&self.from, to, &self.motion, t))
    }
}

pub fn audience_frame(session: &Session) -> Frame {
    let mut frame = if !session.blanked {
        if let Some(image) = &session.underlay {
            placed_image(
                image,
                OUTPUT_WIDTH,
                OUTPUT_HEIGHT,
                session.background_fit,
                session.background_flip_x,
                session.background_flip_y,
                &session.background_filter,
                &session.background_blend,
                session.background_blur_px,
                session.background_blur_opacity,
            )
        } else {
            clear_frame(OUTPUT_WIDTH, OUTPUT_HEIGHT)
        }
    } else {
        clear_frame(OUTPUT_WIDTH, OUTPUT_HEIGHT)
    };
    frame = paint_overlays(frame, session, true);
    if !session.blanked && !session.slide_hidden {
        let slide = if session.tab == "scripture" {
            lines_frame(&session.output_lines(), true)
        } else {
            show_frame(session, session.slide_index)
        };
        frame = over(&frame, &slide);
    }
    paint_overlays(frame, session, false)
}

fn paint_overlays(mut frame: Frame, session: &Session, under_slide: bool) -> Frame {
    for id in &session.active_overlays {
        let Some(overlay) = session.overlay_catalog.get(id) else {
            continue;
        };
        let place_under = overlay
            .get("placeUnderSlide")
            .and_then(|value| value.as_bool())
            == Some(true);
        if place_under != under_slide {
            continue;
        }
        let items = overlay
            .get("items")
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        styled_item(item, session.root.as_deref(), None, None, &session_variables(session))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let layer = render_styled(&StyledSlide {
            output_width: OUTPUT_WIDTH,
            output_height: OUTPUT_HEIGHT,
            source_width: SOURCE_WIDTH,
            source_height: SOURCE_HEIGHT,
            background: Rgba::TRANSPARENT,
            items,
        });
        frame = over(&frame, &layer);
    }
    frame
}

pub fn stage_frame(session: &Session) -> Frame {
    let variables = session_variables(session);
    let timers = session.timer_labels();
    let (slides, index) = if session.tab == "scripture" {
        let lines = session.output_lines();
        let index = if lines.is_empty() { None } else { Some(0) };
        (
            vec![crate::stage::StageSlide {
                lines,
                notes: String::new(),
            }],
            index,
        )
    } else {
        let page = session.style_page();
        let reveal = session.reveal_count;
        let clicks = session.click_revealed;
        let current_index = session.slide_index;
        let slides = session
            .show_index
            .and_then(|slot| session.shows.get(slot))
            .map(|show| {
                show.ordered_slides()
                    .into_iter()
                    .enumerate()
                    .map(|(slide_index, slide)| {
                        let current = current_index == Some(slide_index);
                        crate::stage::StageSlide {
                            lines: show
                                .stage_lines(slide_index, current, reveal, clicks, page)
                                .into_iter()
                                .map(|line| {
                                    freeshow_data::fill_variables(&line, &variables)
                                })
                                .collect(),
                            notes: freeshow_data::fill_variables(&slide.notes, &variables),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        (slides, session.slide_index)
    };
    crate::stage::paint_stage(
        &slides,
        index,
        &session.stage,
        &clock_label(),
        &session.timer_label(),
        &crate::stage::StageContext {
            timers: &timers,
            variables: &variables,
            root: session.root.as_deref(),
        },
    )
}

fn clock_label() -> String {
    freeshow_timecode::latest_clock().unwrap_or_else(local_clock)
}

fn local_clock() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

fn show_frame(session: &Session, index: Option<usize>) -> Frame {
    let Some(index) = index else {
        return clear_frame(OUTPUT_WIDTH, OUTPUT_HEIGHT);
    };
    let Some(show) = session.show_index.and_then(|slot| session.shows.get(slot)) else {
        return clear_frame(OUTPUT_WIDTH, OUTPUT_HEIGHT);
    };
    frame_for_slide(
        show,
        index,
        session.root.as_deref(),
        true,
        Some(session.reveal_count),
        session.click_revealed,
        session.style_page(),
        &session_variables(session),
    )
}

pub fn frame_for_slide(
    show: &ShowFile,
    index: usize,
    root: Option<&Path>,
    over_media: bool,
    reveal: Option<u32>,
    clicks_shown: bool,
    page: Option<(usize, usize)>,
    variables: &[(String, String)],
) -> Frame {
    let Some(slide_id) = show
        .ordered_slides()
        .get(index)
        .map(|slide| slide.id.clone())
    else {
        return clear_frame(OUTPUT_WIDTH, OUTPUT_HEIGHT);
    };
    let Some(slide) = show
        .show
        .get("slides")
        .and_then(|slides| slides.get(&slide_id))
    else {
        return clear_frame(OUTPUT_WIDTH, OUTPUT_HEIGHT);
    };
    let background = slide
        .pointer("/settings/color")
        .and_then(|value| value.as_str())
        .and_then(parse_color)
        .unwrap_or(if over_media {
            Rgba::TRANSPARENT
        } else {
            Rgba::BLACK
        });
    let source_width = slide
        .pointer("/settings/resolution/width")
        .and_then(|value| value.as_f64())
        .unwrap_or(f64::from(SOURCE_WIDTH)) as f32;
    let source_height = slide
        .pointer("/settings/resolution/height")
        .and_then(|value| value.as_f64())
        .unwrap_or(f64::from(SOURCE_HEIGHT)) as f32;
    let items = slide
        .get("items")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter(|item| {
                    clicks_shown
                        || item.get("clickReveal").and_then(|value| value.as_bool()) != Some(true)
                })
                .map(|item| styled_item(item, root, reveal, page, variables))
                .collect()
        })
        .unwrap_or_default();
    render_styled(&StyledSlide {
        output_width: OUTPUT_WIDTH,
        output_height: OUTPUT_HEIGHT,
        source_width,
        source_height,
        background,
        items,
    })
}

fn styled_item(
    item: &serde_json::Value,
    root: Option<&Path>,
    reveal: Option<u32>,
    page: Option<(usize, usize)>,
    variables: &[(String, String)],
) -> StyledItem {
    let chords_on = item.pointer("/chords/enabled").and_then(|value| value.as_bool()) == Some(true);
    let reveal_lines = item.get("lineReveal").and_then(|value| value.as_bool()) == Some(true);
    let lines = item
        .get("lines")
        .and_then(|value| value.as_array())
        .map(|lines| {
            lines
                .iter()
                .enumerate()
                .filter(|(index, _)| {
                    if reveal_lines {
                        reveal
                            .map(|count| *index < count as usize)
                            .unwrap_or(true)
                    } else if let Some((start, end)) = page {
                        *index >= start && *index < end
                    } else {
                        true
                    }
                })
                .map(|(_, line)| line)
                .flat_map(|line| {
                    let lyric = styled_line(line, variables);
                    let chord = chords_on.then(|| chord_line(item, line)).flatten();
                    chord.into_iter().chain(std::iter::once(lyric)).collect::<Vec<_>>()
                })
                .collect()
        })
        .unwrap_or_default();
    let qr = item.get("type").and_then(|value| value.as_str()) == Some("qr_code");
    let image = if qr {
        freeshow_data::QrItem::parse(item).and_then(|qr| {
            freeshow_output::qr_image(&qr.text, &qr.color, &qr.background)
        })
    } else {
        item.get("src")
            .and_then(|value| value.as_str())
            .and_then(|src| load_image(root, src))
    };
    StyledItem {
        style: item
            .get("style")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string(),
        align: item
            .get("align")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string(),
        lines: if qr { Vec::new() } else { lines },
        image,
        fit: if qr {
            Fit::Contain
        } else {
            Fit::named(item.get("fit").and_then(|value| value.as_str()).unwrap_or("contain"))
        },
        line_bg: item
            .pointer("/specialStyle/lineBg")
            .and_then(|value| value.as_str())
            .and_then(parse_color),
        line_gap: json_number(item.pointer("/specialStyle/lineGap")).unwrap_or(0.0),
        line_radius: json_number(item.pointer("/specialStyle/lineRadius")).unwrap_or(0.0),
        flip_x: item.get("flipped").and_then(|value| value.as_bool()) == Some(true),
        flip_y: item.get("flippedY").and_then(|value| value.as_bool()) == Some(true),
        opacity: filter_opacity(item.get("filter").and_then(|value| value.as_str()).unwrap_or("")),
        grayscale: filter_grayscale(
            item.get("filter").and_then(|value| value.as_str()).unwrap_or(""),
        ),
        invert: filter_invert(item.get("filter").and_then(|value| value.as_str()).unwrap_or("")),
        sepia: filter_sepia(item.get("filter").and_then(|value| value.as_str()).unwrap_or("")),
        brightness: filter_brightness(
            item.get("filter").and_then(|value| value.as_str()).unwrap_or(""),
        ),
        contrast: filter_contrast(item.get("filter").and_then(|value| value.as_str()).unwrap_or("")),
        saturate: filter_saturate(item.get("filter").and_then(|value| value.as_str()).unwrap_or("")),
        hue_rotate: filter_hue_rotate(
            item.get("filter").and_then(|value| value.as_str()).unwrap_or(""),
        ),
        blur: filter_blur(item.get("filter").and_then(|value| value.as_str()).unwrap_or("")),
        blend: blend_mode(item.get("blend").and_then(|value| value.as_str()).unwrap_or("normal")),
        crop: item_crop(item),
    }
}

fn item_crop(item: &serde_json::Value) -> freeshow_output::Crop {
    freeshow_output::Crop {
        top: json_number(item.pointer("/cropping/top")).unwrap_or(0.0),
        right: json_number(item.pointer("/cropping/right")).unwrap_or(0.0),
        bottom: json_number(item.pointer("/cropping/bottom")).unwrap_or(0.0),
        left: json_number(item.pointer("/cropping/left")).unwrap_or(0.0),
        clip: item.pointer("/cropping/type").and_then(|value| value.as_str()) != Some("ppt"),
    }
}

fn json_number(value: Option<&serde_json::Value>) -> Option<f32> {
    let value = value?;
    value
        .as_f64()
        .map(|number| number as f32)
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
}

fn session_variables(session: &Session) -> Vec<(String, String)> {
    session
        .root
        .as_deref()
        .map(|root| freeshow_data::Library::open(root).variables())
        .unwrap_or_default()
}

fn styled_line(line: &serde_json::Value, variables: &[(String, String)]) -> StyledLine {
    StyledLine {
        align: line
            .get("align")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string(),
        line_bar: true,
        wrapped: false,
        shift_y: 0,
        runs: line
            .get("text")
            .and_then(|value| value.as_array())
            .map(|runs| {
                runs.iter()
                    .map(|run| TextRun {
                        text: freeshow_data::fill_variables(
                            run.get("value").and_then(|value| value.as_str()).unwrap_or(""),
                            variables,
                        ),
                        style: run
                            .get("style")
                            .and_then(|value| value.as_str())
                            .unwrap_or("")
                            .to_string(),
                        place_on: None,
                        paint: true,
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// A chord sits on the letter at `pos`. Its glyphs use the item chord size, which defaults to half the lyric.
fn chord_line(item: &serde_json::Value, line: &serde_json::Value) -> Option<StyledLine> {
    let chords = chord_marks(line)?;
    let lyric = line_text(line);
    let lyric_style = line
        .pointer("/text/0/style")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let size = font_size_px(lyric_style).unwrap_or(100);
    let percent = item
        .pointer("/chords/size")
        .and_then(|value| value.as_f64())
        .filter(|size| *size > 0.0)
        .unwrap_or(50.0);
    let chord_size = ((size as f64) * percent / 100.0).round().max(1.0) as u32;
    let color = item
        .pointer("/chords/color")
        .and_then(|value| value.as_str())
        .unwrap_or("#FF851B");
    let mut runs = vec![TextRun {
        text: lyric.clone(),
        style: if lyric_style.is_empty() {
            format!("font-size:{size}px;")
        } else {
            lyric_style.to_string()
        },
        place_on: None,
        paint: false,
    }];
    for (pos, key) in chords {
        let prefix: String = lyric.chars().take(pos).collect();
        runs.push(TextRun {
            text: key,
            style: format!("font-size:{chord_size}px;color:{color};"),
            place_on: Some(prefix),
            paint: true,
        });
    }
    Some(StyledLine {
        align: line
            .get("align")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string(),
        line_bar: false,
        wrapped: false,
        shift_y: json_number(item.pointer("/chords/offsetY"))
            .unwrap_or(0.0)
            .round() as i32,
        runs,
    })
}

fn line_text(line: &serde_json::Value) -> String {
    line.get("text")
        .and_then(|value| value.as_array())
        .map(|runs| {
            runs.iter()
                .filter_map(|run| run.get("value").and_then(|value| value.as_str()))
                .collect()
        })
        .unwrap_or_default()
}

fn chord_marks(line: &serde_json::Value) -> Option<Vec<(usize, String)>> {
    let mut chords: Vec<(usize, String)> = line
        .get("chords")
        .and_then(|value| value.as_array())?
        .iter()
        .filter_map(|chord| {
            let key = chord.get("key").and_then(|value| value.as_str())?;
            if key.is_empty() {
                return None;
            }
            let pos = chord.get("pos").and_then(|value| value.as_u64()).unwrap_or(0) as usize;
            Some((pos, key.to_string()))
        })
        .collect();
    if chords.is_empty() {
        return None;
    }
    chords.sort_by_key(|(pos, _)| *pos);
    Some(chords)
}

fn font_size_px(style: &str) -> Option<u32> {
    style.split(';').find_map(|part| {
        let part = part.trim().strip_prefix("font-size:")?;
        part.trim().trim_end_matches("px").trim().parse().ok()
    })
}

pub(crate) fn load_image(root: Option<&Path>, src: &str) -> Option<Raster> {
    let path = resolve_media(root, src)?;
    let bytes = std::fs::read(path).ok()?;
    decode_image(&bytes)
}

fn resolve_media(root: Option<&Path>, src: &str) -> Option<PathBuf> {
    let path = PathBuf::from(src);
    if path.is_file() {
        return Some(path);
    }
    let root = root?;
    let joined = root.join(src);
    joined.is_file().then_some(joined)
}

fn lines_frame(lines: &[String], over_media: bool) -> Frame {
    if lines.is_empty() {
        return if over_media {
            render_styled(&StyledSlide {
                output_width: OUTPUT_WIDTH,
                output_height: OUTPUT_HEIGHT,
                source_width: SOURCE_WIDTH,
                source_height: SOURCE_HEIGHT,
                background: Rgba::TRANSPARENT,
                items: Vec::new(),
            })
        } else {
            clear_frame(OUTPUT_WIDTH, OUTPUT_HEIGHT)
        };
    }
    render_styled(&StyledSlide {
        output_width: OUTPUT_WIDTH,
        output_height: OUTPUT_HEIGHT,
        source_width: SOURCE_WIDTH,
        source_height: SOURCE_HEIGHT,
        background: if over_media {
            Rgba::TRANSPARENT
        } else {
            Rgba::BLACK
        },
        items: vec![StyledItem {
            style: "top:88px;left:50px;height:904px;width:1820px;".into(),
            align: String::new(),
            lines: lines
                .iter()
                .map(|line| StyledLine {
                    align: String::new(),
                    line_bar: true,
                    wrapped: false,
                    shift_y: 0,
                    runs: vec![TextRun {
                        text: line.clone(),
                        style: "font-size: 72px;color: #FFFFFF;".into(),
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
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use freeshow_data::Catalog;

    #[test]
    fn stored_qr_items_render_the_same_square_on_audience_and_stage() {
        let item = serde_json::json!({
            "type": "qr_code",
            "style": "left:0px;top:0px;width:320px;height:160px;",
            "qr_code": {"text":"https://example.org", "color":"#c01054", "background":"#fff"}
        });
        let mut show = ShowFile::blank("qr", "QR");
        show.show["slides"]["s1"]["items"] = serde_json::json!([item.clone()]);
        let audience = frame_for_slide(&show, 0, None, false, None, true, None, &[]);

        let temporary = std::env::temp_dir().join(freeshow_data::new_id());
        let library = freeshow_data::Library::open(&temporary);
        let stage_path = library.stage_path();
        std::fs::create_dir_all(stage_path.parent().unwrap()).unwrap();
        std::fs::write(
            stage_path,
            serde_json::json!({"layout":{"items":{"code": item}}}).to_string(),
        )
        .unwrap();
        let mut session = Session::empty();
        session.stage = library.load_stage_layout();
        std::fs::remove_dir_all(temporary).unwrap();
        let stage = stage_frame(&session);

        let foreground = parse_color("#c01054").unwrap();
        let quiet_zone = parse_color("#fff").unwrap();
        for frame in [&audience, &stage] {
            assert_eq!(frame.pixel(40, 0), quiet_zone);
            assert_eq!(frame.pixel(119, 79), quiet_zone);
            assert_ne!(frame.pixel(39, 0), quiet_zone);
            assert_ne!(frame.pixel(120, 79), quiet_zone);
            assert_eq!(frame.pixel(52, 12), foreground);
        }
        for y in 0..80 {
            for x in 40..120 {
                assert_eq!(audience.pixel(x, y), stage.pixel(x, y));
            }
        }
    }

    #[test]
    fn the_first_video_frame_fades_in_from_the_previous_picture() {
        let from = Frame {
            width: 1,
            height: 1,
            pixels: vec![255, 0, 0, 255],
        };
        let to = Frame {
            width: 1,
            height: 1,
            pixels: vec![0, 0, 255, 255],
        };
        let fade = OpeningFade::new(
            from,
            freeshow_data::TextTransition {
                duration_ms: 400,
                ..freeshow_data::TextTransition::default()
            },
        );
        let opening = fade.frame(&to, 0).expect("the fade is still running");
        assert!(opening.pixel(0, 0).r > 200);
        assert!(opening.pixel(0, 0).b < 40);
        assert!(fade.frame(&to, 400).is_none());
    }

    #[test]
    fn a_hidden_slide_leaves_the_picture_visible() {
        let mut session = Session::empty();
        let mut show = ShowFile::blank("song", "Song");
        show.show
            .pointer_mut("/slides/s1/settings")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("color".into(), serde_json::json!("#FFFFFF"));
        session.shows.push(show);
        session.show_index = Some(0);
        session.slide_index = Some(0);
        session.underlay = Some(Raster {
            width: 4,
            height: 4,
            rgba: vec![255, 0, 0, 255].repeat(16),
        });
        let covered = audience_frame(&session);
        assert!(
            covered
                .pixels
                .chunks(4)
                .all(|pixel| !(pixel[0] > 200 && pixel[1] < 40 && pixel[2] < 40)),
            "the song covers the picture"
        );
        session.slide_hidden = true;
        let picture = audience_frame(&session);
        assert!(
            picture
                .pixels
                .chunks(4)
                .any(|pixel| pixel[0] > 200 && pixel[1] < 40 && pixel[2] < 40),
            "the picture stays when the song is hidden"
        );
    }

    #[test]
    fn an_overlay_is_painted_on_a_cleared_screen() {
        let mut session = Session::empty();
        session.blanked = true;
        session.overlay_catalog = Catalog::parse(
            r#"{"logo":{"name":"Logo","items":[{"style":"left:0px;top:0px;width:1800px;height:300px;","lines":[{"text":[{"value":"LIVE","style":"font-size:120px;color:#FFFFFF;"}]}]}]}}"#,
        )
        .unwrap();
        session.active_overlays.push("logo".into());
        let frame = audience_frame(&session);
        assert!(
            frame.pixels.chunks(4).any(|pixel| pixel[0] > 200),
            "overlay text should paint a light pixel"
        );
        session.active_overlays.clear();
        let cleared = audience_frame(&session);
        assert!(cleared.pixels.chunks(4).all(|pixel| pixel[0] == 0));
    }

    #[test]
    fn an_overlay_marked_under_the_slide_is_covered_by_the_slide() {
        let mut session = Session::empty();
        let mut show = ShowFile::blank("song", "Song");
        show.show
            .pointer_mut("/slides/s1/settings")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("color".into(), serde_json::json!("#FFFFFF"));
        session.shows.push(show);
        session.show_index = Some(0);
        session.slide_index = Some(0);
        session.overlay_catalog = Catalog::parse(
            r#"{"back":{"name":"Back","placeUnderSlide":true,"items":[{"style":"left:0px;top:0px;width:1800px;height:300px;","lines":[{"text":[{"value":"UNDER","style":"font-size:120px;color:#FF0000;"}]}]}]}}"#,
        )
        .unwrap();
        session.active_overlays.push("back".into());
        let behind = audience_frame(&session);
        assert!(
            !behind
                .pixels
                .chunks(4)
                .any(|pixel| pixel[0] > 200 && pixel[1] < 40),
            "a white slide covers an overlay placed under it"
        );

        session.overlay_catalog = Catalog::parse(
            r#"{"front":{"name":"Front","items":[{"style":"left:0px;top:0px;width:1800px;height:300px;","lines":[{"text":[{"value":"OVER","style":"font-size:120px;color:#FF0000;"}]}]}]}}"#,
        )
        .unwrap();
        session.active_overlays = vec!["front".into()];
        let above = audience_frame(&session);
        assert!(
            above
                .pixels
                .chunks(4)
                .any(|pixel| pixel[0] > 200 && pixel[1] < 40),
            "an overlay without placeUnderSlide paints on top of the slide"
        );
    }

    #[test]
    fn a_line_background_and_gap_come_from_the_item() {
        let mut show = ShowFile::blank("song", "Song");
        *show.show.pointer_mut("/slides/s1/items/0").unwrap() = serde_json::json!({
            "style": "left:0px;top:0px;width:1920px;height:1080px;",
            "specialStyle": { "lineBg": "#FF0000", "lineGap": 80, "lineRadius": 0 },
            "lines": [
                { "text": [{ "value": "A", "style": "font-size:40px;color:#FFFFFF;" }] },
                { "text": [{ "value": "B", "style": "font-size:40px;color:#FFFFFF;" }] }
            ]
        });
        let frame = frame_for_slide(&show, 0, None, true, None, true, None, &[]);
        let red_rows: Vec<u32> = (0..frame.height)
            .filter(|y| {
                let pixel = frame.pixel(0, *y);
                pixel.r > 200 && pixel.g < 40 && pixel.a > 200
            })
            .collect();
        let first = red_rows[0];
        let last = *red_rows.last().unwrap();
        assert!(
            (first..=last).any(|y| !red_rows.contains(&y)),
            "the line gap separates the two lyric bars"
        );
    }

    #[test]
    fn a_chord_is_painted_above_its_lyric() {
        let mut show = ShowFile::blank("song", "Song");
        *show.show.pointer_mut("/slides/s1/items/0").unwrap() = serde_json::json!({
            "style": "left:40px;top:40px;width:1800px;height:900px;",
            "chords": { "enabled": true, "color": "#FF851B", "size": 50 },
            "lines": [{
                "text": [{ "value": "Amazing grace", "style": "font-size:80px;color:#FFFFFF;" }],
                "chords": [{ "id": "c1", "pos": 0, "key": "G" }]
            }]
        });
        let frame = frame_for_slide(&show, 0, None, true, None, true, None, &[]);
        assert!(
            frame.pixels.chunks(4).any(|pixel| {
                pixel[0] > 200 && pixel[1] > 80 && pixel[1] < 200 && pixel[2] < 80
            }),
            "the chord uses the orange chord color"
        );
        *show
            .show
            .pointer_mut("/slides/s1/items/0/chords/enabled")
            .unwrap() = serde_json::json!(false);
        let hidden = frame_for_slide(&show, 0, None, true, None, true, None, &[]);
        assert!(
            !hidden.pixels.chunks(4).any(|pixel| {
                pixel[0] > 200 && pixel[1] > 80 && pixel[1] < 200 && pixel[2] < 80
            }),
            "chords stay off until the item enables them"
        );
        *show.show.pointer_mut("/slides/s1/items/0/chords/enabled").unwrap() =
            serde_json::json!(true);
        *show.show.pointer_mut("/slides/s1/items/0/chords/size").unwrap() =
            serde_json::json!(100);
        let full = frame_for_slide(&show, 0, None, true, None, true, None, &[]);
        assert!(
            orange_rows(&frame) < orange_rows(&full),
            "the default chord size is smaller than the lyric"
        );
    }

    #[test]
    fn chord_offset_lifts_the_chord() {
        let mut show = ShowFile::blank("song", "Song");
        *show.show.pointer_mut("/slides/s1/items/0").unwrap() = serde_json::json!({
            "style": "left:40px;top:200px;width:1800px;height:700px;",
            "chords": { "enabled": true, "color": "#FF851B", "size": 80, "offsetY": 0 },
            "lines": [{
                "text": [{ "value": "Amazing grace", "style": "font-size:80px;color:#FFFFFF;" }],
                "chords": [{ "id": "c1", "pos": 0, "key": "G" }]
            }]
        });
        let resting = frame_for_slide(&show, 0, None, true, None, true, None, &[]);
        *show.show.pointer_mut("/slides/s1/items/0/chords/offsetY").unwrap() =
            serde_json::json!(80);
        let lifted = frame_for_slide(&show, 0, None, true, None, true, None, &[]);
        assert!(top_orange(&lifted) + 20 < top_orange(&resting));
    }

    fn top_orange(frame: &Frame) -> u32 {
        for y in 0..frame.height {
            for x in 0..frame.width {
                let pixel = frame.pixel(x, y);
                if pixel.r > 200 && pixel.g > 80 && pixel.g < 200 && pixel.b < 80 {
                    return y;
                }
            }
        }
        frame.height
    }

    fn orange_rows(frame: &Frame) -> usize {
        frame
            .pixels
            .chunks(frame.width as usize * 4)
            .filter(|row| {
                row.chunks(4).any(|pixel| {
                    pixel[0] > 200 && pixel[1] > 80 && pixel[1] < 200 && pixel[2] < 80
                })
            })
            .count()
    }

    #[test]
    fn an_item_picture_uses_the_fit_stored_on_the_item() {
        let root = std::env::temp_dir().join(format!("freeshow-rust-fit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let png = [
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
            0x52, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x76,
            0x5e, 0x98, 0x9a, 0x00, 0x00, 0x00, 0x10, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8,
            0xcf, 0xc0, 0x00, 0x44, 0x60, 0xe2, 0x3f, 0x00, 0x19, 0xf3, 0x03, 0xfd, 0x9a, 0xd8, 0x73,
            0x31, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];
        std::fs::write(root.join("wide.png"), png).unwrap();
        let mut show = ShowFile::blank("song", "Song");
        let item = show
            .show
            .pointer_mut("/slides/s1/items/0")
            .unwrap();
        item.as_object_mut().unwrap().insert(
            "style".into(),
            serde_json::json!("left:0px;top:0px;width:1920px;height:1080px;"),
        );
        item.as_object_mut().unwrap().insert(
            "src".into(),
            serde_json::json!(root.join("wide.png").to_string_lossy()),
        );
        item.as_object_mut()
            .unwrap()
            .insert("fit".into(), serde_json::json!("contain"));
        let contained = frame_for_slide(&show, 0, None, true, None, true, None, &[]);
        assert_eq!(contained.pixel(0, 0), Rgba::TRANSPARENT);

        *show
            .show
            .pointer_mut("/slides/s1/items/0/fit")
            .unwrap() = serde_json::json!("fill");
        let filled = frame_for_slide(&show, 0, None, true, None, true, None, &[]);
        assert_eq!(filled.pixel(0, 0).r, 255);
        assert_eq!(filled.pixel(0, 0).g, 0);
        let _ = std::fs::remove_dir_all(&root);
    }
}
