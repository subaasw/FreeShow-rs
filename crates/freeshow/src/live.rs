// SPDX-License-Identifier: GPL-3.0-only

use std::io::Read;
use std::path::{Path, PathBuf};

use crate::session::Session;
use freeshow_data::ShowFile;
use freeshow_output::render_styled;
use freeshow_output::{
    Blend, Fit, Frame, Raster, Rgba, SOURCE_HEIGHT, SOURCE_WIDTH, StyledItem, StyledLine,
    StyledSlide, TextRun, blend_mode, blur_frames, clear_frame, composite_styled, crossfade,
    decode_image, ease_progress, filter_blur, filter_brightness, filter_contrast, filter_grayscale,
    filter_hue_rotate, filter_invert, filter_opacity, filter_saturate, filter_sepia, parse_color,
    placed_image, scale_frames, slide_frames, slide_way, spin_frames,
};
#[cfg(test)]
use freeshow_output::{OUTPUT_HEIGHT, OUTPUT_WIDTH};

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

/// The content layer of the last audience frame, independent of the cue being run.
#[derive(Clone)]
pub(crate) enum ProgramContent {
    Scripture(Vec<String>),
    Slide {
        show_id: String,
        slide_id: String,
        /// Position in the show's layout, so the slide list can mark what is live.
        index: usize,
        reveal: u32,
        clicks: bool,
        page: Option<(usize, usize)>,
    },
}

/// One slide of `show` drawn small for the slide list.
pub fn slide_thumbnail(
    session: &Session,
    show: &ShowFile,
    index: usize,
    width: u32,
    height: u32,
) -> Frame {
    frame_for_slide_at(
        show,
        index,
        session.root.as_deref(),
        false,
        None,
        true,
        None,
        &session_variables(session),
        width,
        height,
    )
}

/// An overlay or template drawn small for the drawer, over its own background colour or black.
pub fn catalog_thumbnail(
    session: &Session,
    entry: &serde_json::Value,
    width: u32,
    height: u32,
) -> Frame {
    let background = entry["settings"]["backgroundColor"]
        .as_str()
        .and_then(freeshow_output::parse_color)
        .unwrap_or(Rgba::BLACK);
    render_styled(&StyledSlide {
        output_width: width,
        output_height: height,
        source_width: SOURCE_WIDTH,
        source_height: SOURCE_HEIGHT,
        background,
        items: catalog_items(session, entry),
    })
}

fn catalog_items(session: &Session, entry: &serde_json::Value) -> Vec<StyledItem> {
    let variables = session_variables(session);
    entry
        .get("items")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .map(|item| styled_item(item, session.root.as_deref(), None, None, &variables))
                .collect()
        })
        .unwrap_or_default()
}

/// The selected slide on its own, for the editor preview. Unlike [`audience_frame`]
/// it ignores Clear and does not touch what the output records as live.
pub fn edit_frame(session: &Session) -> Frame {
    let (width, height) = (session.output_width, session.output_height);
    let Some((show, index)) = session
        .show_index
        .and_then(|slot| session.shows.get(slot))
        .zip(session.slide_index)
    else {
        return clear_frame(width, height);
    };
    frame_for_slide_at(
        show,
        index,
        session.root.as_deref(),
        false,
        None,
        true,
        None,
        &session_variables(session),
        width,
        height,
    )
}

pub fn audience_frame(session: &Session) -> Frame {
    let mut frame = if !session.blanked {
        if let Some(image) = &session.underlay {
            placed_image(
                image,
                session.output_width,
                session.output_height,
                session.background_fit,
                session.background_flip_x,
                session.background_flip_y,
                &session.background_filter,
                &session.background_blend,
                session.background_blur_px,
                session.background_blur_opacity,
            )
        } else {
            style_blank(session)
        }
    } else {
        style_blank(session)
    };
    paint_overlays(&mut frame, session, true);
    paint_program_content(&mut frame, session);
    paint_overlays(&mut frame, session, false);
    paint_interaction(&mut frame, session);
    paint_drawing(&mut frame, session);
    paint_effects(&mut frame, session);
    delayed_audience_frame(frame, session.preferences.output_delay_ms())
}

/// An empty audience picture in the active output style's background colour.
fn style_blank(session: &Session) -> Frame {
    match session.style_background {
        Some(color) => render_styled(&StyledSlide {
            output_width: session.output_width,
            output_height: session.output_height,
            source_width: SOURCE_WIDTH,
            source_height: SOURCE_HEIGHT,
            background: color,
            items: Vec::new(),
        }),
        None => clear_frame(session.output_width, session.output_height),
    }
}

fn paint_program_content(frame: &mut Frame, session: &Session) {
    let mut program = session.program_content.borrow_mut();
    if session.blanked || session.slide_hidden {
        *program = None;
        return;
    }
    let (width, height) = (session.output_width, session.output_height);
    if session.program_held {
        // The operator is browsing: repaint what is live, with any edits made to it since.
        match program.clone() {
            Some(ProgramContent::Scripture(lines)) => {
                composite_styled(frame, &lines_slide_at(&lines, true, width, height));
            }
            Some(ProgramContent::Slide {
                show_id,
                slide_id,
                reveal,
                clicks,
                page,
                ..
            }) => {
                let live = session
                    .shows
                    .iter()
                    .find(|show| show.id == show_id)
                    .and_then(|show| show.show.get("slides")?.get(&slide_id));
                if let Some(slide) = live {
                    composite_styled(
                        frame,
                        &styled_slide(
                            &output_look(session, slide),
                            session.root.as_deref(),
                            true,
                            Some(reveal),
                            clicks,
                            page,
                            &session_variables(session),
                            width,
                            height,
                        ),
                    );
                } else {
                    *program = None;
                }
            }
            None => {}
        }
        return;
    }
    if session.tab == "scripture" {
        let lines = session.output_lines();
        composite_styled(frame, &lines_slide_at(&lines, true, width, height));
        *program = Some(ProgramContent::Scripture(lines));
        return;
    }
    let selected = session
        .show_index
        .and_then(|slot| session.shows.get(slot))
        .and_then(|show| {
            let slide_id = show.ordered_slides().get(session.slide_index?)?.id.clone();
            let slide = show.show.get("slides")?.get(&slide_id)?;
            Some((show, slide_id, slide))
        });
    let Some((show, slide_id, slide)) = selected else {
        *program = None;
        return;
    };
    let shown = if action_only(slide) {
        match program.as_ref() {
            Some(ProgramContent::Scripture(lines)) => {
                composite_styled(frame, &lines_slide_at(lines, true, width, height));
                return;
            }
            Some(ProgramContent::Slide {
                show_id, slide_id, ..
            }) => session
                .shows
                .iter()
                .find(|show| &show.id == show_id)
                .and_then(|show| show.show.get("slides")?.get(slide_id)),
            None => None,
        }
    } else {
        *program = Some(ProgramContent::Slide {
            show_id: show.id.clone(),
            slide_id,
            index: session.slide_index.unwrap_or_default(),
            reveal: session.reveal_count,
            clicks: session.click_revealed,
            page: session.style_page(),
        });
        Some(slide)
    };
    if let (
        Some(slide),
        Some(ProgramContent::Slide {
            reveal,
            clicks,
            page,
            ..
        }),
    ) = (shown, program.as_ref())
    {
        composite_styled(
            frame,
            &styled_slide(
                &output_look(session, slide),
                session.root.as_deref(),
                true,
                Some(*reveal),
                *clicks,
                *page,
                &session_variables(session),
                width,
                height,
            ),
        );
    }
}

/// The slide as the output shows it: restyled by the output style's template when it has one.
fn output_look<'a>(
    session: &Session,
    slide: &'a serde_json::Value,
) -> std::borrow::Cow<'a, serde_json::Value> {
    let template = session
        .style_template
        .as_deref()
        .and_then(|id| session.template_catalog.get(id));
    match template {
        Some(template) => {
            let mut restyled = slide.clone();
            if freeshow_data::restyle_with_template(&mut restyled, template) {
                std::borrow::Cow::Owned(restyled)
            } else {
                std::borrow::Cow::Borrowed(slide)
            }
        }
        None => std::borrow::Cow::Borrowed(slide),
    }
}

fn action_only(slide: &serde_json::Value) -> bool {
    slide
        .pointer("/settings/actionOnly")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
}

thread_local! {
    static OUTPUT_DELAY: std::cell::RefCell<OutputDelay> = const { std::cell::RefCell::new(OutputDelay::new()) };
}

/// A short frame buffer used when `special.outputDelayMs` is greater than zero.
pub struct OutputDelay {
    frames: std::collections::VecDeque<(u128, Frame)>,
}

impl OutputDelay {
    pub const fn new() -> Self {
        Self {
            frames: std::collections::VecDeque::new(),
        }
    }

    /// Returns the frame that is `delay_ms` old. A delay of 0 is a no-op.
    pub fn push(&mut self, frame: Frame, delay_ms: u32, now_ms: u128) -> Frame {
        if delay_ms == 0 {
            self.frames.clear();
            return frame;
        }
        let width = frame.width;
        let height = frame.height;
        self.frames.push_back((now_ms, frame));
        let Some(due) = now_ms.checked_sub(u128::from(delay_ms)) else {
            return clear_frame(width, height);
        };
        let mut released = None;
        while self.frames.front().is_some_and(|(time, _)| *time <= due) {
            released = self.frames.pop_front();
        }
        released
            .map(|(_, frame)| frame)
            .unwrap_or_else(|| clear_frame(width, height))
    }
}

fn delayed_audience_frame(frame: Frame, delay_ms: u32) -> Frame {
    if delay_ms == 0 {
        OUTPUT_DELAY.with(|buffer| buffer.borrow_mut().frames.clear());
        return frame;
    }
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    OUTPUT_DELAY.with(|buffer| buffer.borrow_mut().push(frame, delay_ms, now_ms))
}

fn paint_effects(frame: &mut Frame, session: &Session) {
    for effect in session.live_effects() {
        let color = parse_color(&effect.color)
            .or_else(|| parse_color(&effect.background))
            .unwrap_or(Rgba {
                r: 255,
                g: 255,
                b: 255,
                a: 40,
            });
        let wash = Rgba {
            r: color.r,
            g: color.g,
            b: color.b,
            a: color.a.clamp(24, 48),
        };
        // A light full-frame wash, plus a few deterministic dots for particle types.
        let step_x = (frame.width / 8).max(1);
        let step_y = (frame.height / 6).max(1);
        let particles = effect
            .item_types
            .iter()
            .any(|kind| matches!(kind.as_str(), "stars" | "snow" | "bubbles" | "confetti"));
        if particles {
            let mut y = step_y;
            while y < frame.height {
                let mut x = step_x;
                while x < frame.width {
                    frame.fill_circle(x as i32, y as i32, 3, color);
                    x += step_x;
                }
                y += step_y;
            }
        } else if !effect.color.is_empty() || !effect.background.is_empty() {
            let cx = (frame.width / 2) as i32;
            let cy = (frame.height / 2) as i32;
            frame.fill_circle(cx, cy, (frame.height / 6).max(8) as i32, wash);
        }
    }
}

fn paint_interaction(frame: &mut Frame, session: &Session) {
    let lines = session.interaction_lines();
    if lines.is_empty() {
        return;
    }
    composite_styled(
        frame,
        &lines_slide_at(&lines, true, session.output_width, session.output_height),
    );
}

fn paint_drawing(frame: &mut Frame, session: &Session) {
    let width = session.output_width as f32;
    let height = session.output_height as f32;
    let default_color = session.preferences.draw_color();
    let default_size = session.preferences.draw_size();
    for stroke in &session.draw_strokes {
        if stroke.up {
            continue;
        }
        let (x, y) = stroke_point(stroke.x, stroke.y, width, height);
        let size = if stroke.size > 0.0 {
            stroke.size
        } else {
            default_size
        };
        let radius = ((size * width / SOURCE_WIDTH) / 2.0).round() as i32;
        let color = parse_color(if stroke.color.is_empty() {
            &default_color
        } else {
            &stroke.color
        })
        .unwrap_or(Rgba::WHITE);
        frame.fill_circle(x, y, radius.max(1), color);
    }
}

fn stroke_point(x: f32, y: f32, width: f32, height: f32) -> (i32, i32) {
    if (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y) {
        return ((x * width).round() as i32, (y * height).round() as i32);
    }
    (
        (x * width / SOURCE_WIDTH).round() as i32,
        (y * height / SOURCE_HEIGHT).round() as i32,
    )
}

fn paint_overlays(frame: &mut Frame, session: &Session, under_slide: bool) {
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
        let items = catalog_items(session, overlay);
        composite_styled(
            frame,
            &StyledSlide {
                output_width: session.output_width,
                output_height: session.output_height,
                source_width: SOURCE_WIDTH,
                source_height: SOURCE_HEIGHT,
                background: Rgba::TRANSPARENT,
                items,
            },
        );
    }
}

pub fn stage_frame(session: &Session) -> Frame {
    let variables = session_variables(session);
    let timers = session.timer_labels();
    let audience = session
        .stage
        .items
        .iter()
        .any(|item| item.enabled && item.kind == freeshow_data::StageItemKind::CurrentOutput)
        .then(|| {
            let (id, name) = session.audience();
            let frame = audience_frame(session);
            let image = Raster {
                width: frame.width,
                height: frame.height,
                rgba: frame.pixels,
            };
            (id, name, image)
        });
    // While the operator browses or edits, the stage follows what the audience sees, not the selection.
    let held = if session.program_held {
        Some(session.program_content.borrow().clone())
    } else {
        None
    };
    let held_slide = match &held {
        Some(Some(ProgramContent::Slide {
            show_id,
            index,
            reveal,
            clicks,
            page,
            ..
        })) => session
            .shows
            .iter()
            .position(|show| &show.id == show_id)
            .map(|slot| (Some(slot), Some(*index), *reveal, *clicks, *page)),
        _ => None,
    };
    let scripture_lines = match &held {
        Some(Some(ProgramContent::Scripture(lines))) => Some(lines.clone()),
        Some(_) => None,
        None => (session.tab == "scripture").then(|| session.output_lines()),
    }
    .map(|lines| {
        if session.blanked || session.slide_hidden {
            Vec::new()
        } else {
            lines
        }
    });
    let (show_slot, current_index, reveal, clicks, page) = held_slide.unwrap_or((
        session.show_index,
        // Nothing is on the output, as after Clear or a foreground video.
        if matches!(held, Some(None)) || session.blanked || session.slide_hidden {
            None
        } else {
            session.slide_index
        },
        session.reveal_count,
        session.click_revealed,
        session.style_page(),
    ));
    let showing_show = scripture_lines.is_none();
    let (slides, index) = if let Some(lines) = scripture_lines {
        let index = if lines.is_empty() { None } else { Some(0) };
        (
            vec![crate::stage::StageSlide {
                text_boxes: vec![lines],
                notes: String::new(),
            }],
            index,
        )
    } else {
        let slides = show_slot
            .and_then(|slot| session.shows.get(slot))
            .map(|show| {
                show.ordered_slides()
                    .into_iter()
                    .enumerate()
                    .map(|(slide_index, slide)| {
                        let current = current_index == Some(slide_index);
                        if current && action_only(&show.show["slides"][&slide.id]) {
                            program_stage_slide(session, &variables)
                        } else {
                            stage_slide(
                                show.stage_text_boxes_for_slide(
                                    &slide.id, current, reveal, clicks, page,
                                ),
                                &slide.notes,
                                &variables,
                            )
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        (slides, current_index)
    };
    let root = session.root.as_deref();
    let mut still_backgrounds = std::collections::HashMap::new();
    if showing_show
        && let Some(index) = index
        && let Some(show) = show_slot.and_then(|slot| session.shows.get(slot))
    {
        for item in &session.stage.items {
            if !item.enabled {
                continue;
            }
            if item.kind != freeshow_data::StageItemKind::Slide
                || !item.include_media
                || item.offset == 0
            {
                continue;
            }
            let Some(target) = i64::try_from(index)
                .ok()
                .and_then(|index| index.checked_add(item.offset))
                .and_then(|index| usize::try_from(index).ok())
            else {
                continue;
            };
            if let freeshow_data::SlideBackdrop::Picture { path, .. } = show.backdrop_at(target)
                && freeshow_media::media_kind(Path::new(&path)) == freeshow_media::MediaKind::Image
            {
                still_backgrounds.insert(item.offset, path);
            }
        }
    }
    let sources = session
        .stage
        .items
        .iter()
        .filter(|item| {
            item.enabled
                && matches!(
                    item.kind,
                    freeshow_data::StageItemKind::Media | freeshow_data::StageItemKind::Web
                )
        })
        .filter_map(|item| item.src.as_deref())
        .chain(still_backgrounds.values().map(String::as_str));
    let mut media = session.stage_media.borrow_mut();
    media.update(root, sources);
    media.update_cameras(
        session
            .stage
            .items
            .iter()
            .filter(|item| item.enabled && item.kind == freeshow_data::StageItemKind::Camera)
            .filter_map(|item| item.device_name.as_deref()),
    );
    let (width, height) = session.stage_output_size();
    crate::stage::paint_stage_at(
        &slides,
        index,
        &session.stage,
        &clock_label(),
        &session.timer_label(),
        &crate::stage::StageContext {
            timers: &timers,
            variables: &variables,
            media: Some(&media),
            audience: audience
                .as_ref()
                .map(|(id, name, image)| (id.as_str(), name.as_str(), image)),
            current_background: session.underlay.as_ref().filter(|_| !session.blanked).map(
                |image| {
                    (
                        image,
                        session.background_fit,
                        session.background_flip_x,
                        session.background_flip_y,
                    )
                },
            ),
            still_backgrounds: Some(&still_backgrounds),
            twelve_hour: session.preferences.uses_12_hour_clock(),
        },
        width,
        height,
    )
}

fn program_stage_slide(
    session: &Session,
    variables: &[(String, String)],
) -> crate::stage::StageSlide {
    match session.program_content.borrow().as_ref() {
        Some(ProgramContent::Scripture(lines)) => stage_slide(vec![lines.clone()], "", variables),
        Some(ProgramContent::Slide {
            show_id,
            slide_id,
            reveal,
            clicks,
            page,
            ..
        }) => {
            if let Some(show) = session.shows.iter().find(|show| &show.id == show_id) {
                let notes = show.show["slides"][slide_id]["notes"]
                    .as_str()
                    .unwrap_or_default();
                return stage_slide(
                    show.stage_text_boxes_for_slide(slide_id, true, *reveal, *clicks, *page),
                    notes,
                    variables,
                );
            }
            stage_slide(Vec::new(), "", variables)
        }
        None => stage_slide(Vec::new(), "", variables),
    }
}

fn stage_slide(
    mut text_boxes: Vec<Vec<String>>,
    notes: &str,
    variables: &[(String, String)],
) -> crate::stage::StageSlide {
    for line in text_boxes.iter_mut().flatten() {
        *line = freeshow_data::fill_variables(line, variables);
    }
    crate::stage::StageSlide {
        text_boxes,
        notes: freeshow_data::fill_variables(notes, variables),
    }
}

fn clock_label() -> String {
    freeshow_timecode::latest_clock().unwrap_or_else(local_clock)
}

fn local_clock() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
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
    frame_for_slide_at(
        show,
        index,
        root,
        over_media,
        reveal,
        clicks_shown,
        page,
        variables,
        OUTPUT_WIDTH,
        OUTPUT_HEIGHT,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn frame_for_slide_at(
    show: &ShowFile,
    index: usize,
    root: Option<&Path>,
    over_media: bool,
    reveal: Option<u32>,
    clicks_shown: bool,
    page: Option<(usize, usize)>,
    variables: &[(String, String)],
    output_width: u32,
    output_height: u32,
) -> Frame {
    match styled_slide_at(
        show,
        index,
        root,
        over_media,
        reveal,
        clicks_shown,
        page,
        variables,
        output_width,
        output_height,
    ) {
        Some(slide) => render_styled(&slide),
        None => clear_frame(output_width, output_height),
    }
}

#[allow(clippy::too_many_arguments)]
fn styled_slide_at(
    show: &ShowFile,
    index: usize,
    root: Option<&Path>,
    over_media: bool,
    reveal: Option<u32>,
    clicks_shown: bool,
    page: Option<(usize, usize)>,
    variables: &[(String, String)],
    output_width: u32,
    output_height: u32,
) -> Option<StyledSlide> {
    let slide_id = show
        .ordered_slides()
        .get(index)
        .map(|slide| slide.id.clone())?;
    let slide = show
        .show
        .get("slides")
        .and_then(|slides| slides.get(&slide_id))?;
    Some(styled_slide(
        slide,
        root,
        over_media,
        reveal,
        clicks_shown,
        page,
        variables,
        output_width,
        output_height,
    ))
}

#[allow(clippy::too_many_arguments)]
fn styled_slide(
    slide: &serde_json::Value,
    root: Option<&Path>,
    over_media: bool,
    reveal: Option<u32>,
    clicks_shown: bool,
    page: Option<(usize, usize)>,
    variables: &[(String, String)],
    output_width: u32,
    output_height: u32,
) -> StyledSlide {
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
    StyledSlide {
        output_width,
        output_height,
        source_width,
        source_height,
        background,
        items,
    }
}

fn styled_item(
    item: &serde_json::Value,
    root: Option<&Path>,
    reveal: Option<u32>,
    page: Option<(usize, usize)>,
    variables: &[(String, String)],
) -> StyledItem {
    let chords_on = item
        .pointer("/chords/enabled")
        .and_then(|value| value.as_bool())
        == Some(true);
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
                        reveal.map(|count| *index < count as usize).unwrap_or(true)
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
                    chord
                        .into_iter()
                        .chain(std::iter::once(lyric))
                        .collect::<Vec<_>>()
                })
                .collect()
        })
        .unwrap_or_default();
    let kind = item
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let qr = kind == "qr_code";
    let image = if qr {
        freeshow_data::QrItem::parse(item)
            .and_then(|qr| freeshow_output::qr_image(&qr.text, &qr.color, &qr.background))
    } else if let Some(raster) = box_item_image(item) {
        Some(raster)
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
            Fit::named(
                item.get("fit")
                    .and_then(|value| value.as_str())
                    .unwrap_or("contain"),
            )
        },
        line_bg: item
            .pointer("/specialStyle/lineBg")
            .and_then(|value| value.as_str())
            .and_then(parse_color),
        line_gap: json_number(item.pointer("/specialStyle/lineGap")).unwrap_or(0.0),
        line_radius: json_number(item.pointer("/specialStyle/lineRadius")).unwrap_or(0.0),
        flip_x: item.get("flipped").and_then(|value| value.as_bool()) == Some(true),
        flip_y: item.get("flippedY").and_then(|value| value.as_bool()) == Some(true),
        opacity: filter_opacity(
            item.get("filter")
                .and_then(|value| value.as_str())
                .unwrap_or(""),
        ),
        grayscale: filter_grayscale(
            item.get("filter")
                .and_then(|value| value.as_str())
                .unwrap_or(""),
        ),
        invert: filter_invert(
            item.get("filter")
                .and_then(|value| value.as_str())
                .unwrap_or(""),
        ),
        sepia: filter_sepia(
            item.get("filter")
                .and_then(|value| value.as_str())
                .unwrap_or(""),
        ),
        brightness: filter_brightness(
            item.get("filter")
                .and_then(|value| value.as_str())
                .unwrap_or(""),
        ),
        contrast: filter_contrast(
            item.get("filter")
                .and_then(|value| value.as_str())
                .unwrap_or(""),
        ),
        saturate: filter_saturate(
            item.get("filter")
                .and_then(|value| value.as_str())
                .unwrap_or(""),
        ),
        hue_rotate: filter_hue_rotate(
            item.get("filter")
                .and_then(|value| value.as_str())
                .unwrap_or(""),
        ),
        blur: filter_blur(
            item.get("filter")
                .and_then(|value| value.as_str())
                .unwrap_or(""),
        ),
        blend: blend_mode(
            item.get("blend")
                .and_then(|value| value.as_str())
                .unwrap_or("normal"),
        ),
        crop: item_crop(item),
    }
}

fn item_crop(item: &serde_json::Value) -> freeshow_output::Crop {
    freeshow_output::Crop {
        top: json_number(item.pointer("/cropping/top")).unwrap_or(0.0),
        right: json_number(item.pointer("/cropping/right")).unwrap_or(0.0),
        bottom: json_number(item.pointer("/cropping/bottom")).unwrap_or(0.0),
        left: json_number(item.pointer("/cropping/left")).unwrap_or(0.0),
        clip: item
            .pointer("/cropping/type")
            .and_then(|value| value.as_str())
            != Some("ppt"),
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
                            run.get("value")
                                .and_then(|value| value.as_str())
                                .unwrap_or(""),
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
            let pos = chord
                .get("pos")
                .and_then(|value| value.as_u64())
                .unwrap_or(0) as usize;
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

fn box_item_image(item: &serde_json::Value) -> Option<Raster> {
    let kind = item
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    match kind {
        "web" | "html" | "website" => Some(html_box_image(item)),
        "weather" => Some(weather_box_image(item)),
        _ => None,
    }
}

fn box_size(item: &serde_json::Value) -> (u32, u32) {
    let width = json_number(item.get("width"))
        .or_else(|| json_number(item.pointer("/style/width")))
        .unwrap_or(640.0)
        .clamp(160.0, 1920.0) as u32;
    let height = json_number(item.get("height"))
        .or_else(|| json_number(item.pointer("/style/height")))
        .unwrap_or(360.0)
        .clamp(90.0, 1080.0) as u32;
    (width, height)
}

fn html_box_image(item: &serde_json::Value) -> Raster {
    let (width, height) = box_size(item);
    let src = item
        .get("src")
        .or_else(|| item.get("url"))
        .or_else(|| item.pointer("/web/src"))
        .or_else(|| item.pointer("/web/url"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(src) = src {
        if freeshow_media::is_remote_url(src) {
            return freeshow_output::web_preview(src, width, height);
        }
        return freeshow_output::html_snippet(src, width, height);
    }
    let html = item
        .get("html")
        .or_else(|| item.get("content"))
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if html.trim().is_empty() {
        let text = item
            .get("lines")
            .and_then(|value| value.as_array())
            .map(|lines| {
                lines
                    .iter()
                    .filter_map(|line| {
                        if let Some(text) = line.get("text").and_then(|value| value.as_str()) {
                            return Some(text.to_string());
                        }
                        line.get("text")
                            .and_then(|value| value.as_array())
                            .map(|runs| {
                                runs.iter()
                                    .filter_map(|run| run.get("value").and_then(|v| v.as_str()))
                                    .collect::<Vec<_>>()
                                    .join("")
                            })
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        return freeshow_output::html_snippet(&text, width, height);
    }
    freeshow_output::html_snippet(html, width, height)
}

fn weather_box_image(item: &serde_json::Value) -> Raster {
    let (width, height) = box_size(item);
    let weather = item.get("weather").unwrap_or(item);
    let fahrenheit = weather
        .get("useFahrenheit")
        .and_then(|value| value.as_bool())
        == Some(true);
    let lat = weather
        .get("latitude")
        .and_then(|value| value.as_f64())
        .unwrap_or(0.0);
    let lon = weather
        .get("longitude")
        .and_then(|value| value.as_f64())
        .unwrap_or(0.0);
    let cached = crate::operator::cached_weather(lat, lon);
    let temperature = cached
        .as_ref()
        .map(|report| report.temperature_label(fahrenheit))
        .or_else(|| {
            weather
                .get("temperature")
                .or_else(|| weather.get("temp"))
                .and_then(|value| value.as_f64())
                .map(|c| {
                    if fahrenheit {
                        format!("{:.0}°F", c * 9.0 / 5.0 + 32.0)
                    } else {
                        format!("{:.0}°C", c)
                    }
                })
        })
        .or_else(|| {
            weather
                .get("temperatureLabel")
                .and_then(|value| value.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "—".to_string());
    let detail = cached
        .as_ref()
        .map(|report| report.description.clone())
        .or_else(|| {
            weather
                .get("description")
                .or_else(|| weather.get("text"))
                .and_then(|value| value.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default();
    let title = if lat == 0.0 && lon == 0.0 {
        "Weather".to_string()
    } else {
        format!("{lat:.2}, {lon:.2}")
    };
    freeshow_output::weather_card(&title, &temperature, &detail, width, height)
}

pub(crate) fn load_image(root: Option<&Path>, src: &str) -> Option<Raster> {
    let bytes = if freeshow_media::is_remote_url(src) {
        fetch_remote_image(src)?
    } else {
        std::fs::read(resolve_media(root, src)?).ok()?
    };
    decode_image(&bytes)
}

fn fetch_remote_image(url: &str) -> Option<Vec<u8>> {
    if !freeshow_media::is_remote_url(url) {
        return None;
    }
    const LIMIT: u64 = 16 * 1024 * 1024;
    let response = ureq::get(url)
        .timeout(std::time::Duration::from_secs(5))
        .call()
        .ok()?;
    if !(200..300).contains(&response.status()) {
        return None;
    }
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= LIMIT && !bytes.is_empty()).then_some(bytes)
}

pub(crate) fn resolve_media(root: Option<&Path>, src: &str) -> Option<PathBuf> {
    let path = PathBuf::from(src);
    if path.is_file() {
        return Some(path);
    }
    let root = root?;
    let joined = root.join(src);
    joined.is_file().then_some(joined)
}

fn lines_slide_at(
    lines: &[String],
    over_media: bool,
    output_width: u32,
    output_height: u32,
) -> StyledSlide {
    if lines.is_empty() {
        return StyledSlide {
            output_width,
            output_height,
            source_width: SOURCE_WIDTH,
            source_height: SOURCE_HEIGHT,
            background: if over_media {
                Rgba::TRANSPARENT
            } else {
                Rgba::BLACK
            },
            items: Vec::new(),
        };
    }
    StyledSlide {
        output_width,
        output_height,
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_and_template_thumbnails_use_their_background_and_items() {
        let root = std::env::temp_dir().join(format!("freeshow-cat-thumb-{}", std::process::id()));
        let session = Session::open(root);
        let template = serde_json::json!({
            "name": "Red",
            "settings": { "backgroundColor": "#ff0000" },
            "items": []
        });
        let thumb = catalog_thumbnail(&session, &template, 128, 72);
        assert_eq!((thumb.width, thumb.height), (128, 72));
        assert_eq!(&thumb.pixels[..3], &[255, 0, 0]);
        let overlay = serde_json::json!({
            "name": "Box",
            "items": [{ "type": "text", "style": "left:0px;top:0px;width:1920px;height:1080px;background-color:#00ff00;", "lines": [] }]
        });
        let thumb = catalog_thumbnail(&session, &overlay, 128, 72);
        assert_eq!(&thumb.pixels[64 * 4 + 36 * 128 * 4..][..3], &[0, 255, 0]);
    }

    #[test]
    fn slide_thumbnails_are_small_and_show_the_text() {
        let mut show = ShowFile::blank("thumb", "Thumb");
        assert!(show.replace_lyrics_with("Amazing grace\nhow sweet the sound", true));
        let root = std::env::temp_dir().join(format!("freeshow-thumb-{}", std::process::id()));
        let session = Session::open(root);
        let thumb = slide_thumbnail(&session, &show, 0, 320, 180);
        assert_eq!((thumb.width, thumb.height), (320, 180));
        assert!(
            thumb.pixels.chunks(4).any(|pixel| pixel[0] > 128),
            "text is drawn"
        );
    }
    use freeshow_data::Catalog;
    use std::io::Write;
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn an_http_still_is_decoded_from_a_remote_url() {
        let png = include_bytes!("../../freeshow-data/tests/fixtures/lesson-cover.png");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut request = [0_u8; 1024];
                let _ = std::io::Read::read(&mut stream, &mut request);
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    png.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(png);
            }
        });
        let url = format!("http://{addr}/sky.png");
        let image = load_image(None, &url).expect("remote still");
        assert!(image.width > 0 && image.height > 0);
        let _ = server.join();
    }

    #[test]
    fn imported_propresenter_box_paints_at_its_original_resolution() {
        let root = std::env::temp_dir().join(freeshow_data::new_id());
        let library = freeshow_data::Library::open(&root);
        std::fs::create_dir_all(library.imports_dir()).unwrap();
        std::fs::write(
            library.imports_dir().join("Test.pro"),
            include_bytes!("../../freeshow-data/tests/fixtures/propresenter7.pro"),
        )
        .unwrap();
        assert!(library.import_show_sources().unwrap().is_empty());
        let show = library.list_shows().unwrap().remove(0);
        std::fs::remove_dir_all(root).unwrap();

        let frame = frame_for_slide(&show, 1, None, false, None, true, None, &[]);
        assert_eq!(frame.pixel(0, 0), parse_color("#000040").unwrap());
        let green = frame
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .filter(|(_, pixel)| pixel[1] > 220 && pixel[0] < 30 && pixel[2] < 30)
            .map(|(index, _)| (index % frame.width as usize, index / frame.width as usize))
            .collect::<Vec<_>>();
        assert!(!green.is_empty(), "the styled text did not render");
        assert!(
            green
                .iter()
                .all(|(x, y)| (66..1180).contains(x) && (90..408).contains(y))
        );
    }

    #[test]
    fn network_background_renders_and_stops_after_clear_or_replacement() {
        let mut session = Session::empty();
        let received = Raster {
            width: 16,
            height: 9,
            rgba: [255, 0, 0, 255].repeat(16 * 9),
        };
        session.network_background = true;
        assert!(session.update_network_background(received.clone()));
        assert_eq!(
            audience_frame(&session).pixel(80, 45),
            parse_color("#f00").unwrap()
        );

        session.apply_remote(freeshow_remote::Command::ClearBackground);
        assert!(!session.update_network_background(received.clone()));
        assert_ne!(
            audience_frame(&session).pixel(80, 45),
            parse_color("#f00").unwrap()
        );

        session.network_background = true;
        assert!(session.update_network_background(received.clone()));
        session.replace_underlay(Some(Raster {
            width: 16,
            height: 9,
            rgba: [0, 255, 0, 255].repeat(16 * 9),
        }));
        assert!(!session.update_network_background(received));
        assert_eq!(
            audience_frame(&session).pixel(80, 45),
            parse_color("#0f0").unwrap()
        );
    }

    #[test]
    fn stage_lyrics_can_mirror_the_current_background() {
        use freeshow_data::StageItemKind;
        let mut session = Session::empty();
        session
            .stage
            .items
            .retain(|item| item.kind == StageItemKind::Slide);
        assert!(!session.stage.items.is_empty());
        session.stage.items.truncate(1);
        let box_spec = &mut session.stage.items[0];
        box_spec.style = "left:0px;top:0px;width:320px;height:180px;".into();
        box_spec.include_media = true;
        session.replace_underlay(Some(Raster {
            width: 16,
            height: 9,
            rgba: [255, 0, 0, 255].repeat(144),
        }));
        let frame = stage_frame(&session);
        assert_eq!(frame.pixel(80, 45), parse_color("#ff0000").unwrap());
        assert_ne!(frame.pixel(400, 200), parse_color("#ff0000").unwrap());
        session.stage.items[0].include_media = false;
        assert_ne!(
            stage_frame(&session).pixel(80, 45),
            parse_color("#ff0000").unwrap()
        );
    }

    #[test]
    fn stage_video_boxes_decode_local_clips_without_using_audience_playback() {
        let root = std::env::temp_dir().join(freeshow_data::new_id());
        let library = freeshow_data::Library::open(&root);
        let stage_path = library.stage_path();
        std::fs::create_dir_all(stage_path.parent().unwrap()).unwrap();
        std::fs::write(
            root.join("clip.mp4"),
            include_bytes!("../../freeshow-data/tests/fixtures/lesson-clip.mp4"),
        )
        .unwrap();
        std::fs::write(stage_path, r##"{
            "layout":{"settings":{"color":"#111111"}, "items":{"clip":{"type":"media", "src":"clip.mp4",
            "style":"left:0px;top:0px;width:320px;height:180px;"}}}
        }"##).unwrap();
        let mut session = Session::empty();
        session.root = Some(root.clone());
        session.stage = library.load_stage_layout();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let frame = stage_frame(&session);
            let pixel = frame.pixel(80, 45);
            if pixel.r < 50 && pixel.g > 100 && pixel.b < 50 {
                assert_eq!(frame.pixel(400, 200), parse_color("#111111").unwrap());
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "stage clip did not paint green: {pixel:?}, (player open, decoded frame, error): {:?}",
                session.stage_media.borrow().file_status("clip.mp4")
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(session.underlay.is_none());
        session.stage.items.clear();
        assert_eq!(
            stage_frame(&session).pixel(80, 45),
            parse_color("#111111").unwrap()
        );
        drop(session);
        std::fs::remove_dir_all(root).unwrap();
    }

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
            assert_eq!(frame.pixel(80, 0), quiet_zone);
            assert_eq!(frame.pixel(238, 158), quiet_zone);
            assert_ne!(frame.pixel(79, 0), quiet_zone);
            assert_ne!(frame.pixel(240, 158), quiet_zone);
            assert_eq!(frame.pixel(104, 24), foreground);
        }
        for y in 0..160 {
            for x in 80..240 {
                assert_eq!(audience.pixel(x, y), stage.pixel(x, y));
            }
        }
    }

    #[test]
    fn html_and_weather_boxes_paint_into_the_item() {
        let html = serde_json::json!({
            "type": "html",
            "html": "<h1>Hello</h1><p>World</p>",
            "style": "top:0px;left:0px;width:400px;height:200px;"
        });
        let web = serde_json::json!({
            "type": "web",
            "src": "https://example.org/announcements",
            "style": "top:0px;left:0px;width:400px;height:200px;"
        });
        let weather = serde_json::json!({
            "type": "weather",
            "weather": {"latitude": 27.7, "longitude": 85.3, "temperature": 18.0, "description": "Clear"},
            "style": "top:0px;left:0px;width:400px;height:200px;"
        });
        assert!(box_item_image(&html).is_some());
        let preview = box_item_image(&web).unwrap();
        assert!(preview.width >= 160);
        let card = box_item_image(&weather).unwrap();
        assert!(card.width >= 160);
        assert!(card.height >= 90);
    }

    #[test]
    fn output_delay_releases_the_older_frame_and_zero_is_a_passthrough() {
        let first = Frame {
            width: 1,
            height: 1,
            pixels: vec![255, 0, 0, 255],
        };
        let second = Frame {
            width: 1,
            height: 1,
            pixels: vec![0, 0, 255, 255],
        };
        let mut buffer = OutputDelay::new();
        let early = buffer.push(first.clone(), 100, 10);
        assert_eq!(early.pixels, vec![0, 0, 0, 255]);
        let released = buffer.push(second.clone(), 100, 120);
        assert_eq!(released.pixels, first.pixels);
        let now = buffer.push(second.clone(), 0, 200);
        assert_eq!(now.pixels, second.pixels);
        assert!(buffer.frames.is_empty());
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
        session.replace_underlay(Some(Raster {
            width: 4,
            height: 4,
            rgba: [255, 0, 0, 255].repeat(16),
        }));
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
    fn current_output_boxes_honor_edited_sources_and_labels_after_reopening() {
        let temporary = std::env::temp_dir().join(freeshow_data::new_id());
        std::fs::create_dir_all(&temporary).unwrap();
        let library = freeshow_data::Library::open(&temporary);
        std::fs::write(
            library.settings_path(),
            r#"{"outputs":{"screen":{"name":"Room","active":true},"aux":{"name":"Lobby"}}}"#,
        )
        .unwrap();
        let stage_path = library.stage_path();
        std::fs::create_dir_all(stage_path.parent().unwrap()).unwrap();
        std::fs::write(
            &stage_path,
            r##"{"stage":{"settings":{"color":"#222"},"items":{
                "current_output":{"style":"left:0px;top:0px;width:320px;height:180px;"},
                "named":{"type":"current_output","style":"left:320px;top:0px;width:320px;height:180px;",
                    "currentOutput":{"source":"screen","showLabel":true}},
                "other":{"type":"current_output","style":"left:640px;top:0px;width:320px;height:180px;",
                    "currentOutput":{"source":"aux","extension":true}}
            }}}"##,
        )
        .unwrap();
        let mut session = Session::open(temporary.clone());
        let new_box = session
            .add_stage_item("output", "left:960px;top:0px;width:320px;height:180px;")
            .unwrap();
        session
            .stage_set(&new_box, "output-source", "screen")
            .unwrap();
        session
            .stage_set(&new_box, "show-output-label", "true")
            .unwrap();
        assert!(
            session
                .stage_set(&new_box, "output-source", "missing")
                .is_err()
        );

        let mut session = Session::open(temporary.clone());
        let row = session
            .stage_rows()
            .into_iter()
            .find(|row| row.id == new_box)
            .unwrap();
        assert_eq!(row.kind, "output");
        assert_eq!(row.output_source, "screen");
        assert!(row.show_output_label);
        session.blanked = false;
        session.slide_hidden = true;
        session.replace_underlay(Some(Raster {
            width: 16,
            height: 9,
            rgba: [255, 0, 0, 255].repeat(16 * 9),
        }));

        let stage = stage_frame(&session);
        let picture = parse_color("#f00").unwrap();
        assert_eq!(stage.pixel(80, 50), picture);
        assert_eq!(stage.pixel(480, 50), picture);
        assert_eq!(stage.pixel(800, 50), parse_color("#222").unwrap());
        assert!((70..180).any(|y| (320..640).any(|x| stage.pixel(x, y).g > 200)));
        assert_eq!(stage.pixel(1120, 50), picture);
        assert!((70..180).any(|y| (960..1280).any(|x| stage.pixel(x, y).g > 200)));

        session.stage_set("other", "output-source", "").unwrap();
        let reloaded = library.load_stage_layout();
        assert!(
            reloaded
                .items
                .iter()
                .find(|item| item.id == "other")
                .unwrap()
                .output_source
                .is_none()
        );
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(stage_path).unwrap()).unwrap();
        assert_eq!(
            saved["stage"]["items"]["other"]["currentOutput"]["extension"],
            true
        );
        std::fs::remove_dir_all(temporary).unwrap();
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
        *show
            .show
            .pointer_mut("/slides/s1/items/0/chords/enabled")
            .unwrap() = serde_json::json!(true);
        *show
            .show
            .pointer_mut("/slides/s1/items/0/chords/size")
            .unwrap() = serde_json::json!(100);
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
        *show
            .show
            .pointer_mut("/slides/s1/items/0/chords/offsetY")
            .unwrap() = serde_json::json!(80);
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
                row.chunks(4)
                    .any(|pixel| pixel[0] > 200 && pixel[1] > 80 && pixel[1] < 200 && pixel[2] < 80)
            })
            .count()
    }

    #[test]
    fn an_item_picture_uses_the_fit_stored_on_the_item() {
        let root = std::env::temp_dir().join(format!("freeshow-rust-fit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let png = [
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
            0x00, 0x76, 0x5e, 0x98, 0x9a, 0x00, 0x00, 0x00, 0x10, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0x00, 0x44, 0x60, 0xe2, 0x3f, 0x00, 0x19, 0xf3, 0x03,
            0xfd, 0x9a, 0xd8, 0x73, 0x31, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
            0x42, 0x60, 0x82,
        ];
        std::fs::write(root.join("wide.png"), png).unwrap();
        let mut show = ShowFile::blank("song", "Song");
        let item = show.show.pointer_mut("/slides/s1/items/0").unwrap();
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

        *show.show.pointer_mut("/slides/s1/items/0/fit").unwrap() = serde_json::json!("fill");
        let filled = frame_for_slide(&show, 0, None, true, None, true, None, &[]);
        assert_eq!(filled.pixel(0, 0).r, 255);
        assert_eq!(filled.pixel(0, 0).g, 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn powerpoint_media_uses_the_saved_source_crop() {
        let root = std::env::temp_dir().join(freeshow_data::new_id());
        std::fs::create_dir_all(&root).unwrap();
        let image = Frame {
            width: 2,
            height: 2,
            pixels: vec![255, 0, 0, 255, 0, 0, 255, 255, 0, 0, 0, 255, 0, 255, 0, 255],
        };
        std::fs::write(root.join("picture.png"), image.encode_png().unwrap()).unwrap();
        let mut show = ShowFile::blank("ppt-picture", "PowerPoint picture");
        show.show["slides"]["s1"]["items"] = serde_json::json!([{
            "type": "media",
            "src": "picture.png",
            "fit": "fill",
            "style": "left:0px;top:0px;width:1920px;height:1080px;",
            "cropping": {"type":"ppt","left":50,"top":50}
        }]);
        let render = |show: &ShowFile| {
            frame_for_slide_at(show, 0, Some(&root), false, None, true, None, &[], 4, 4)
        };
        let frame = render(&show);
        for pixel in frame.pixels.chunks_exact(4) {
            assert_eq!(pixel, [0, 255, 0, 255]);
        }
        show.show["slides"]["s1"]["items"][0]["cropping"]["type"] = serde_json::json!("native");
        let frame = render(&show);
        assert_eq!(frame.pixel(0, 0), Rgba::BLACK);
        assert_eq!(frame.pixel(3, 3).g, 255);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn separate_audience_and_stage_bounds_size_the_framebuffers() {
        let root =
            std::env::temp_dir().join(format!("freeshow-output-size-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Shows")).unwrap();
        std::fs::write(
            root.join("settings.json"),
            r#"{"outputs":{
                "a-stage":{"stageOutput":"default","bounds":{"width":1024,"height":768}},
                "b-audience":{"name":"Main","bounds":{"width":1280,"height":720}},
                "c-stage":{"stageOutput":"other","bounds":{"width":1920,"height":1080}}
            }}"#,
        )
        .unwrap();
        let mut session = crate::session::Session::open(root.clone());
        assert_eq!((session.output_width, session.output_height), (1280, 720));
        let frame = audience_frame(&session);
        assert_eq!((frame.width, frame.height), (1280, 720));
        session.stage.items.clear();
        session
            .add_stage_item(
                "text",
                "left:480px;top:270px;width:480px;height:270px;background:#ff0000;",
            )
            .unwrap();
        let stage = stage_frame(&session);
        assert_eq!((stage.width, stage.height), (1024, 768));
        assert_eq!(stage.pixel(256, 192), parse_color("#ff0000").unwrap());
        assert_ne!(stage.pixel(255, 192), stage.pixel(256, 192));
        assert_ne!(stage.pixel(256, 191), stage.pixel(256, 192));

        std::fs::write(
            root.join("settings.json"),
            r#"{"outputs":{"main":{"bounds":{"width":1280,"height":720}},"stage":{"stageOutput":"default"}}}"#,
        )
        .unwrap();
        let session = crate::session::Session::open(root.clone());
        assert_eq!(session.stage_output_size(), (1280, 720));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn standalone_clear_cue_keeps_the_actual_program_slide_on_audience_and_stage() {
        use freeshow_data::StageItemKind;
        use serde_json::json;

        let mut show = ShowFile::blank("cues", "Cues");
        let first = show.ordered_slides()[0].id.clone();
        let skipped = show.add_slide().unwrap();
        let clear = show.add_slide().unwrap();
        show.show["slides"][&first]["items"] = json!([
            {"type":"shape","style":"left:0px;top:0px;width:200px;height:200px;background:#ff0000;"},
            {"type":"text","lines":[{"text":[{"value":"On screen"}]}]}
        ]);
        show.show["slides"][&skipped]["settings"]["color"] = json!("#00ff00");
        show.show["slides"][&clear]["settings"]["actionOnly"] = json!(true);
        show.show["layouts"]["main"]["slides"][2]["actions"] = json!({
            "slideActions": [{"triggers": ["clear_background"]}]
        });
        let mut session = Session::empty();
        session.shows = vec![show];
        session.show_index = Some(0);
        session
            .stage
            .items
            .retain(|item| item.kind == StageItemKind::Slide && item.offset == 0);
        assert!(!session.stage.items.is_empty());
        session.select_slide(0);
        session.replace_underlay(Some(Raster {
            width: 16,
            height: 9,
            rgba: [0, 0, 255, 255].repeat(144),
        }));
        let original = audience_frame(&session);
        let stage = stage_frame(&session);
        assert_eq!(original.pixel(0, 0), parse_color("#ff0000").unwrap());
        assert_eq!(original.pixel(800, 300), parse_color("#0000ff").unwrap());

        session.select_slide(2);
        let after = audience_frame(&session);
        assert_eq!(session.slide_index, Some(2));
        assert_eq!(after.pixel(0, 0), original.pixel(0, 0));
        assert_eq!(after.pixel(800, 300), Rgba::BLACK);
        assert_eq!(stage_frame(&session).pixels, stage.pixels);
        session.clear_output();
        assert_eq!(audience_frame(&session).pixel(0, 0), Rgba::BLACK);
        assert_ne!(stage_frame(&session).pixels, stage.pixels);
    }

    #[test]
    fn hidden_slide_text_leaves_the_stage_too() {
        let mut show = ShowFile::blank("hide", "Hide");
        let first = show.ordered_slides()[0].id.clone();
        show.show["slides"][&first]["items"] = serde_json::json!([
            {"type":"text","lines":[{"text":[{"value":"On screen"}]}]}
        ]);
        let mut session = Session::empty();
        session.shows = vec![show];
        session.show_index = Some(0);
        session
            .stage
            .items
            .retain(|item| item.kind == freeshow_data::StageItemKind::Slide && item.offset == 0);
        let empty = stage_frame(&session);
        session.select_slide(0);
        assert_ne!(stage_frame(&session).pixels, empty.pixels);
        session.slide_hidden = true;
        assert_eq!(stage_frame(&session).pixels, empty.pixels);
    }

    #[test]
    fn browsing_another_show_keeps_the_live_slide_on_the_output() {
        let root = std::env::temp_dir().join(format!("freeshow-rust-held-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_show_with("a", "Alpha song", "Alpha\n\nBeta"));
        assert!(session.create_show_with("b", "Bravo song", "Gamma"));
        let position = |session: &Session, id: &str| {
            session.shows.iter().position(|show| show.id == id).unwrap()
        };
        session.tab = "shows".into();
        session
            .stage
            .items
            .retain(|item| item.kind == freeshow_data::StageItemKind::Slide && item.offset == 0);
        session.select_drawer(position(&session, "a"));
        session.select_slide(0);
        let live = audience_frame(&session);
        let live_stage = stage_frame(&session);
        assert!(live.pixels.chunks(4).any(|pixel| pixel[0] > 200));

        session.switch_tab("media");
        session.switch_tab("shows");
        session.select_drawer(position(&session, "b"));
        assert!(session.program_held);
        assert_eq!(
            audience_frame(&session).pixels,
            live.pixels,
            "still the live slide"
        );
        assert_eq!(
            stage_frame(&session).pixels,
            live_stage.pixels,
            "the stage follows the output, not the browsed show"
        );

        assert!(session.perform_shortcut("Right"));
        assert_eq!(session.show_index, Some(position(&session, "a")));
        assert_eq!(session.slide_index, Some(1));
        assert!(!session.program_held);
        assert_ne!(audience_frame(&session).pixels, live.pixels);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn editing_the_live_slide_marks_the_output_for_a_repaint() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-live-edit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_show_with("a", "Alpha song", "Alpha\n\nBeta"));
        assert!(session.create_show_with("b", "Bravo song", "Gamma"));
        let position = |session: &Session, id: &str| {
            session.shows.iter().position(|show| show.id == id).unwrap()
        };
        session.tab = "shows".into();
        session.select_drawer(position(&session, "a"));
        session.select_slide(0);
        let before = audience_frame(&session);
        session.select_slide_for_edit(0);
        assert!(session.save_lyrics("Alpha, now much longer"));
        assert!(session.live_edited);
        assert_ne!(
            audience_frame(&session).pixels,
            before.pixels,
            "the edit is on the output"
        );

        session.live_edited = false;
        session.select_drawer(position(&session, "b"));
        session.select_slide_for_edit(0);
        assert!(session.save_lyrics("Gamma two"));
        assert!(!session.live_edited, "another show changed");
        let _ = std::fs::remove_dir_all(&root);
    }
}
