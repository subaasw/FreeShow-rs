// SPDX-License-Identifier: GPL-3.0-only

mod church_import;
mod desktop;
mod feeds;
mod lang;
mod live;
mod operator;
mod present;
mod runtime;
mod service_check;
mod session;
mod spell;
mod stage;
mod streamdeck;
mod theme;
mod update;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex, mpsc};

use church_import::ChurchImport;
use freeshow_ai::SlideAssist;
use freeshow_cloud::CloudBackup;
use freeshow_core::{AppSettings, Rgb, WindowPlacement, WindowProfile};
use freeshow_data::{MediaEntry, MediaEntryKind};
use freeshow_media::{AudioOutput, AudioPlaybackOptions, VideoPlayer};
use freeshow_obs::ObsControl;
use freeshow_output::{Frame, Raster, clear_frame, decode_image, fit_raster, parse_color};
use freeshow_pco::PlanSource;
use freeshow_record::OutputCapture;
use freeshow_remote::Integration;
use freeshow_timecode::TimecodeSource;
use live::{OpeningFade, audience_frame, edit_frame, motion_frame, stage_frame};
use present::{FrameSlot, attach_frame};
use session::{DrawerAction, PendingPlay, Session};
use slint::winit_030::WinitWindowAccessor;

slint::include_modules!();

fn color(rgb: Rgb) -> slint::Color {
    slint::Color::from_rgb_u8(rgb.r, rgb.g, rgb.b)
}

fn color_rgba(color: freeshow_core::Rgba) -> slint::Color {
    slint::Color::from_argb_u8(color.a, color.r, color.g, color.b)
}

/// The theme's text colour on `back`, or black or white when that text would be hard to read
/// (some built-in themes, such as Papyrus, put dark text on a dark accent).
fn readable_on(text: Rgb, back: Rgb) -> Rgb {
    fn luminance(rgb: Rgb) -> f32 {
        let channel = |value: u8| {
            let value = f32::from(value) / 255.0;
            if value <= 0.039_28 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(rgb.r) + 0.7152 * channel(rgb.g) + 0.0722 * channel(rgb.b)
    }
    let (a, b) = (luminance(text), luminance(back));
    let ratio = (a.max(b) + 0.05) / (a.min(b) + 0.05);
    if ratio >= 1.5 {
        text
    } else if b > 0.18 {
        Rgb {
            r: 16,
            g: 16,
            b: 20,
        }
    } else {
        Rgb {
            r: 240,
            g: 240,
            b: 255,
        }
    }
}

fn apply_theme(ui: &ControlWindow, theme: &theme::OperatorTheme) {
    let palette = &theme.palette;
    ui.set_primary(color(palette.primary));
    ui.set_primary_lighter(color(palette.primary_lighter));
    ui.set_primary_darker(color(palette.primary_darker));
    ui.set_primary_darkest(color(palette.primary_darkest));
    ui.set_text_color(color(palette.text));
    ui.set_accent(color(palette.secondary));
    ui.set_accent_text(color(readable_on(
        palette.secondary_text,
        palette.secondary,
    )));
    let primary = palette.primary;
    let luma = 0.2126 * f32::from(primary.r)
        + 0.7152 * f32::from(primary.g)
        + 0.0722 * f32::from(primary.b);
    ui.set_dark_chrome(luma < 128.0);
    let style = ui.global::<OperatorStyle>();
    style.set_hover(color_rgba(palette.hover));
    style.set_focus_fill(color_rgba(palette.focus));
    style.set_chrome_radius(theme.border_radius);
    // Text size and font apply at once, not only after a restart.
    style.set_font_scale(theme.font_scale);
    ui.set_theme_font_family(theme.font_family.clone().into());
}

const SLIDE_THUMB_WIDTH: u32 = 320;
const SLIDE_THUMB_HEIGHT: u32 = 180;

thread_local! {
    /// Slide thumbnails keyed by show content and position, so a refresh only draws changes.
    static SLIDE_THUMBS: RefCell<std::collections::HashMap<u64, slint::Image>> =
        RefCell::new(std::collections::HashMap::new());
}

/// Thumbnails for every slide of the open show, or none in list mode and scripture.
fn slide_thumbs(session: &Session) -> Vec<slint::Image> {
    use std::hash::{Hash, Hasher};
    if session.tab == "scripture" || session.slides_list_columns() < 2 {
        return Vec::new();
    }
    let Some(show) = session
        .show_index
        .and_then(|index| session.shows.get(index))
    else {
        return Vec::new();
    };
    let mut base = std::collections::hash_map::DefaultHasher::new();
    show.id.hash(&mut base);
    show.show.to_string().hash(&mut base);
    session.root.hash(&mut base);
    let base = base.finish();
    let count = show.ordered_slides().len();
    SLIDE_THUMBS.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len() > 4000 {
            cache.clear();
        }
        (0..count)
            .map(|index| {
                let mut key = std::collections::hash_map::DefaultHasher::new();
                (base, index).hash(&mut key);
                cache
                    .entry(key.finish())
                    .or_insert_with(|| {
                        let frame = live::slide_thumbnail(
                            session,
                            show,
                            index,
                            SLIDE_THUMB_WIDTH,
                            SLIDE_THUMB_HEIGHT,
                        );
                        present::frame_image(&frame)
                    })
                    .clone()
            })
            .collect()
    })
}

fn slide_cards(session: &Session) -> slint::ModelRc<SlideRowData> {
    let full_colors = session.preferences.full_colors();
    let thumbs = slide_thumbs(session);
    let cards = session
        .slide_rows()
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let mut data = slide_row_data(index, row, full_colors);
            if let Some(thumb) = thumbs.get(index) {
                data.thumb = thumb.clone();
                data.has_thumb = true;
            }
            data
        })
        .collect::<Vec<_>>();
    slint::ModelRc::new(slint::VecModel::from(cards))
}

fn slide_row_data(index: usize, row: &session::SlideLabel, full_colors: bool) -> SlideRowData {
    let default_text = slint::Color::from_argb_u8(255, 240, 240, 255);
    if let Some(color) = row.color.as_deref().and_then(parse_color) {
        let group = slint::Color::from_argb_u8(color.a, color.r, color.g, color.b);
        SlideRowData {
            label: row.text.as_str().into(),
            index: index as i32,
            has_color: true,
            group_color: group,
            group_text: if full_colors {
                contrast_color(color)
            } else {
                group
            },
            ..SlideRowData::default()
        }
    } else {
        SlideRowData {
            label: row.text.as_str().into(),
            index: index as i32,
            has_color: false,
            group_color: slint::Color::from_argb_u8(0, 0, 0, 0),
            group_text: default_text,
            ..SlideRowData::default()
        }
    }
}

fn contrast_color(color: freeshow_output::Rgba) -> slint::Color {
    let lum =
        0.2126 * f32::from(color.r) + 0.7152 * f32::from(color.g) + 0.0722 * f32::from(color.b);
    if lum > 140.0 {
        slint::Color::from_argb_u8(255, 16, 16, 20)
    } else {
        slint::Color::from_argb_u8(255, 240, 240, 255)
    }
}

fn strings(items: &[String]) -> slint::ModelRc<slint::SharedString> {
    let values: Vec<slint::SharedString> = items.iter().map(|item| item.as_str().into()).collect();
    slint::ModelRc::new(slint::VecModel::from(values))
}

fn drawer_rows(session: &Session) -> slint::ModelRc<DrawerRowData> {
    let rows = if session.tab == "media" {
        session
            .media
            .iter()
            .map(|entry| DrawerRowData {
                name: entry.label().into(),
                thumb: media_thumb(entry),
                show_thumb: true,
                on: false,
            })
            .collect::<Vec<_>>()
    } else {
        let catalog = match session.tab.as_str() {
            "overlays" => Some(&session.overlay_catalog),
            "templates" => Some(&session.template_catalog),
            _ => None,
        };
        session
            .drawer()
            .into_iter()
            .zip(session.drawer_on())
            .map(|(name, on)| {
                let entry = catalog.and_then(|catalog| {
                    catalog
                        .id_named(&name)
                        .and_then(|id| catalog.get(&id).cloned())
                });
                DrawerRowData {
                    thumb: entry
                        .as_ref()
                        .map(|entry| catalog_thumb(session, entry))
                        .unwrap_or_default(),
                    show_thumb: catalog.is_some(),
                    name: name.into(),
                    on,
                }
            })
            .collect()
    };
    slint::ModelRc::new(slint::VecModel::from(rows))
}

/// Overlay and template thumbnails, kept until their saved look changes.
fn catalog_thumb(session: &Session, entry: &serde_json::Value) -> slint::Image {
    thread_local! {
        static CATALOG_THUMBS: RefCell<std::collections::HashMap<String, slint::Image>> =
            RefCell::new(std::collections::HashMap::new());
    }
    let key = entry.to_string();
    if let Some(image) = CATALOG_THUMBS.with(|thumbs| thumbs.borrow().get(&key).cloned()) {
        return image;
    }
    let image = present::frame_image(&live::catalog_thumbnail(session, entry, 128, 72));
    CATALOG_THUMBS.with(|thumbs| {
        let mut thumbs = thumbs.borrow_mut();
        if thumbs.len() > 256 {
            thumbs.clear();
        }
        thumbs.insert(key, image.clone());
    });
    image
}

const THUMB_WIDTH: u32 = 56;
const THUMB_HEIGHT: u32 = 40;

/// A media file and when it last changed, so an edited file gets a fresh thumbnail.
type ThumbKey = (PathBuf, Option<std::time::SystemTime>);

thread_local! {
    /// Drawer thumbnails already made, so a refresh does not decode every picture again.
    static MEDIA_THUMBS: RefCell<std::collections::HashMap<ThumbKey, slint::Image>> =
        RefCell::new(std::collections::HashMap::new());
}

/// Video stills from the background worker: `None` while queued, then the still (or none).
static VIDEO_STILLS: Mutex<Option<std::collections::HashMap<ThumbKey, Option<Option<Raster>>>>> =
    Mutex::new(None);
static VIDEO_STILLS_READY: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static VIDEO_STILL_QUEUE: std::sync::OnceLock<Mutex<mpsc::Sender<ThumbKey>>> =
    std::sync::OnceLock::new();

/// Queues a video for one background worker, which decodes stills one at a time.
fn queue_video_still(key: ThumbKey) {
    let queue = VIDEO_STILL_QUEUE.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<ThumbKey>();
        let _ = std::thread::Builder::new()
            .name("video-stills".into())
            .spawn(move || {
                for key in rx {
                    let still = freeshow_media::video_thumbnail(&key.0, THUMB_WIDTH, THUMB_HEIGHT);
                    if let Ok(mut stills) = VIDEO_STILLS.lock() {
                        stills
                            .get_or_insert_with(Default::default)
                            .insert(key, Some(still));
                    }
                    VIDEO_STILLS_READY.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            });
        Mutex::new(tx)
    });
    if let Ok(queue) = queue.lock() {
        let _ = queue.send(key);
    }
}

fn media_thumb(entry: &MediaEntry) -> slint::Image {
    let modified = std::fs::metadata(&entry.path)
        .and_then(|meta| meta.modified())
        .ok();
    let key: ThumbKey = (entry.path.clone(), modified);
    if let Some(image) = MEDIA_THUMBS.with(|thumbs| thumbs.borrow().get(&key).cloned()) {
        return image;
    }
    let made = if !entry.exists {
        Some(kind_thumb(entry))
    } else {
        match entry.kind {
            MediaEntryKind::Image => Some(
                std::fs::read(&entry.path)
                    .ok()
                    .and_then(|bytes| decode_image(&bytes))
                    .map(|image| fit_raster(&image, THUMB_WIDTH, THUMB_HEIGHT))
                    .unwrap_or_else(|| kind_thumb(entry)),
            ),
            MediaEntryKind::Video => {
                let mut stills = VIDEO_STILLS.lock().ok();
                let slot = stills
                    .as_mut()
                    .map(|stills| stills.get_or_insert_with(Default::default));
                match slot.as_ref().and_then(|stills| stills.get(&key)) {
                    Some(Some(still)) => Some(still.clone().unwrap_or_else(|| kind_thumb(entry))),
                    Some(None) => None,
                    None => {
                        if let Some(stills) = slot {
                            stills.insert(key.clone(), None);
                        }
                        drop(stills);
                        queue_video_still(key.clone());
                        None
                    }
                }
            }
            _ => Some(kind_thumb(entry)),
        }
    };
    match made {
        Some(raster) => {
            let image = slint_image(&raster);
            MEDIA_THUMBS.with(|thumbs| thumbs.borrow_mut().insert(key, image.clone()));
            image
        }
        // Still decoding: show the play icon until the still arrives.
        None => slint_image(&kind_thumb(entry)),
    }
}

fn slint_image(raster: &Raster) -> slint::Image {
    let needed = raster.width as usize * raster.height as usize * 4;
    if raster.width == 0 || raster.height == 0 || raster.rgba.len() < needed {
        return slint::Image::default();
    }
    slint::Image::from_rgba8(
        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
            &raster.rgba,
            raster.width,
            raster.height,
        ),
    )
}

fn kind_thumb(entry: &MediaEntry) -> Raster {
    let mut rgba = [0x1c, 0x1c, 0x28, 0xff].repeat((THUMB_WIDTH * THUMB_HEIGHT) as usize);
    let color = if !entry.exists {
        [0x88, 0x44, 0x44, 0xff]
    } else {
        match entry.kind {
            MediaEntryKind::Video => [0xf0, 0xf0, 0xff, 0xff],
            MediaEntryKind::Audio => [0x88, 0xcc, 0xff, 0xff],
            MediaEntryKind::Camera => [0xff, 0xcc, 0x66, 0xff],
            _ => [0xaa, 0xaa, 0xbb, 0xff],
        }
    };
    match entry.kind {
        MediaEntryKind::Video if entry.exists => draw_play(&mut rgba, color),
        MediaEntryKind::Audio if entry.exists => draw_bars(&mut rgba, color),
        _ => fill_rect(&mut rgba, 18, 10, 20, 20, color),
    }
    Raster {
        width: THUMB_WIDTH,
        height: THUMB_HEIGHT,
        rgba,
    }
}

fn put_pixel(rgba: &mut [u8], x: u32, y: u32, color: [u8; 4]) {
    if x >= THUMB_WIDTH || y >= THUMB_HEIGHT {
        return;
    }
    let index = ((y * THUMB_WIDTH + x) * 4) as usize;
    rgba[index..index + 4].copy_from_slice(&color);
}

fn fill_rect(rgba: &mut [u8], x: u32, y: u32, width: u32, height: u32, color: [u8; 4]) {
    for row in y..y + height {
        for column in x..x + width {
            put_pixel(rgba, column, row, color);
        }
    }
}

fn draw_play(rgba: &mut [u8], color: [u8; 4]) {
    for y in 10..30 {
        let inset = (y as i32 - 20).unsigned_abs();
        let start = 22 + inset / 2;
        let end = 38 - inset;
        for x in start..end {
            put_pixel(rgba, x, y, color);
        }
    }
}

fn draw_bars(rgba: &mut [u8], color: [u8; 4]) {
    for (index, height) in [10u32, 22, 16, 28, 12].into_iter().enumerate() {
        let x = 16 + index as u32 * 6;
        let y = 32 - height;
        fill_rect(rgba, x, y, 3, height, color);
    }
}

fn selected(index: Option<usize>) -> i32 {
    index.map(|value| value as i32).unwrap_or(-1)
}

/// Spelling hints for the lyrics editor; English libraries only, like FreeShow's default.
fn spelling_note(session: &Session, text: &str) -> String {
    let language = session.preferences.language.as_deref().unwrap_or("en");
    if language.is_empty() || language.starts_with("en") {
        spell::report(text, 4)
    } else {
        String::new()
    }
}

/// Tells the operator when another program holds the phone remote or stage port.
fn busy_ports_notice(remote: u16, stage: u16) -> Option<String> {
    let busy = |port: u16| std::net::TcpListener::bind(("0.0.0.0", port)).is_err();
    let names: Vec<String> = [("remote", remote), ("stage", stage)]
        .into_iter()
        .filter(|(_, port)| busy(*port))
        .map(|(name, port)| format!("{name} port {port}"))
        .collect();
    (!names.is_empty()).then(|| {
        let (verb, pages) = if names.len() > 1 {
            ("are", "those pages are")
        } else {
            ("is", "that page is")
        };
        format!(
            "The {} {verb} in use by another program, so {pages} off. Close the other program or pick another port in Settings, Connection, then restart.",
            names.join(" and ")
        )
    })
}

fn show_now_playing(ui: &ControlWindow, session: &Session) {
    let (name, time, progress, paused) = session.now_playing().unwrap_or_default();
    if ui.get_now_playing().as_str() != name {
        ui.set_now_playing(name.into());
        if session.tab == "audio" {
            ui.set_drawer_rows(drawer_rows(session));
        }
    }
    if ui.get_now_playing_time().as_str() != time {
        ui.set_now_playing_time(time.into());
    }
    if (ui.get_now_playing_progress() - progress).abs() > f32::EPSILON {
        ui.set_now_playing_progress(progress);
    }
    if ui.get_now_playing_paused() != paused {
        ui.set_now_playing_paused(paused);
    }
}

/// Status messages start with a capital letter, whichever part of the app wrote them.
fn sentence_case(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

thread_local! {
    /// Set whenever the operator window refreshes, so the clock redraws the stage soon after.
    static STAGE_DIRTY: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

fn refresh(ui: &ControlWindow, session: &Session) {
    STAGE_DIRTY.set(true);
    // A shortcut or remote command can open the next project item in another drawer tab.
    if ui.get_active_tab().as_str() != session.tab {
        ui.set_active_tab(session.tab.clone().into());
    }
    // The heading follows the tab and the chosen language.
    if let Some((_, label, _)) = lang::drawer_tabs(session.preferences.language.as_deref())
        .into_iter()
        .find(|(id, _, _)| *id == session.tab)
        && ui.get_panel_title().as_str() != label
    {
        ui.set_panel_title(label.into());
    }
    ui.set_drawer_rows(drawer_rows(session));
    ui.set_slide_cards(slide_cards(session));
    ui.set_slides_columns(session.slides_list_columns() as i32);
    ui.set_live_slide(session.live_slide_index().map_or(-1, |index| index as i32));
    ui.set_full_colors(session.preferences.full_colors());
    ui.set_project_names(strings(&session.project_labels()));
    ui.set_project_kinds(strings(&session.project_kinds()));
    ui.set_live_project(session.live_project_item().map_or(-1, |item| item as i32));
    ui.set_project_title(session.project_title().into());
    let lyrics = session.lyrics();
    if ui.get_lyrics().as_str() != lyrics {
        ui.set_spelling(spelling_note(session, &lyrics).into());
        ui.set_lyrics(lyrics.into());
    }
    ui.set_group(session.group().into());
    ui.set_item_label(session.item_label().into());
    ui.set_show_name(session.show_name().into());
    ui.set_notice(sentence_case(&session.notice).into());
    ui.set_can_restore(session.can_restore_show());
    ui.set_project_choices(strings(&session.project_choices()));
    ui.set_project_choice(session.project_choice_index());
    // Do not rewrite the show text under the operator's cursor while they type.
    if !(ui.get_text_mode() && ui.get_text_focused()) {
        let text = session.show_text();
        if ui.get_show_text().as_str() != text {
            ui.set_show_text(text.into());
        }
    }
    refresh_panels(ui, session);
    ui.set_selected_drawer(match session.tab.as_str() {
        "shows" => selected(session.visible_drawer_index(session.show_index)),
        "scripture" => selected(session.visible_drawer_index(session.bible_index)),
        "media" => selected(session.visible_drawer_index(session.media_index)),
        "audio" => selected(session.visible_drawer_index(session.audio_index)),
        "calendar" | "player" => selected(session.visible_drawer_index(session.drawer_pick)),
        _ => -1,
    });
    ui.set_volume(session.volume * 100.0);
    ui.set_chrome_status(session.chrome_status().into());
    ui.set_focus_mode(session.focus_mode);
    ui.set_guide_visible(session.guide_visible);
    ui.set_guide_text(session.guide_text().into());
    ui.set_drawer_height(session.preferences.drawer_height() as i32);
    ui.set_selected_slide(if session.tab == "scripture" {
        selected(session.verse_index)
    } else {
        selected(session.slide_index)
    });
    ui.set_selected_project(
        session
            .highlighted_project_item()
            .map_or(-1, |item| item as i32),
    );
}

/// Loads the picked Calendar event, Player video or RSS feed list into the drawer form.
fn fill_drawer_form(ui: &ControlWindow, session: &Session) {
    let form = session.drawer_form();
    let [a, b, c] = form.fields;
    ui.set_form_a(a.into());
    ui.set_form_b(b.into());
    ui.set_form_c(c.into());
    let mut shows = vec!["No show to start".to_string()];
    shows.extend(
        session
            .shows
            .iter()
            .map(|show| format!("Start {}", show.name())),
    );
    ui.set_form_show_names(strings(&shows));
    ui.set_form_show(form.show as i32);
    ui.set_form_editing(form.editing);
}

/// Settings, Inspector and Stage editor data.
fn refresh_panels(ui: &ControlWindow, session: &Session) {
    STAGE_DIRTY.set(true);
    let settings = session.settings_form();
    ui.set_settings_data(SettingsData {
        language: settings.language.into(),
        time_format: settings.time_format.into(),
        autosave: settings.autosave.into(),
        alert_updates: settings.alert_updates,
        auto_output: settings.auto_output,
        out_locked: settings.out_locked,
        full_colors: settings.full_colors,
        labels_disabled: settings.labels_disabled,
        group_numbers: settings.group_numbers,
        split_lines: settings.split_lines,
        remote_port: settings.remote_port,
        stage_port: settings.stage_port,
        max_connections: settings.max_connections,
        theme: settings.theme.into(),
        profiles: settings.profiles.into(),
        output_name: settings.output_name.into(),
        output_always_on_top: settings.output_always_on_top,
        output_kiosk: settings.output_kiosk,
        output_resolution: settings.output_resolution.into(),
        text_transition: settings.text_transition.into(),
        text_duration: settings.text_duration,
        media_transition: settings.media_transition.into(),
        media_duration: settings.media_duration,
        addresses: settings.addresses.into(),
        style_names: strings(&settings.style_names),
        style_name: settings.style_name.into(),
        style_is_default: settings.style_is_default,
        style_swatch: parse_color(&settings.style_background)
            .map_or(slint::Color::from_rgb_u8(0, 0, 0), |color| {
                slint::Color::from_argb_u8(color.a, color.r, color.g, color.b)
            }),
        style_background: settings.style_background.into(),
        style_fit: settings.style_fit.into(),
        style_blur_amount: settings.style_blur_amount,
        style_blur_opacity: settings.style_blur_opacity,
        style_lines: settings.style_lines,
        style_template: settings.style_template.into(),
        template_choices: strings(&settings.template_choices),
        output_names: strings(&settings.output_names),
        theme_colors: slint::ModelRc::new(slint::VecModel::from(
            settings
                .theme_colors
                .iter()
                .map(|(key, label, hex)| ThemeColorRow {
                    key: key.as_str().into(),
                    label: label.as_str().into(),
                    hex: hex.as_str().into(),
                    swatch: parse_color(hex).map_or(slint::Color::from_rgb_u8(0, 0, 0), |color| {
                        slint::Color::from_rgb_u8(color.r, color.g, color.b)
                    }),
                })
                .collect::<Vec<_>>(),
        )),
        theme_font: settings.theme_font.into(),
        theme_font_size: settings.theme_font_size,
        theme_radius: settings.theme_radius,
        theme_edited: settings.theme_edited,
    });
    ui.set_theme_names(strings(&settings.themes));
    ui.set_transition_kinds(strings(&session::TRANSITION_KINDS.map(str::to_string)));
    ui.set_autosave_choices(strings(&session::AUTOSAVE_CHOICES.map(str::to_string)));
    let inspector = session.inspector_form();
    ui.set_inspector_data(InspectorData {
        has_item: inspector.has_item,
        family: inspector.typography.family.into(),
        font_px: inspector.typography.font_px,
        bold: inspector.typography.bold,
        italic: inspector.typography.italic,
        color: inspector.typography.color.into(),
        align: inspector.typography.align.into(),
        x: inspector.item_box.left,
        y: inspector.item_box.top,
        w: inspector.item_box.width,
        h: inspector.item_box.height,
        background: inspector.item_box.background.into(),
        radius: inspector.item_box.radius,
        line_height: inspector.typography.line_height_pct,
        letter_spacing: inspector.typography.letter_spacing,
        outline_width: inspector.typography.outline_width,
        outline_color: inspector.typography.outline_color.into(),
        shadow_x: inspector.typography.shadow.x,
        shadow_y: inspector.typography.shadow.y,
        shadow_blur: inspector.typography.shadow.blur,
        shadow_color: inspector.typography.shadow.color.into(),
        blur: inspector.filters.blur,
        grayscale: inspector.filters.grayscale,
        sepia: inspector.filters.sepia,
        invert: inspector.filters.invert,
        brightness: inspector.filters.brightness,
        contrast: inspector.filters.contrast,
        saturate: inspector.filters.saturate,
        hue_rotate: inspector.filters.hue_rotate,
        opacity: inspector.filters.opacity,
        blend: inspector.filters.blend.into(),
        slide_transition: inspector.slide_transition.into(),
        slide_transition_ms: inspector.slide_transition_ms,
        valign: inspector.item_box.valign.into(),
        rotation: inspector.item_box.rotation,
        text_case: inspector.typography.text_case.into(),
    });
    ui.set_stage_layout_name(session.stage.name.clone().into());
    let rows: Vec<StageRowData> = session
        .stage_rows()
        .into_iter()
        .map(|row| StageRowData {
            id: row.id.into(),
            kind: row.kind.into(),
            x: row.left,
            y: row.top,
            w: row.width,
            h: row.height,
            font_px: row.font_px,
            color: row.color.into(),
            title: row.title.into(),
            align: row.align.into(),
            offset: row.offset,
            line_count: row.line_count,
            include_media: row.include_media,
            text_box: row.text_box,
            invert_items: row.invert_items,
            text: row.text.into(),
            seconds: row.seconds,
            date_format: row.date_format.into(),
            timer: row.timer.into(),
            src: row.src.into(),
            fit: row.fit.into(),
            tracker_type: row.tracker_type.into(),
            tracker_accent: row.tracker_accent.into(),
            variable: row.variable.into(),
        })
        .collect();
    ui.set_stage_rows(Rc::new(slint::VecModel::from(rows)).into());
    ui.set_stage_add_kinds(strings(&session::STAGE_ADD_KINDS.map(str::to_string)));
    ui.set_stage_add_names(strings(&session::STAGE_ADD_NAMES.map(str::to_string)));
    if ui.get_show_stage_editor() {
        ui.set_stage_preview(present::frame_image(&stage_frame(session)));
    }
    ui.set_stage_date_formats(strings(&session::STAGE_DATE_FORMATS.map(str::to_string)));
    let mut timer_ids = vec![String::new()];
    let mut timer_names = vec!["Whichever timer is running".to_string()];
    for (id, name) in session.timer_choices() {
        timer_ids.push(id);
        timer_names.push(name);
    }
    ui.set_stage_timer_ids(strings(&timer_ids));
    ui.set_stage_timer_names(strings(&timer_names));
    let mut variable_ids = vec![String::new()];
    let mut variable_names = vec!["Pick a variable".to_string()];
    for (id, name) in session.variable_choices() {
        variable_ids.push(id);
        variable_names.push(name);
    }
    ui.set_stage_variable_ids(strings(&variable_ids));
    ui.set_stage_variable_names(strings(&variable_names));
}

/// Closing an output or stage window with its title-bar button hides it and
/// remembers that it is closed, like pressing its header button.
fn remember_window_close(
    window: &slint::Window,
    ui: slint::Weak<ControlWindow>,
    settings: Rc<RefCell<AppSettings>>,
    settings_path: PathBuf,
    stage: bool,
) {
    window.on_close_requested(move || {
        {
            let mut settings = settings.borrow_mut();
            if stage {
                settings.show_stage = false;
            } else {
                settings.show_output = false;
            }
            if let Err(err) = settings.save(&settings_path) {
                tracing::error!("could not save settings: {err}");
            }
        }
        if let Some(ui) = ui.upgrade() {
            if stage {
                ui.set_stage_open(false);
            } else {
                ui.set_output_open(false);
            }
        }
        slint::CloseRequestResponse::HideWindow
    });
}

fn publish(slot: &FrameSlot, frame: Frame) {
    slot.publish(frame);
}

fn publish_audience(slot: &FrameSlot, frame: Frame) {
    submit_network(NetworkPicture::Audience, &frame);
    slot.publish(frame);
}

fn publish_stage(slot: &FrameSlot, frame: Frame) {
    submit_network(NetworkPicture::Stage, &frame);
    slot.publish(frame);
}

#[derive(Clone, Copy)]
enum NetworkPicture {
    Audience,
    Stage,
}

struct NdiBroadcast {
    audience: Vec<freeshow_ndi::NdiSender>,
    stage: Vec<freeshow_ndi::NdiSender>,
}

impl NdiBroadcast {
    fn open(session: &session::Session) -> Self {
        let mut audience = Vec::new();
        let mut stage = Vec::new();
        let mut opened = std::collections::BTreeSet::new();
        for target in session.ndi_outputs() {
            let source_name = target.grouped_name();
            let key = target
                .group
                .clone()
                .map(|group| format!("{}:{group}", target.stage))
                .unwrap_or_else(|| target.id.clone());
            if !opened.insert(key) {
                continue;
            }
            let sender = freeshow_ndi::NdiSender::open_routed(
                &source_name,
                session.output_width,
                session.output_height,
                &target.id,
            );
            if target.stage {
                stage.push(sender);
            } else {
                audience.push(sender);
            }
        }
        Self { audience, stage }
    }

    fn submit(&self, picture: NetworkPicture, pixels: &[u8], width: u32, height: u32) {
        let senders = match picture {
            NetworkPicture::Audience => &self.audience,
            NetworkPicture::Stage => &self.stage,
        };
        for sender in senders {
            sender.submit(pixels, width, height);
        }
    }
}

static NDI: std::sync::OnceLock<NdiBroadcast> = std::sync::OnceLock::new();

fn ndi() -> &'static NdiBroadcast {
    NDI.get_or_init(|| NdiBroadcast {
        audience: Vec::new(),
        stage: Vec::new(),
    })
}

struct WebrtcBroadcast {
    audience: Vec<freeshow_record::WebrtcSender>,
    stage: Vec<freeshow_record::WebrtcSender>,
}

impl WebrtcBroadcast {
    fn open(session: &session::Session) -> Self {
        let mut audience = Vec::new();
        let mut stage = Vec::new();
        for target in session.webrtc_outputs() {
            let Some(sender) = freeshow_record::WebrtcSender::open_routed(
                &target.url,
                target.token.as_deref(),
                session.output_width,
                session.output_height,
                &target.id,
            ) else {
                tracing::warn!("WebRTC output {} did not start", target.id);
                continue;
            };
            if target.stage {
                stage.push(sender);
            } else {
                audience.push(sender);
            }
        }
        Self { audience, stage }
    }

    fn submit(&self, picture: NetworkPicture, pixels: &[u8], width: u32, height: u32) {
        let senders = match picture {
            NetworkPicture::Audience => &self.audience,
            NetworkPicture::Stage => &self.stage,
        };
        for sender in senders {
            sender.submit(pixels, width, height);
        }
    }
}

static WEBRTC: std::sync::OnceLock<WebrtcBroadcast> = std::sync::OnceLock::new();

fn webrtc_out() -> &'static WebrtcBroadcast {
    WEBRTC.get_or_init(|| WebrtcBroadcast {
        audience: Vec::new(),
        stage: Vec::new(),
    })
}

fn submit_network(picture: NetworkPicture, frame: &Frame) {
    ndi().submit(picture, &frame.pixels, frame.width, frame.height);
    webrtc_out().submit(picture, &frame.pixels, frame.width, frame.height);
    if matches!(picture, NetworkPicture::Audience) {
        freeshow_record::ServiceCapture.submit(&frame.pixels, frame.width, frame.height);
        hardware().submit(&frame.pixels, frame.width, frame.height);
    }
}

struct HardwareBroadcast {
    camera: freeshow_record::OptionalSink,
    blackmagic: freeshow_record::OptionalSink,
    share: freeshow_record::OptionalSink,
}

impl HardwareBroadcast {
    fn open(session: &session::Session) -> Self {
        let width = session.output_width;
        let height = session.output_height;
        let camera = session
            .virtual_camera_device()
            .map(|device| {
                freeshow_record::OptionalSink::open(
                    &freeshow_record::virtual_camera_pipeline(&device, width, height),
                    width,
                    height,
                )
            })
            .unwrap_or_else(|| freeshow_record::OptionalSink::disabled(width, height));
        let blackmagic = session
            .blackmagic_device()
            .map(|device| {
                freeshow_record::OptionalSink::open(
                    &freeshow_record::blackmagic_pipeline(device, width, height),
                    width,
                    height,
                )
            })
            .unwrap_or_else(|| freeshow_record::OptionalSink::disabled(width, height));
        let share = session
            .capture_share_name()
            .and_then(|name| {
                let pipeline = freeshow_record::spout_syphon_pipeline(&name, width, height)?;
                Some(freeshow_record::OptionalSink::open(
                    &pipeline, width, height,
                ))
            })
            .unwrap_or_else(|| freeshow_record::OptionalSink::disabled(width, height));
        Self {
            camera,
            blackmagic,
            share,
        }
    }

    fn submit(&self, pixels: &[u8], width: u32, height: u32) {
        self.camera.submit(pixels, width, height);
        self.blackmagic.submit(pixels, width, height);
        self.share.submit(pixels, width, height);
    }
}

static HARDWARE: std::sync::OnceLock<HardwareBroadcast> = std::sync::OnceLock::new();

fn hardware() -> &'static HardwareBroadcast {
    HARDWARE.get_or_init(|| HardwareBroadcast {
        camera: freeshow_record::OptionalSink::disabled(1, 1),
        blackmagic: freeshow_record::OptionalSink::disabled(1, 1),
        share: freeshow_record::OptionalSink::disabled(1, 1),
    })
}

fn open_network_receive(session: &session::Session) -> Option<freeshow_ndi::NetworkReceiver> {
    let (kind, name) = session.network_receive()?;
    let receiver = match kind {
        freeshow_ndi::SourceKind::Ndi => freeshow_ndi::NetworkReceiver::open_ndi(
            &name,
            session.output_width,
            session.output_height,
        ),
        freeshow_ndi::SourceKind::Omt => freeshow_ndi::NetworkReceiver::open_omt(
            &name,
            session.output_width,
            session.output_height,
        ),
    };
    receiver.is_live().then_some(receiver)
}

fn fade_into(
    timer: Rc<slint::Timer>,
    slots: Vec<Rc<FrameSlot>>,
    from: Frame,
    to: Frame,
    picture: NetworkPicture,
    motion: freeshow_data::TextTransition,
) {
    if motion.kind == "none" {
        timer.stop();
        let frame = to;
        submit_network(picture, &frame);
        for slot in &slots {
            slot.publish(frame.clone());
        }
        return;
    }
    // Progress follows the clock, so a slow machine drops frames instead of stretching the fade.
    let duration = std::time::Duration::from_millis(motion.duration_ms.clamp(1, 10_000));
    let started = std::time::Instant::now();
    let timer_cb = Rc::clone(&timer);
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(40),
        move || {
            let progress = (started.elapsed().as_secs_f32() / duration.as_secs_f32()).min(1.0);
            let frame = motion_frame(&from, &to, &motion, progress);
            submit_network(picture, &frame);
            for slot in &slots {
                slot.publish(frame.clone());
            }
            if progress >= 1.0 {
                timer_cb.stop();
            }
        },
    );
}

fn sample_audio(session: &RefCell<Session>, audio: &RefCell<Option<AudioOutput>>) {
    let mut session = session.borrow_mut();
    if session.audio_change_pending() {
        return;
    }
    if let Some(output) = audio.borrow().as_ref() {
        session.audio_time = output.position_seconds();
        session.audio_duration = output.duration_seconds();
        session.audio_paused = output.is_paused();
        if output.is_finished()
            && let Some(path) = session.playing_audio.clone()
        {
            let _ = session.finish_media(&path.to_string_lossy(), true);
        }
    } else {
        session.audio_time = 0.0;
        session.audio_duration = 0.0;
    }
}

type LivePump = Rc<RefCell<Option<Rc<dyn Fn()>>>>;

/// Applies queued operator commands and output refreshes right away.
fn pump_live(pump: &LivePump) {
    let run = pump.borrow().clone();
    if let Some(run) = run {
        run();
    }
}

fn stop_video(timer: &slint::Timer, video: &RefCell<Option<VideoPlayer>>) {
    timer.stop();
    *video.borrow_mut() = None;
}

#[derive(Clone)]
struct LivePlayback {
    audio: Rc<RefCell<Option<AudioOutput>>>,
    video: Rc<RefCell<Option<VideoPlayer>>>,
    timer: Rc<slint::Timer>,
}

#[allow(clippy::too_many_arguments)]
fn start_pending(
    pending: Option<PendingPlay>,
    session: &Rc<RefCell<Session>>,
    playback: &LivePlayback,
    output_slot: &Rc<FrameSlot>,
    preview_slot: &Rc<FrameSlot>,
    stage_slot: &Rc<FrameSlot>,
    last_output: &Rc<RefCell<Frame>>,
    last_stage: &Rc<RefCell<Frame>>,
) {
    match pending {
        Some(PendingPlay::Audio(path)) => {
            *playback.audio.borrow_mut() = None;
            let (routes, looping, from, to, tempo) = {
                let current = session.borrow();
                let (from, to) = current.audio_span();
                (
                    current.audio_routes(),
                    current.audio_should_loop(),
                    from,
                    to,
                    current.audio_tempo(),
                )
            };
            let network_routes = session.borrow().audio_network_routes();
            match AudioOutput::play_file_configured(
                &path,
                &routes,
                &network_routes,
                &AudioPlaybackOptions {
                    looping,
                    from_seconds: from,
                    to_seconds: to,
                    rate: tempo,
                    effects: freeshow_media::effects_from_value(
                        &session.borrow().preferences.audio_effects,
                    ),
                },
            ) {
                Ok(output) => {
                    *playback.audio.borrow_mut() = Some(output);
                }
                Err(err) => {
                    session.borrow_mut().timeline_audio_failed(&path);
                    tracing::error!("could not play audio: {err}");
                }
            }
        }
        Some(PendingPlay::Video(path)) => {
            stop_video(&playback.timer, &playback.video);
            let ended = path.clone();
            let routes = session.borrow().video_audio_routes();
            let network_routes = session.borrow().video_network_routes();
            match VideoPlayer::open_with_network(&path, &routes, &network_routes) {
                Ok(player) => {
                    player.set_looping(session.borrow().media_loop);
                    player.set_muted(session.borrow().media_muted);
                    player.set_volume(1.0);
                    player.set_rate(session.borrow().video_rate);
                    let (from, to) = {
                        let current = session.borrow();
                        (current.video_from, current.video_to)
                    };
                    player.set_span(from, to);
                    follow_video(
                        Rc::clone(&playback.timer),
                        Rc::clone(&playback.video),
                        player,
                        Some(ended),
                        Rc::clone(session),
                        Rc::clone(output_slot),
                        Rc::clone(preview_slot),
                        Rc::clone(stage_slot),
                        Rc::clone(last_output),
                        Rc::clone(last_stage),
                    );
                }
                Err(err) => tracing::error!("could not play video: {err}"),
            }
        }
        Some(PendingPlay::Camera(name)) => {
            stop_video(&playback.timer, &playback.video);
            match VideoPlayer::open_camera(&name) {
                Ok(player) => follow_video(
                    Rc::clone(&playback.timer),
                    Rc::clone(&playback.video),
                    player,
                    None,
                    Rc::clone(session),
                    Rc::clone(output_slot),
                    Rc::clone(preview_slot),
                    Rc::clone(stage_slot),
                    Rc::clone(last_output),
                    Rc::clone(last_stage),
                ),
                Err(err) => tracing::error!("could not open camera: {err}"),
            }
        }
        Some(PendingPlay::StopAudio) => {
            *playback.audio.borrow_mut() = None;
        }
        Some(PendingPlay::StopVideo) => {
            stop_video(&playback.timer, &playback.video);
        }
        Some(PendingPlay::PauseAudio) => {
            if let Some(output) = playback.audio.borrow().as_ref() {
                output.set_paused(true);
            }
        }
        Some(PendingPlay::ResumeAudio) => {
            if let Some(output) = playback.audio.borrow().as_ref() {
                output.set_paused(false);
            }
        }
        Some(PendingPlay::SeekAudio(millis)) => {
            if let Some(output) = playback.audio.borrow().as_ref() {
                output.seek_seconds(millis as f64 / 1000.0);
            }
        }
        Some(PendingPlay::SeekVideo(millis)) => {
            if let Some(player) = playback.video.borrow().as_ref() {
                let _ = player.seek_seconds(millis as f64 / 1000.0);
            }
        }
        Some(PendingPlay::SetMute) => {
            if let Some(player) = playback.video.borrow().as_ref() {
                player.set_muted(session.borrow().media_muted);
            }
        }
        Some(PendingPlay::SetLoop) => {
            if let Some(player) = playback.video.borrow().as_ref() {
                player.set_looping(session.borrow().media_loop);
            }
        }
        Some(PendingPlay::ToggleMedia) => {
            let paused = !session.borrow().media_paused;
            if let Some(player) = playback.video.borrow().as_ref() {
                player.set_paused(paused);
                session.borrow_mut().media_paused = paused;
            }
        }
        None => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn start_plays(
    plays: [Option<PendingPlay>; 2],
    session: &Rc<RefCell<Session>>,
    playback: &LivePlayback,
    output_slot: &Rc<FrameSlot>,
    preview_slot: &Rc<FrameSlot>,
    stage_slot: &Rc<FrameSlot>,
    last_output: &Rc<RefCell<Frame>>,
    last_stage: &Rc<RefCell<Frame>>,
) {
    for play in plays {
        start_pending(
            play,
            session,
            playback,
            output_slot,
            preview_slot,
            stage_slot,
            last_output,
            last_stage,
        );
    }
    if let Some(output) = playback.audio.borrow().as_ref() {
        output.set_route_gains(&session.borrow().audio_routes());
        output.set_network_routes(&session.borrow().audio_network_routes());
    }
    if let Some(player) = playback.video.borrow().as_ref() {
        player.set_audio_routes(&session.borrow().video_audio_routes());
        player.set_network_routes(&session.borrow().video_network_routes());
    }
}

fn picture_fades_in(pending: &Option<PendingPlay>, session: &Session) -> bool {
    session.background_transition
        && matches!(
            pending,
            Some(PendingPlay::Video(_)) | Some(PendingPlay::Camera(_))
        )
}

#[allow(clippy::too_many_arguments)]
fn follow_video(
    timer: Rc<slint::Timer>,
    player: Rc<RefCell<Option<VideoPlayer>>>,
    opened: VideoPlayer,
    ended: Option<PathBuf>,
    session: Rc<RefCell<Session>>,
    output_slot: Rc<FrameSlot>,
    preview_slot: Rc<FrameSlot>,
    stage_slot: Rc<FrameSlot>,
    last_output: Rc<RefCell<Frame>>,
    last_stage: Rc<RefCell<Frame>>,
) {
    *player.borrow_mut() = Some(opened);
    let opening = {
        let mut current = session.borrow_mut();
        if current.background_transition {
            let (motion, _) = current.output_motion();
            Some(OpeningFade::new(last_output.borrow().clone(), motion))
        } else {
            None
        }
    };
    let opening = Rc::new(RefCell::new(opening));
    let started = std::time::Instant::now();
    let player_tick = Rc::clone(&player);
    let opening_tick = Rc::clone(&opening);
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(33),
        move || {
            let timing = player_tick
                .borrow()
                .as_ref()
                .map(|player| (player.position_seconds(), player.duration_seconds()));
            let raster = player_tick
                .borrow_mut()
                .as_mut()
                .and_then(|player| player.next_frame());
            let Some(raster) = raster else {
                let mut session = session.borrow_mut();
                if let Some((time, duration)) = timing {
                    session.video_time = time;
                    session.video_duration = duration;
                }
                let finished = player_tick
                    .borrow()
                    .as_ref()
                    .is_some_and(|player| player.is_finished());
                if finished && let Some(path) = ended.as_deref() {
                    let _ = session.finish_media(&path.to_string_lossy(), false);
                }
                return;
            };
            let (audience, stage) = {
                let mut session = session.borrow_mut();
                if let Some((time, duration)) = timing {
                    session.video_time = time;
                    session.video_duration = duration;
                }
                session.replace_underlay(Some(raster));
                (audience_frame(&session), stage_frame(&session))
            };
            let shown = {
                let mut fade = opening_tick.borrow_mut();
                if let Some(current) = fade.as_ref() {
                    match current.frame(&audience, started.elapsed().as_millis()) {
                        Some(blended) => blended,
                        None => {
                            *fade = None;
                            audience.clone()
                        }
                    }
                } else {
                    audience.clone()
                }
            };
            *last_output.borrow_mut() = audience;
            *last_stage.borrow_mut() = stage.clone();
            publish_audience(&output_slot, shown.clone());
            publish(&preview_slot, shown);
            publish_stage(&stage_slot, stage);
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn send_frames(
    output_fade: &Rc<slint::Timer>,
    stage_fade: &Rc<slint::Timer>,
    output_slot: &Rc<FrameSlot>,
    preview_slot: &Rc<FrameSlot>,
    stage_slot: &Rc<FrameSlot>,
    last_output: &Rc<RefCell<Frame>>,
    last_stage: &Rc<RefCell<Frame>>,
    audience: Frame,
    stage: Frame,
    motion: freeshow_data::TextTransition,
    background: bool,
    next_timer: &Rc<slint::Timer>,
    session: &Rc<RefCell<Session>>,
    ui: slint::Weak<ControlWindow>,
    seconds: Option<f64>,
    playback: &LivePlayback,
) {
    let from_output = last_output.borrow().clone();
    *last_output.borrow_mut() = audience.clone();
    fade_into(
        Rc::clone(output_fade),
        vec![Rc::clone(output_slot), Rc::clone(preview_slot)],
        from_output,
        audience,
        NetworkPicture::Audience,
        motion.clone(),
    );
    let from_stage = last_stage.borrow().clone();
    *last_stage.borrow_mut() = stage.clone();
    let stage_motion = if background {
        freeshow_data::TextTransition {
            kind: "none".into(),
            duration_ms: 0,
            ..freeshow_data::TextTransition::default()
        }
    } else {
        motion
    };
    fade_into(
        Rc::clone(stage_fade),
        vec![Rc::clone(stage_slot)],
        from_stage,
        stage,
        NetworkPicture::Stage,
        stage_motion,
    );
    if !background {
        schedule_next_slide(
            next_timer,
            session,
            output_fade,
            stage_fade,
            output_slot,
            preview_slot,
            stage_slot,
            last_output,
            last_stage,
            ui,
            seconds,
            playback,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn schedule_next_slide(
    timer: &Rc<slint::Timer>,
    session: &Rc<RefCell<Session>>,
    output_fade: &Rc<slint::Timer>,
    stage_fade: &Rc<slint::Timer>,
    output_slot: &Rc<FrameSlot>,
    preview_slot: &Rc<FrameSlot>,
    stage_slot: &Rc<FrameSlot>,
    last_output: &Rc<RefCell<Frame>>,
    last_stage: &Rc<RefCell<Frame>>,
    ui: slint::Weak<ControlWindow>,
    seconds: Option<f64>,
    playback: &LivePlayback,
) {
    timer.stop();
    let Some(seconds) = seconds.filter(|_| !session.borrow().has_pending_cue_completion()) else {
        return;
    };
    let delay = (seconds * 1000.0).round().clamp(1.0, 3_600_000.0) as u64;
    let timer_cb = Rc::clone(timer);
    let session = Rc::clone(session);
    let output_fade = Rc::clone(output_fade);
    let stage_fade = Rc::clone(stage_fade);
    let output_slot = Rc::clone(output_slot);
    let preview_slot = Rc::clone(preview_slot);
    let stage_slot = Rc::clone(stage_slot);
    let last_output = Rc::clone(last_output);
    let last_stage = Rc::clone(last_stage);
    let playback = playback.clone();
    timer.start(
        slint::TimerMode::SingleShot,
        std::time::Duration::from_millis(delay),
        move || {
            let mut current = session.borrow_mut();
            if current.has_pending_cue_completion() || !current.advance_show() {
                return;
            }
            let plays = current.take_plays();
            if picture_fades_in(&plays[0], &current) {
                drop(current);
                start_plays(
                    plays,
                    &session,
                    &playback,
                    &output_slot,
                    &preview_slot,
                    &stage_slot,
                    &last_output,
                    &last_stage,
                );
                if let Some(ui) = ui.upgrade() {
                    refresh(&ui, &session.borrow());
                }
                return;
            }
            if current.background_transition {
                stop_video(&playback.timer, &playback.video);
            }
            let audience = audience_frame(&current);
            let stage = stage_frame(&current);
            let (motion, background) = current.output_motion();
            drop(current);
            send_frames(
                &output_fade,
                &stage_fade,
                &output_slot,
                &preview_slot,
                &stage_slot,
                &last_output,
                &last_stage,
                audience,
                stage,
                motion,
                background,
                &timer_cb,
                &session,
                ui.clone(),
                session.borrow().next_timer_seconds(),
                &playback,
            );
            start_plays(
                plays,
                &session,
                &playback,
                &output_slot,
                &preview_slot,
                &stage_slot,
                &last_output,
                &last_stage,
            );
            if let Some(ui) = ui.upgrade() {
                refresh(&ui, &session.borrow());
            }
        },
    );
}

fn place_output(window: &slint::Window, saved: Option<usize>) {
    let placed = window.with_winit_window(|winit_window| {
        let monitors: Vec<_> = winit_window.available_monitors().collect();
        let Some(index) = freeshow_core::output_monitor_index(saved, monitors.len()) else {
            return false;
        };
        let Some(monitor) = monitors.get(index) else {
            return false;
        };
        let pos = monitor.position();
        winit_window.set_outer_position(slint::winit_030::winit::dpi::PhysicalPosition::new(
            pos.x, pos.y,
        ));
        true
    });
    if placed != Some(true) {
        place(window, 80, 80);
    }
}

fn monitor_count(window: &slint::Window) -> usize {
    window
        .with_winit_window(|winit_window| winit_window.available_monitors().count())
        .unwrap_or(0)
}

fn screen_label(index: Option<usize>, outputs: &[freeshow_data::OutputPreference]) -> String {
    let screen = match index {
        Some(index) => format!("Screen {}", index + 1),
        None => "Screen".to_string(),
    };
    let name = outputs
        .iter()
        .find(|output| output.active && output.enabled)
        .map(|output| output.name.as_str())
        .filter(|name| !name.is_empty());
    match name {
        Some(name) => format!("{screen} · {name}"),
        None => screen,
    }
}

fn apply_chrome(ui: &ControlWindow, session: &Session, output_screen: Option<usize>) {
    ui.set_show_labels(!session.preferences.labels_disabled());
    ui.set_screen_label(screen_label(output_screen, &session.preferences.outputs).into());
    ui.set_drawer_width(session.preferences.left_panel_width() as i32);
    ui.set_project_width(session.preferences.right_panel_width() as i32);
    ui.set_drawer_height(session.preferences.drawer_height() as i32);
    ui.set_focus_mode(session.focus_mode);
    ui.set_guide_visible(session.guide_visible);
    ui.set_guide_text(session.guide_text().into());
    let tabs: Vec<DrawerTab> = lang::drawer_tabs(session.preferences.language.as_deref())
        .into_iter()
        .map(|(id, label, short)| DrawerTab {
            id: id.into(),
            label: label.into(),
            short: short.into(),
        })
        .collect();
    ui.set_tabs(slint::ModelRc::new(slint::VecModel::from(tabs)));
}

fn apply_window_behavior(window: &slint::Window, always_on_top: bool, kiosk: bool) {
    let _ = window.with_winit_window(|winit_window| {
        winit_window.set_window_level(if always_on_top {
            slint::winit_030::winit::window::WindowLevel::AlwaysOnTop
        } else {
            slint::winit_030::winit::window::WindowLevel::Normal
        });
        winit_window.set_decorations(!kiosk);
        winit_window.set_fullscreen(if kiosk {
            Some(slint::winit_030::winit::window::Fullscreen::Borderless(
                None,
            ))
        } else {
            None
        });
    });
}

fn capture_placement(window: &slint::Window) -> Option<WindowPlacement> {
    window
        .with_winit_window(|winit_window| {
            let pos = winit_window.outer_position().ok()?;
            let size = winit_window.inner_size();
            Some(WindowPlacement::new(pos.x, pos.y, size.width, size.height))
        })
        .flatten()
}

fn apply_placement(window: &slint::Window, place: &WindowPlacement) {
    let _ = window.with_winit_window(|winit_window| {
        winit_window.set_outer_position(slint::winit_030::winit::dpi::PhysicalPosition::new(
            place.x, place.y,
        ));
        let _ = winit_window.request_inner_size(slint::winit_030::winit::dpi::PhysicalSize::new(
            place.width,
            place.height,
        ));
    });
}

fn apply_window_profile(
    control: &slint::Window,
    output: &slint::Window,
    stage: &slint::Window,
    settings: &AppSettings,
) {
    let Some(profile) = settings.active_profile() else {
        return;
    };
    if let Some(place) = &profile.control {
        apply_placement(control, place);
    }
    if let Some(place) = &profile.output {
        apply_placement(output, place);
    }
    if let Some(place) = &profile.stage {
        apply_placement(stage, place);
    }
}

fn capture_window_profile(
    control: &slint::Window,
    output: &slint::Window,
    stage: &slint::Window,
) -> WindowProfile {
    WindowProfile {
        control: capture_placement(control),
        output: capture_placement(output),
        stage: capture_placement(stage),
    }
}

fn place_from_profile_or(
    window: &slint::Window,
    place: Option<&WindowPlacement>,
    fallback: impl FnOnce(),
) {
    if let Some(place) = place {
        apply_placement(window, place);
    } else {
        fallback();
    }
}

fn place(window: &slint::Window, x: i32, y: i32) {
    let moved = window.with_winit_window(|winit_window| {
        winit_window.set_outer_position(slint::winit_030::winit::dpi::PhysicalPosition::new(x, y));
    });
    if moved.is_none() {
        tracing::debug!("window was not ready to move");
    }
}

struct Launch {
    data: Option<PathBuf>,
    show: Option<PathBuf>,
    check_runtime: bool,
    install_desktop: bool,
    uninstall_desktop: bool,
    check_media: Option<PathBuf>,
    check_perf: Option<Option<PathBuf>>,
    check_library: Option<Option<PathBuf>>,
    check_service: Option<Option<PathBuf>>,
    list_audio_devices: bool,
    list_video_audio_devices: bool,
}

fn launch_args() -> Result<Launch, Box<dyn std::error::Error>> {
    let mut launch = Launch {
        data: None,
        show: None,
        check_runtime: false,
        install_desktop: false,
        uninstall_desktop: false,
        check_media: None,
        check_perf: None,
        check_library: None,
        check_service: None,
        list_audio_devices: false,
        list_video_audio_devices: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--install-desktop" => launch.install_desktop = true,
            "--uninstall-desktop" => launch.uninstall_desktop = true,
            "--check-runtime" => launch.check_runtime = true,
            "--check-perf" => {
                launch.check_perf = Some(args.next().map(PathBuf::from));
            }
            "--check-library" => {
                launch.check_library = Some(args.next().map(PathBuf::from));
            }
            "--check-service" => {
                launch.check_service = Some(args.next().map(PathBuf::from));
            }
            "--check-media" => {
                launch.check_media = Some(PathBuf::from(
                    args.next().ok_or("--check-media needs a video file")?,
                ))
            }
            "--data" => launch.data = args.next().map(PathBuf::from),
            "--show" => launch.show = args.next().map(PathBuf::from),
            "--list-audio-devices" => launch.list_audio_devices = true,
            "--list-video-audio-devices" => launch.list_video_audio_devices = true,
            _ => {}
        }
    }
    Ok(launch)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    runtime::configure_bundle()?;
    let launch = launch_args()?;
    if launch.install_desktop || launch.uninstall_desktop {
        return desktop::install(launch.uninstall_desktop);
    }
    if let Some(path) = launch.check_perf {
        return service_check::check_perf(path.as_deref());
    }
    if let Some(path) = launch.check_library {
        return service_check::check_library(path.as_deref());
    }
    if let Some(path) = launch.check_service {
        return service_check::check_service(path.as_deref());
    }
    if launch.check_runtime || launch.check_media.is_some() {
        return runtime::check(launch.check_media.as_deref());
    }
    if launch.list_video_audio_devices {
        for device in freeshow_media::video_output_devices() {
            println!("{device}");
        }
        return Ok(());
    }
    if launch.list_audio_devices {
        for device in freeshow_media::output_devices() {
            println!("{device}");
        }
        return Ok(());
    }
    let config_dir = freeshow_core::config_dir();
    let log_path = freeshow_core::init_logging(&config_dir)?;
    tracing::info!("log {}", log_path.display());

    let settings_path = config_dir.join("settings.json");
    let mut settings = match AppSettings::load(&settings_path) {
        Ok(settings) => settings,
        Err(err) => {
            tracing::error!("settings unreadable ({err}); using defaults");
            AppSettings::default()
        }
    };
    if settings.data_path.is_none() {
        settings.data_path = freeshow_data::default_library_dir();
    }
    if let Some(path) = &launch.data {
        settings.data_path = Some(path.clone());
    }
    if let Err(err) = settings.save(&settings_path) {
        tracing::error!("could not create settings file: {err}");
    }

    present::select_renderer()?;

    let mut session = if let Some(root) = settings.data_path.clone() {
        let planned = freeshow_pco::PlanningCenter.sync(&root);
        if std::env::var("FREESHOW_CLOUD_DIR")
            .ok()
            .is_some_and(|value| !value.trim().is_empty())
            || std::env::var("FREESHOW_CLOUD_URL")
                .ok()
                .is_some_and(|value| !value.trim().is_empty())
        {
            let _ = freeshow_cloud::LibraryCloud.backup(&root);
        }
        let mut session = Session::open(root);
        if let Some(id) = planned.project_id {
            session.project_id = Some(id);
            session.project_item = 0;
        }
        session
    } else {
        Session::empty()
    };
    if let Some(obs) = freeshow_obs::ObsOutput::from_settings(&session.preferences.obs_data)
        && let Ok(scene) = std::env::var("FREESHOW_OBS_SCENE")
        && !scene.trim().is_empty()
    {
        let scene = scene.trim().to_string();
        std::thread::spawn(move || obs.set_scene(&scene));
    }
    for warning in &session.warnings {
        tracing::warn!("{warning}");
    }
    let _ = NDI.set(NdiBroadcast::open(&session));
    let _ = HARDWARE.set(HardwareBroadcast::open(&session));
    let _network_in = open_network_receive(&session);
    let _ = WEBRTC.set(WebrtcBroadcast::open(&session));
    for target in session.omt_outputs() {
        tracing::warn!(
            "OMT output {} ({}) is configured, but the Open Media Transport runtime is not bundled",
            target.id,
            target.name
        );
    }
    session.audio_device_map = settings.audio_devices.clone();
    let theme = theme::operator_theme(settings.theme.as_deref(), &session.preferences);
    if let Some(path) = &launch.show
        && let Some(show) = session::load_show_file(path)
    {
        let id = show.id.clone();
        if let Some(index) = session.shows.iter().position(|existing| existing.id == id) {
            session.show_index = Some(index);
        } else {
            session.shows.insert(0, show);
            session.show_index = Some(0);
        }
        session.slide_index = Some(0);
        session.tab = "shows".to_string();
    }

    let control = ControlWindow::new()?;
    control.global::<ComboIndex>().on_find(|model, value| {
        use slint::Model;
        model
            .iter()
            .position(|row| row == value)
            .map_or(0, |index| index as i32)
    });
    let startup_import = session
        .root
        .clone()
        .and_then(|root| match ChurchImport::start(root) {
            Ok(job) => Some(job),
            Err(error) => {
                tracing::error!("could not start ChurchApps import: {error}");
                None
            }
        });
    let church_import = Rc::new(RefCell::new(startup_import));
    {
        let church_import = Rc::clone(&church_import);
        control.on_cancel_import(move || {
            if let Some(job) = church_import.borrow().as_ref() {
                job.control.cancel();
            }
        });
    }

    apply_theme(&control, &theme);
    let data_label = settings
        .data_path
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "Data folder not set".to_string());
    control.set_data_path(data_label.into());
    control.set_active_tab(session.tab.clone().into());
    control.set_panel_title("Shows".into());
    refresh(&control, &session);
    apply_chrome(&control, &session, settings.output_screen);
    let output_width = session.output_width;
    let output_height = session.output_height;

    let output = OutputWindow::new()?;
    let stage = StageWindow::new()?;
    let chosen =
        freeshow_core::output_monitor_index(settings.output_screen, monitor_count(output.window()));
    apply_chrome(&control, &session, chosen);
    let output_weak = output.as_weak();
    let output_redraw = output.as_weak();
    let output_slot = attach_frame(
        output.window(),
        clear_frame(output_width, output_height),
        move |image| {
            if let Some(output) = output_weak.upgrade() {
                output.set_frame(image);
            }
        },
        move || {
            if let Some(output) = output_redraw.upgrade() {
                output.window().request_redraw();
            }
        },
    )?;
    let stage_weak = stage.as_weak();
    let stage_redraw = stage.as_weak();
    let stage_slot = attach_frame(
        stage.window(),
        clear_frame(output_width, output_height),
        move |image| {
            if let Some(stage) = stage_weak.upgrade() {
                stage.set_frame(image);
            }
        },
        move || {
            if let Some(stage) = stage_redraw.upgrade() {
                stage.window().request_redraw();
            }
        },
    )?;
    let preview_weak = control.as_weak();
    let preview_redraw = control.as_weak();
    let preview_slot = attach_frame(
        control.window(),
        clear_frame(output_width, output_height),
        move |image| {
            if let Some(control) = preview_weak.upgrade() {
                control.set_preview(image);
            }
        },
        move || {
            if let Some(control) = preview_redraw.upgrade() {
                control.window().request_redraw();
            }
        },
    )?;

    // The slide being edited, shown in place of the live picture while editing.
    let edit_weak = control.as_weak();
    let edit_redraw = control.as_weak();
    let edit_slot = Rc::new(attach_frame(
        control.window(),
        clear_frame(output_width, output_height),
        move |image| {
            if let Some(control) = edit_weak.upgrade() {
                control.set_edit_preview(image);
            }
        },
        move || {
            if let Some(control) = edit_redraw.upgrade() {
                control.window().request_redraw();
            }
        },
    )?);
    let output_slot = Rc::new(output_slot);
    let stage_slot = Rc::new(stage_slot);
    let preview_slot = Rc::new(preview_slot);
    let output_fade = Rc::new(slint::Timer::default());
    let stage_fade = Rc::new(slint::Timer::default());
    let next_slide = Rc::new(slint::Timer::default());
    let last_output = Rc::new(RefCell::new(clear_frame(output_width, output_height)));
    let last_stage = Rc::new(RefCell::new(clear_frame(output_width, output_height)));
    {
        // Even an empty output starts in the style's background colour.
        let audience = audience_frame(&session);
        let stage = stage_frame(&session);
        *last_output.borrow_mut() = audience.clone();
        *last_stage.borrow_mut() = stage.clone();
        publish_audience(&output_slot, audience.clone());
        publish(&preview_slot, audience);
        publish_stage(&stage_slot, stage);
    }
    let _ = ndi();

    if settings.show_output || session.preferences.auto_output() {
        output.show()?;
        place_output(output.window(), settings.output_screen);
        let (on_top, kiosk) = session.output_window_flags();
        apply_window_behavior(output.window(), on_top, kiosk);
    }
    if settings.show_stage {
        stage.show()?;
        place(stage.window(), 120, 140);
    }

    let session = Rc::new(RefCell::new(session));
    // Runs one live-update pass now instead of waiting for the next 100 ms tick.
    let live_pump: LivePump = Rc::default();
    let settings = Rc::new(RefCell::new(settings));
    let audio_out: Rc<RefCell<Option<AudioOutput>>> = Rc::new(RefCell::new(None));
    let video_player: Rc<RefCell<Option<VideoPlayer>>> = Rc::new(RefCell::new(None));
    let video_timer = Rc::new(slint::Timer::default());
    let metronome_timer = Rc::new(slint::Timer::default());
    {
        let session = Rc::clone(&session);
        let ui = control.as_weak();
        metronome_timer.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(50),
            move || {
                let Some(ui) = ui.upgrade() else {
                    return;
                };
                let mut current = session.borrow_mut();
                let now = std::time::Instant::now();
                let timeline = current.tick_timeline(now);
                current.refresh_output |= timeline;
                if current.metronome_on {
                    current.update_metronome(now);
                }
                if timeline || current.metronome_on {
                    let status = current.chrome_status();
                    drop(current);
                    if timeline {
                        refresh(&ui, &session.borrow());
                    } else {
                        ui.set_chrome_status(status.into());
                    }
                }
            },
        );
    }

    control.set_output_open(output.window().is_visible());
    control.set_stage_open(stage.window().is_visible());
    remember_window_close(
        output.window(),
        control.as_weak(),
        Rc::clone(&settings),
        settings_path.clone(),
        false,
    );
    remember_window_close(
        stage.window(),
        control.as_weak(),
        Rc::clone(&settings),
        settings_path.clone(),
        true,
    );
    let output_toggle = output.as_weak();
    let settings_for_output = Rc::clone(&settings);
    let session_for_output_flags = Rc::clone(&session);
    let settings_path_for_output = settings_path.clone();
    let ui_for_output = control.as_weak();
    control.on_open_output(move || {
        let Some(output) = output_toggle.upgrade() else {
            return;
        };
        let visible = output.window().is_visible();
        if visible {
            let _ = output.hide();
        } else if output.show().is_ok() {
            let settings = settings_for_output.borrow();
            place_from_profile_or(
                output.window(),
                settings.active_profile().and_then(|p| p.output.as_ref()),
                || place_output(output.window(), settings.output_screen),
            );
            let (on_top, kiosk) = session_for_output_flags.borrow().output_window_flags();
            apply_window_behavior(output.window(), on_top, kiosk);
        }
        settings_for_output.borrow_mut().show_output = !visible;
        if let Some(ui) = ui_for_output.upgrade() {
            ui.set_output_open(output.window().is_visible());
        }
        if let Err(err) = settings_for_output.borrow().save(&settings_path_for_output) {
            tracing::error!("could not save settings: {err}");
        }
    });

    {
        let output_for_screen = output.as_weak();
        let settings_for_screen = Rc::clone(&settings);
        let settings_path_for_screen = settings_path.clone();
        let session_for_screen = Rc::clone(&session);
        let ui = control.as_weak();
        control.on_choose_screen(move || {
            let Some(output) = output_for_screen.upgrade() else {
                return;
            };
            let count = monitor_count(output.window());
            let next = freeshow_core::next_output_monitor(
                settings_for_screen.borrow().output_screen,
                count,
            );
            settings_for_screen.borrow_mut().output_screen = next;
            if output.window().is_visible() {
                place_output(output.window(), next);
            }
            if let Some(ui) = ui.upgrade() {
                apply_chrome(&ui, &session_for_screen.borrow(), next);
            }
            if let Err(err) = settings_for_screen.borrow().save(&settings_path_for_screen) {
                tracing::error!("could not save settings: {err}");
            }
        });
    }

    let stage_toggle = stage.as_weak();
    let settings_for_stage = Rc::clone(&settings);
    let settings_path_for_stage = settings_path.clone();
    let ui_for_stage = control.as_weak();
    control.on_open_stage(move || {
        let Some(stage) = stage_toggle.upgrade() else {
            return;
        };
        let visible = stage.window().is_visible();
        if visible {
            let _ = stage.hide();
        } else if stage.show().is_ok() {
            place_from_profile_or(
                stage.window(),
                settings_for_stage
                    .borrow()
                    .active_profile()
                    .and_then(|p| p.stage.as_ref()),
                || place(stage.window(), 120, 140),
            );
        }
        settings_for_stage.borrow_mut().show_stage = !visible;
        if let Some(ui) = ui_for_stage.upgrade() {
            ui.set_stage_open(stage.window().is_visible());
        }
        if let Err(err) = settings_for_stage.borrow().save(&settings_path_for_stage) {
            tracing::error!("could not save settings: {err}");
        }
    });

    let ui = control.as_weak();
    let session_for_tab = Rc::clone(&session);
    control.on_select_tab(move |tab| {
        session_for_tab.borrow_mut().switch_tab(tab.as_str());
        if let Some(ui) = ui.upgrade() {
            refresh(&ui, &session_for_tab.borrow());
            fill_drawer_form(&ui, &session_for_tab.borrow());
        }
    });

    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        control.on_set_search(move |query| {
            session.borrow_mut().set_search(query.to_string());
            if let Some(ui) = ui.upgrade() {
                refresh(&ui, &session.borrow());
            }
        });
    }

    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        control.on_dismiss_guide(move || {
            session.borrow_mut().dismiss_guide();
            if let Some(ui) = ui.upgrade() {
                refresh(&ui, &session.borrow());
            }
        });
    }

    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        let settings = Rc::clone(&settings);
        let edit_slot = Rc::clone(&edit_slot);
        let output_window = output.as_weak();
        control.on_apply_setting(move |key, value| {
            let result = session.borrow_mut().apply_setting(&key, &value);
            let Some(ui) = ui.upgrade() else {
                return;
            };
            match result {
                Ok(()) => {
                    ui.set_settings_status(
                        format!("Saved {}", session::setting_label(&key)).into(),
                    );
                    let current = session.borrow();
                    if key.starts_with("output")
                        && let Some(output) = output_window.upgrade()
                        && output.window().is_visible()
                    {
                        let (on_top, kiosk) = current.output_window_flags();
                        apply_window_behavior(output.window(), on_top, kiosk);
                    }
                    if key.starts_with("theme") {
                        let theme = theme::operator_theme(
                            settings.borrow().theme.as_deref(),
                            &current.preferences,
                        );
                        apply_theme(&ui, &theme);
                    }
                    // Labels, language and output names in the header follow at once.
                    apply_chrome(&ui, &current, settings.borrow().output_screen);
                    refresh(&ui, &current);
                    publish(&edit_slot, edit_frame(&current));
                }
                Err(err) => {
                    tracing::warn!("setting {key} not saved: {err}");
                    ui.set_settings_status(err.into());
                    refresh_panels(&ui, &session.borrow());
                }
            }
        });
    }
    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        let edit_slot = Rc::clone(&edit_slot);
        control.on_inspector_set(move |key, value| {
            let result = session.borrow_mut().inspector_set(&key, &value);
            if let Err(err) = &result {
                tracing::warn!("inspector {key} not applied: {err}");
            }
            if let Some(ui) = ui.upgrade() {
                let current = session.borrow();
                refresh(&ui, &current);
                if result.is_ok() {
                    publish(&edit_slot, edit_frame(&current));
                }
            }
        });
    }
    {
        let ui = control.as_weak();
        let stage_session = Rc::clone(&session);
        let stage_slot = Rc::clone(&stage_slot);
        let after_stage_edit = move |result: Result<String, String>| {
            let Some(ui) = ui.upgrade() else {
                return;
            };
            let current = stage_session.borrow();
            match result {
                Ok(message) => {
                    ui.set_stage_status(message.into());
                    publish_stage(&stage_slot, stage_frame(&current));
                }
                Err(err) => {
                    tracing::warn!("stage editor: {err}");
                    ui.set_stage_status(err.into());
                }
            }
            refresh_panels(&ui, &current);
        };
        let after_stage_edit = Rc::new(after_stage_edit);
        {
            let session = Rc::clone(&session);
            let done = Rc::clone(&after_stage_edit);
            control.on_stage_set(move |id, key, value| {
                let result = session.borrow_mut().stage_set(&id, &key, &value);
                let name = session.borrow().stage_item_name(&id);
                done(result.map(|()| format!("Saved {name}")));
            });
        }
        {
            let session = Rc::clone(&session);
            let done = Rc::clone(&after_stage_edit);
            let ui = control.as_weak();
            control.on_stage_add(move |kind| {
                // Each new box lands a little lower and to the right so it never hides the last one.
                let step = 60 + 40 * (session.borrow().stage.items.len() % 10);
                let style =
                    format!("left:{step}px;top:{step}px;width:600px;height:200px;font-size:80px;");
                let added = session.borrow_mut().add_stage_item(&kind, &style);
                if let (Some(ui), Some(_)) = (ui.upgrade(), added.as_ref()) {
                    ui.set_stage_selected(session.borrow().stage.items.len() as i32 - 1);
                }
                let named = added
                    .as_ref()
                    .map(|id| session.borrow().stage_item_name(id));
                done(
                    named
                        .map(|name| format!("Added {name}"))
                        .ok_or_else(|| "could not save stage.json".to_string()),
                );
            });
        }
        {
            let session = Rc::clone(&session);
            let done = Rc::clone(&after_stage_edit);
            control.on_stage_remove(move |id| {
                let name = session.borrow().stage_item_name(&id);
                let removed = session.borrow_mut().remove_stage_item(&id);
                done(if removed {
                    Ok(format!("Removed {name}"))
                } else {
                    Err(format!("could not remove {name}"))
                });
            });
        }
        {
            let session = Rc::clone(&session);
            let ui = control.as_weak();
            control.on_stage_editor_opened(move || {
                if let Some(ui) = ui.upgrade() {
                    refresh_panels(&ui, &session.borrow());
                }
            });
        }
    }

    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        control.on_save_shortcuts(move |text| {
            session.borrow_mut().save_shortcuts(&text);
            if let Some(ui) = ui.upgrade() {
                ui.set_show_shortcuts(false);
                refresh(&ui, &session.borrow());
            }
        });
    }

    {
        let ui = control.as_weak();
        let settings = Rc::clone(&settings);
        let settings_path = settings_path.clone();
        let output = output.as_weak();
        let stage = stage.as_weak();
        control.on_save_layouts(move |text| {
            settings.borrow_mut().apply_profile_text(&text);
            if let Err(err) = settings.borrow().save(&settings_path) {
                tracing::error!("could not save window layouts: {err}");
            }
            if let (Some(ui), Some(output), Some(stage)) =
                (ui.upgrade(), output.upgrade(), stage.upgrade())
            {
                apply_window_profile(
                    ui.window(),
                    output.window(),
                    stage.window(),
                    &settings.borrow(),
                );
                ui.set_show_layouts(false);
            }
        });
    }

    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        control.on_save_variables(move |text| {
            session.borrow_mut().save_variables(&text);
            if let Some(ui) = ui.upgrade() {
                ui.set_show_variables(false);
                refresh(&ui, &session.borrow());
            }
        });
    }

    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        control.on_save_timeline(move |text| {
            session.borrow_mut().save_timeline(&text);
            if let Some(ui) = ui.upgrade() {
                refresh(&ui, &session.borrow());
            }
        });
    }

    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        control.on_play_timeline(move || {
            let mut current = session.borrow_mut();
            if current.timeline_cues.is_empty()
                && let Some(ui) = ui.upgrade()
            {
                current.save_timeline(&ui.get_timeline_text());
            }
            let _ = current.start_timeline();
            drop(current);
            if let Some(ui) = ui.upgrade() {
                ui.set_show_timeline(false);
                refresh(&ui, &session.borrow());
            }
        });
    }

    // Shortcuts pressed while the output or stage window has focus drive the show too.
    {
        let ui = control.as_weak();
        output.on_hotkey(move |text, ctrl, alt, shift| {
            if let Some(ui) = ui.upgrade() {
                ui.invoke_hotkey(text, ctrl, alt, shift);
            }
        });
        let ui = control.as_weak();
        stage.on_hotkey(move |text, ctrl, alt, shift| {
            if let Some(ui) = ui.upgrade() {
                ui.invoke_hotkey(text, ctrl, alt, shift);
            }
        });
    }

    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        let live_pump = Rc::clone(&live_pump);
        let edit_slot = Rc::clone(&edit_slot);
        control.on_hotkey(move |text, ctrl, alt, shift| {
            let mut chord = String::new();
            if ctrl {
                chord.push_str("Ctrl+");
            }
            if alt {
                chord.push_str("Alt+");
            }
            if shift {
                chord.push_str("Shift+");
            }
            chord.push_str(&freeshow_data::normalize_chord(&text));
            if session.borrow_mut().perform_shortcut(&chord)
                && let Some(ui) = ui.upgrade()
            {
                refresh(&ui, &session.borrow());
                // Next/Previous in edit mode moves the editor to the slide that went live.
                if ui.get_edit_mode() {
                    publish(&edit_slot, edit_frame(&session.borrow()));
                }
                apply_window_behavior(
                    ui.window(),
                    session.borrow().always_on_top,
                    session.borrow().kiosk,
                );
                pump_live(&live_pump);
            }
        });
    }

    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        let output_slot = Rc::clone(&output_slot);
        let stage_slot = Rc::clone(&stage_slot);
        let preview_slot = Rc::clone(&preview_slot);
        let edit_slot = Rc::clone(&edit_slot);
        let output_fade = Rc::clone(&output_fade);
        let stage_fade = Rc::clone(&stage_fade);
        let last_output = Rc::clone(&last_output);
        let last_stage = Rc::clone(&last_stage);
        let next_slide = Rc::clone(&next_slide);
        let audio_out = Rc::clone(&audio_out);
        let video_player = Rc::clone(&video_player);
        let video_timer = Rc::clone(&video_timer);
        let church_import = Rc::clone(&church_import);
        let settings_for_functions = Rc::clone(&settings);
        let settings_path_for_functions = settings_path.clone();
        let output_for_functions = output.as_weak();
        let stage_for_functions = stage.as_weak();
        control.on_choose_drawer(move |index| {
            let playback = LivePlayback {
                audio: Rc::clone(&audio_out),
                video: Rc::clone(&video_player),
                timer: Rc::clone(&video_timer),
            };
            let index = session.borrow().drawer_real_index(index.max(0) as usize);
            if session.borrow().tab == "functions" {
                let mut current = session.borrow_mut();
                match index {
                    0 => {
                        if !current.clear_all_layers() {
                            return;
                        }
                        let _ = current.take_plays();
                        stop_video(&video_timer, &video_player);
                        *audio_out.borrow_mut() = None;
                        let blank = audience_frame(&current);
                        next_slide.stop();
                        output_fade.stop();
                        stage_fade.stop();
                        *last_output.borrow_mut() = blank.clone();
                        publish_audience(&output_slot, blank.clone());
                        publish(&preview_slot, blank);
                        let stage = stage_frame(&current);
                        *last_stage.borrow_mut() = stage.clone();
                        publish_stage(&stage_slot, stage);
                    }
                    1 => {
                        if !current.advance_show() {
                            return;
                        }
                        let plays = current.take_plays();
                        if picture_fades_in(&plays[0], &current) {
                            next_slide.stop();
                            drop(current);
                            start_plays(
                                plays,
                                &session,
                                &playback,
                                &output_slot,
                                &preview_slot,
                                &stage_slot,
                                &last_output,
                                &last_stage,
                            );
                            if let Some(ui) = ui.upgrade() {
                                refresh(&ui, &session.borrow());
                            }
                            return;
                        }
                        if current.background_transition {
                            stop_video(&video_timer, &video_player);
                        }
                        let audience = audience_frame(&current);
                        let stage = stage_frame(&current);
                        let (motion, background) = current.output_motion();
                        let seconds = if background {
                            None
                        } else {
                            current.next_timer_seconds()
                        };
                        drop(current);
                        send_frames(
                            &output_fade,
                            &stage_fade,
                            &output_slot,
                            &preview_slot,
                            &stage_slot,
                            &last_output,
                            &last_stage,
                            audience,
                            stage,
                            motion,
                            background,
                            &next_slide,
                            &session,
                            ui.clone(),
                            seconds,
                            &playback,
                        );
                        start_plays(
                            plays,
                            &session,
                            &playback,
                            &output_slot,
                            &preview_slot,
                            &stage_slot,
                            &last_output,
                            &last_stage,
                        );
                        if let Some(ui) = ui.upgrade() {
                            refresh(&ui, &session.borrow());
                        }
                        return;
                    }
                    2 => {
                        if !current.retreat_show() {
                            return;
                        }
                        let plays = current.take_plays();
                        if picture_fades_in(&plays[0], &current) {
                            next_slide.stop();
                            drop(current);
                            start_plays(
                                plays,
                                &session,
                                &playback,
                                &output_slot,
                                &preview_slot,
                                &stage_slot,
                                &last_output,
                                &last_stage,
                            );
                            if let Some(ui) = ui.upgrade() {
                                refresh(&ui, &session.borrow());
                            }
                            return;
                        }
                        if current.background_transition {
                            stop_video(&video_timer, &video_player);
                        }
                        let audience = audience_frame(&current);
                        let stage = stage_frame(&current);
                        let (motion, background) = current.output_motion();
                        let seconds = if background {
                            None
                        } else {
                            current.next_timer_seconds()
                        };
                        drop(current);
                        send_frames(
                            &output_fade,
                            &stage_fade,
                            &output_slot,
                            &preview_slot,
                            &stage_slot,
                            &last_output,
                            &last_stage,
                            audience,
                            stage,
                            motion,
                            background,
                            &next_slide,
                            &session,
                            ui.clone(),
                            seconds,
                            &playback,
                        );
                        start_plays(
                            plays,
                            &session,
                            &playback,
                            &output_slot,
                            &preview_slot,
                            &stage_slot,
                            &last_output,
                            &last_stage,
                        );
                        if let Some(ui) = ui.upgrade() {
                            refresh(&ui, &session.borrow());
                        }
                        return;
                    }
                    3 => {
                        current.toggle_timer();
                        let stage = stage_frame(&current);
                        *last_stage.borrow_mut() = stage.clone();
                        publish_stage(&stage_slot, stage);
                    }
                    4 => {
                        let Some(root) = current.root.clone() else {
                            tracing::error!("could not record without a library");
                            return;
                        };
                        let dir = root.join("Recordings");
                        let _ = std::fs::create_dir_all(&dir);
                        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
                        let path = dir.join(format!("freeshow-{stamp}.mp4"));
                        current.recording = freeshow_record::ServiceCapture.toggle_file(
                            &path,
                            output_width,
                            output_height,
                        );
                        if current.recording {
                            let frame = last_output.borrow();
                            freeshow_record::ServiceCapture.submit(
                                &frame.pixels,
                                frame.width,
                                frame.height,
                            );
                        }
                    }
                    5 => {
                        let Some(root) = current.root.clone() else {
                            tracing::error!("could not load planning center without a library");
                            return;
                        };
                        let report = freeshow_pco::PlanningCenter.sync(&root);
                        let fresh = Session::open(root);
                        current.shows = fresh.shows;
                        current.projects = fresh.projects;
                        current.warnings = fresh.warnings;
                        if let Some(id) = report.project_id {
                            current.project_id = Some(id);
                            current.project_item = 0;
                        }
                    }
                    6 => {
                        let Some(root) = current.root.clone() else {
                            tracing::error!("could not load churchapps without a library");
                            return;
                        };
                        if church_import.borrow().is_none() {
                            match ChurchImport::start(root) {
                                Ok(job) => {
                                    church_import.replace(Some(job));
                                }
                                Err(error) => {
                                    tracing::error!("could not start ChurchApps import: {error}");
                                    if let Some(ui) = ui.upgrade() {
                                        ui.set_import_status(
                                            "Could not start ChurchApps import".into(),
                                        );
                                    }
                                }
                            }
                        }
                    }
                    7 => {
                        let Some(root) = current.root.clone() else {
                            tracing::error!("could not back up without a library");
                            return;
                        };
                        let _ = freeshow_cloud::LibraryCloud.backup(&root);
                    }
                    8 => {
                        if let Some(job) = church_import.borrow().as_ref() {
                            job.control.cancel();
                        }
                        let Some(root) = current.root.clone() else {
                            tracing::error!("could not restore without a library");
                            return;
                        };
                        let report = freeshow_cloud::LibraryCloud.restore_latest(&root);
                        if report.path.is_none() {
                            return;
                        }
                        let fresh = Session::open(root);
                        current.shows = fresh.shows;
                        current.projects = fresh.projects;
                        current.bibles = fresh.bibles;
                        current.icecast = fresh.icecast;
                        current.warnings = fresh.warnings;
                    }
                    9 => {
                        let Ok(scene) = std::env::var("FREESHOW_OBS_SCENE") else {
                            tracing::error!("obs scene name is missing");
                            return;
                        };
                        freeshow_obs::ObsOutput::from_settings(&current.preferences.obs_data)
                            .unwrap_or_else(freeshow_obs::ObsOutput::from_env)
                            .set_scene(scene.trim());
                    }
                    10 => freeshow_obs::ObsOutput::from_settings(&current.preferences.obs_data)
                        .unwrap_or_else(freeshow_obs::ObsOutput::from_env)
                        .toggle_stream(),
                    11 => freeshow_obs::ObsOutput::from_settings(&current.preferences.obs_data)
                        .unwrap_or_else(freeshow_obs::ObsOutput::from_env)
                        .toggle_record(),
                    12 => {
                        let Some(assist) = freeshow_ai::OllamaAssist::configured() else {
                            tracing::error!("ai is not configured");
                            return;
                        };
                        let Some(slide) = current.current_slide_text() else {
                            tracing::error!("ai needs a selected slide");
                            return;
                        };
                        let prompt = std::env::var("FREESHOW_AI_PROMPT").unwrap_or_else(|_| {
                            "Rewrite this slide as short lyric lines.".to_string()
                        });
                        let Some(reply) = assist.rewrite(&prompt, &slide) else {
                            return;
                        };
                        if let Some(lines) = reply.lines {
                            if !current.replace_slide_text(&lines.join("\n")) {
                                tracing::error!("ai could not update the slide");
                                return;
                            }
                            publish(&edit_slot, edit_frame(&current));
                        }
                    }
                    13 => current.toggle_icecast(None),
                    14 => match current.export_current_show_text() {
                        Ok(path) => tracing::info!("exported show text to {}", path.display()),
                        Err(err) => tracing::warn!("show text was not exported: {err}"),
                    },
                    15 => match current.export_current_show_pdf() {
                        Ok(path) => tracing::info!("exported show PDF to {}", path.display()),
                        Err(err) => tracing::warn!("show PDF was not exported: {err}"),
                    },
                    16 => match current.export_current_project_package() {
                        Ok(path) => {
                            tracing::info!("exported project package to {}", path.display())
                        }
                        Err(err) => tracing::warn!("project package was not exported: {err}"),
                    },
                    17 => {
                        let frame = audience_frame(&current);
                        match current.export_screenshot(&frame) {
                            Ok(path) => tracing::info!("saved screenshot to {}", path.display()),
                            Err(err) => tracing::warn!("screenshot was not saved: {err}"),
                        }
                    }
                    18 => current.toggle_metronome(),
                    19 => {
                        let _ = current.undo();
                    }
                    20 => {
                        let _ = current.redo();
                    }
                    21 => {
                        let _ = current.toggle_focus_mode();
                    }
                    22 => current.show_guide(),
                    23 => {
                        let _ = current.toggle_kiosk();
                    }
                    24 => {
                        let _ = current.toggle_always_on_top();
                    }
                    25 => {
                        if let Some(ui) = ui.upgrade() {
                            let open = !ui.get_show_shortcuts();
                            ui.set_show_shortcuts(open);
                            if open {
                                ui.set_shortcut_text(current.shortcut_text().into());
                            }
                        }
                    }
                    26 => {
                        if let (Some(ui), Some(output), Some(stage)) = (
                            ui.upgrade(),
                            output_for_functions.upgrade(),
                            stage_for_functions.upgrade(),
                        ) {
                            let profile = capture_window_profile(
                                ui.window(),
                                output.window(),
                                stage.window(),
                            );
                            let mut settings = settings_for_functions.borrow_mut();
                            let name = settings
                                .active_window_profile
                                .clone()
                                .unwrap_or_else(|| "default".to_string());
                            settings.save_window_profile(name, profile);
                            if let Err(err) = settings.save(&settings_path_for_functions) {
                                tracing::error!("could not save window layout: {err}");
                            }
                        }
                    }
                    27 => {
                        if let Some(ui) = ui.upgrade() {
                            let open = !ui.get_show_layouts();
                            ui.set_show_layouts(open);
                            if open {
                                ui.set_layout_text(
                                    settings_for_functions.borrow().profile_text().into(),
                                );
                            }
                        }
                    }
                    28 => {
                        let _ = current.toggle_watermark();
                    }
                    29 => {
                        if let Some(ui) = ui.upgrade() {
                            let open = !ui.get_show_variables();
                            ui.set_show_variables(open);
                            if open {
                                ui.set_variable_text(current.variable_text().into());
                            }
                        }
                    }
                    30 => current.clear_drawing(),
                    31 => {
                        let _ = current.toggle_first_interaction();
                    }
                    32 => {
                        if current.timeline_playing {
                            current.stop_timeline();
                        } else if let Some(ui) = ui.upgrade() {
                            let open = !ui.get_show_timeline();
                            ui.set_show_timeline(open);
                            if open {
                                ui.set_timeline_text(current.timeline_text().into());
                            }
                        }
                    }
                    33 => {
                        let _ = current.toggle_effects();
                    }
                    _ => {}
                }
                let always_on_top = current.always_on_top;
                let kiosk = current.kiosk;
                if let Some(ui) = ui.upgrade() {
                    refresh(&ui, &current);
                    apply_window_behavior(ui.window(), always_on_top, kiosk);
                }
                return;
            }
            let action = session.borrow_mut().select_drawer(index);
            match action {
                DrawerAction::PlayAudio(path) => {
                    *audio_out.borrow_mut() = None;
                    session.borrow_mut().playing_audio = Some(path.clone());
                    session.borrow_mut().audio_paused = false;
                    let (routes, looping, from, to, tempo) = {
                        let current = session.borrow();
                        let (from, to) = current.audio_span();
                        (
                            current.audio_routes(),
                            current.audio_should_loop(),
                            from,
                            to,
                            current.audio_tempo(),
                        )
                    };
                    let network_routes = session.borrow().audio_network_routes();
                    match AudioOutput::play_file_configured(
                        &path,
                        &routes,
                        &network_routes,
                        &AudioPlaybackOptions {
                            looping,
                            from_seconds: from,
                            to_seconds: to,
                            rate: tempo,
                            effects: freeshow_media::effects_from_value(
                                &session.borrow().preferences.audio_effects,
                            ),
                        },
                    ) {
                        Ok(output) => {
                            *audio_out.borrow_mut() = Some(output);
                        }
                        Err(err) => tracing::error!("could not play audio: {err}"),
                    }
                }
                DrawerAction::PlayVideo(path) => {
                    stop_video(&video_timer, &video_player);
                    let routes = session.borrow().video_audio_routes();
                    let network_routes = session.borrow().video_network_routes();
                    match VideoPlayer::open_with_network(&path, &routes, &network_routes) {
                        Ok(player) => {
                            player.set_looping(session.borrow().media_loop);
                            player.set_muted(session.borrow().media_muted);
                            player.set_volume(1.0);
                            player.set_rate(session.borrow().video_rate);
                            let (from, to) = {
                                let current = session.borrow();
                                (current.video_from, current.video_to)
                            };
                            player.set_span(from, to);
                            follow_video(
                                Rc::clone(&video_timer),
                                Rc::clone(&video_player),
                                player,
                                Some(path.clone()),
                                Rc::clone(&session),
                                Rc::clone(&output_slot),
                                Rc::clone(&preview_slot),
                                Rc::clone(&stage_slot),
                                Rc::clone(&last_output),
                                Rc::clone(&last_stage),
                            );
                        }
                        Err(err) => tracing::error!("could not play video: {err}"),
                    }
                }
                DrawerAction::PlayCamera(name) => {
                    stop_video(&video_timer, &video_player);
                    match VideoPlayer::open_camera(&name) {
                        Ok(player) => follow_video(
                            Rc::clone(&video_timer),
                            Rc::clone(&video_player),
                            player,
                            None,
                            Rc::clone(&session),
                            Rc::clone(&output_slot),
                            Rc::clone(&preview_slot),
                            Rc::clone(&stage_slot),
                            Rc::clone(&last_output),
                            Rc::clone(&last_stage),
                        ),
                        Err(err) => tracing::error!("could not open camera: {err}"),
                    }
                }
                kind @ (DrawerAction::Present | DrawerAction::Refresh) => {
                    if matches!(kind, DrawerAction::Present) {
                        session.borrow_mut().blanked = false;
                        stop_video(&video_timer, &video_player);
                    }
                    let audience = audience_frame(&session.borrow());
                    let stage = stage_frame(&session.borrow());
                    let (motion, background) = session.borrow_mut().output_motion();
                    let seconds = if background {
                        None
                    } else {
                        session.borrow().next_timer_seconds()
                    };
                    send_frames(
                        &output_fade,
                        &stage_fade,
                        &output_slot,
                        &preview_slot,
                        &stage_slot,
                        &last_output,
                        &last_stage,
                        audience,
                        stage,
                        motion,
                        background,
                        &next_slide,
                        &session,
                        ui.clone(),
                        seconds,
                        &playback,
                    );
                }
                DrawerAction::None => {}
            }
            if let Some(ui) = ui.upgrade() {
                refresh(&ui, &session.borrow());
            }
        });
    }

    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        let live_pump = Rc::clone(&live_pump);
        let edit_slot = Rc::clone(&edit_slot);
        control.on_choose_slide(move |index| {
            let Some(ui) = ui.upgrade() else {
                return;
            };
            {
                let mut current = session.borrow_mut();
                // As in FreeShow, a click outside the editor puts the slide on the output.
                // Edit mode only applies where the editor is shown.
                if ui.get_editing() {
                    current.select_slide_for_edit(index.max(0) as usize);
                } else {
                    current.select_slide(index.max(0) as usize);
                    current.blanked = false;
                    current.refresh_output = true;
                }
            }
            refresh(&ui, &session.borrow());
            if ui.get_editing() {
                publish(&edit_slot, edit_frame(&session.borrow()));
            }
            pump_live(&live_pump);
        });
    }

    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        let edit_slot = Rc::clone(&edit_slot);
        control.on_edit_mode_changed(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };
            if ui.get_edit_mode() {
                if session.borrow_mut().pick_first_slide() {
                    refresh(&ui, &session.borrow());
                }
                publish(&edit_slot, edit_frame(&session.borrow()));
            }
        });
    }

    {
        let session = Rc::clone(&session);
        let live_pump = Rc::clone(&live_pump);
        control.on_clear_output(move |layer| {
            use freeshow_remote::Command;
            let command = match layer.as_str() {
                "slide" => Command::ClearSlide,
                "background" => Command::ClearBackground,
                "overlays" => Command::ClearOverlays,
                "audio" => Command::StopAudio,
                _ => Command::Clear,
            };
            session.borrow_mut().ui_commands.push(command);
            pump_live(&live_pump);
        });
    }

    {
        let session = Rc::clone(&session);
        let live_pump = Rc::clone(&live_pump);
        control.on_audio_control(move |action| {
            if session.borrow_mut().audio_control(action.as_str()) {
                pump_live(&live_pump);
            }
        });
    }

    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        control.on_save_lyrics(move |text| {
            if !session.borrow_mut().save_lyrics(text.as_str()) {
                tracing::error!("could not save lyrics");
            }
            if let Some(ui) = ui.upgrade() {
                refresh(&ui, &session.borrow());
            }
        });
    }

    {
        let session = Rc::clone(&session);
        let audio_out = Rc::clone(&audio_out);
        let video_player = Rc::clone(&video_player);
        control.on_set_volume(move |value| {
            let volume = (value / 100.0).clamp(0.0, 1.0);
            session.borrow_mut().set_channel_volume("main", volume);
            let routes = session.borrow().audio_routes();
            if let Some(output) = audio_out.borrow().as_ref() {
                output.set_route_gains(&routes);
                output.set_network_routes(&session.borrow().audio_network_routes());
            }
            if let Some(player) = video_player.borrow().as_ref() {
                player.set_audio_routes(&session.borrow().video_audio_routes());
                player.set_network_routes(&session.borrow().video_network_routes());
            }
        });
    }

    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        control.on_save_group(move |text| {
            if session.borrow().group() == text.as_str() {
                return;
            }
            if !session.borrow_mut().save_group(text.as_str()) {
                tracing::error!("could not save group");
            }
            if let Some(ui) = ui.upgrade() {
                refresh(&ui, &session.borrow());
            }
        });
    }

    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        let edit_slot = Rc::clone(&edit_slot);
        control.on_prev_item(move || {
            session.borrow_mut().prev_item();
            if let Some(ui) = ui.upgrade() {
                refresh(&ui, &session.borrow());
                publish(&edit_slot, edit_frame(&session.borrow()));
            }
        });
    }
    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        let edit_slot = Rc::clone(&edit_slot);
        control.on_next_item(move || {
            session.borrow_mut().next_item();
            if let Some(ui) = ui.upgrade() {
                refresh(&ui, &session.borrow());
                publish(&edit_slot, edit_frame(&session.borrow()));
            }
        });
    }
    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        let edit_slot = Rc::clone(&edit_slot);
        control.on_add_item(move || {
            if !session.borrow_mut().add_item_box() {
                tracing::error!("could not add a text box");
            }
            if let Some(ui) = ui.upgrade() {
                refresh(&ui, &session.borrow());
                publish(&edit_slot, edit_frame(&session.borrow()));
            }
        });
    }
    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        let edit_slot = Rc::clone(&edit_slot);
        control.on_remove_item(move || {
            if !session.borrow_mut().remove_item_box() {
                tracing::error!("could not remove the text box");
            }
            if let Some(ui) = ui.upgrade() {
                refresh(&ui, &session.borrow());
                publish(&edit_slot, edit_frame(&session.borrow()));
            }
        });
    }

    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        let output_slot = Rc::clone(&output_slot);
        let stage_slot = Rc::clone(&stage_slot);
        let preview_slot = Rc::clone(&preview_slot);
        let output_fade = Rc::clone(&output_fade);
        let stage_fade = Rc::clone(&stage_fade);
        let last_output = Rc::clone(&last_output);
        let last_stage = Rc::clone(&last_stage);
        let next_slide = Rc::clone(&next_slide);
        let audio_out = Rc::clone(&audio_out);
        let video_player = Rc::clone(&video_player);
        let video_timer = Rc::clone(&video_timer);
        control.on_go_slide(move |text| {
            let playback = LivePlayback {
                audio: Rc::clone(&audio_out),
                video: Rc::clone(&video_player),
                timer: Rc::clone(&video_timer),
            };
            let session_rc = Rc::clone(&session);
            let mut session = session.borrow_mut();
            if session.tab == "shows" {
                // Go with no slide picked presents the first one, as Next does.
                if session.slide_index.is_none() {
                    session.slide_index = session.first_slide();
                }
                session.save_lyrics(text.as_str());
                // Run the slide's own background and cues, as a click would.
                if let Some(index) = session.slide_index {
                    session.select_slide(index);
                }
            }
            session.go_live();
            let audience = audience_frame(&session);
            let stage = stage_frame(&session);
            let (motion, background) = session.output_motion();
            let seconds = if background {
                None
            } else {
                session.next_timer_seconds()
            };
            send_frames(
                &output_fade,
                &stage_fade,
                &output_slot,
                &preview_slot,
                &stage_slot,
                &last_output,
                &last_stage,
                audience,
                stage,
                motion,
                background,
                &next_slide,
                &session_rc,
                ui.clone(),
                seconds,
                &playback,
            );
            if let Some(ui) = ui.upgrade() {
                refresh(&ui, &session);
            }
        });
    }

    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        let edit_slot = Rc::clone(&edit_slot);
        control.on_choose_project(move |index| {
            let picked = session
                .borrow_mut()
                .pick_project_item(index.max(0) as usize);
            let Some(ui) = ui.upgrade() else {
                return;
            };
            ui.set_search_query("".into());
            match picked {
                Ok(session::ProjectPick::Opened) => {
                    ui.set_active_tab("shows".into());
                    ui.set_panel_title("Shows".into());
                    refresh(&ui, &session.borrow());
                    publish(&edit_slot, edit_frame(&session.borrow()));
                }
                Ok(session::ProjectPick::Section) => {
                    refresh(&ui, &session.borrow());
                }
                Ok(session::ProjectPick::Drawer { tab, label, index }) => {
                    ui.set_active_tab(tab.into());
                    ui.set_panel_title(label.into());
                    refresh(&ui, &session.borrow());
                    ui.invoke_choose_drawer(index as i32);
                }
                Err(err) => {
                    session.borrow_mut().notice = err;
                    refresh(&ui, &session.borrow());
                }
            }
        });
    }

    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        control.on_move_project(move |delta| {
            if !session.borrow_mut().move_project(delta as isize) {
                tracing::debug!("project item stayed in place");
            }
            if let Some(ui) = ui.upgrade() {
                refresh(&ui, &session.borrow());
            }
        });
    }

    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        control.on_drop_project(move |from, to| {
            if !session
                .borrow_mut()
                .drop_project_item(from.max(0) as usize, to.max(0) as usize)
            {
                tracing::debug!("project item stayed in place");
            }
            if let Some(ui) = ui.upgrade() {
                refresh(&ui, &session.borrow());
            }
        });
    }

    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        control.on_add_to_project(move || {
            if !session.borrow_mut().add_current_show() {
                tracing::error!("could not add the show to the project");
            }
            if let Some(ui) = ui.upgrade() {
                refresh(&ui, &session.borrow());
            }
        });
    }

    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        let edit_slot = Rc::clone(&edit_slot);
        control.on_create_show(move || {
            if !session.borrow_mut().create_show() {
                tracing::error!("could not create a show");
                return;
            }
            publish(&edit_slot, edit_frame(&session.borrow()));
            if let Some(ui) = ui.upgrade() {
                refresh(&ui, &session.borrow());
            }
        });
    }

    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        let edit_slot = Rc::clone(&edit_slot);
        control.on_add_slide(move || {
            if !session.borrow_mut().add_slide() {
                tracing::error!("could not add a slide");
                return;
            }
            publish(&edit_slot, edit_frame(&session.borrow()));
            if let Some(ui) = ui.upgrade() {
                refresh(&ui, &session.borrow());
            }
        });
    }

    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        control.on_create_project(move || {
            if !session.borrow_mut().create_project() {
                tracing::error!("could not create a project");
                return;
            }
            if let Some(ui) = ui.upgrade() {
                refresh(&ui, &session.borrow());
            }
        });
    }

    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        control.on_search(move |query| {
            session.borrow_mut().search(query.as_str());
            if let Some(ui) = ui.upgrade() {
                refresh(&ui, &session.borrow());
            }
        });
    }

    let (command_tx, command_rx) = mpsc::channel();
    let snapshot = Arc::new(Mutex::new(session.borrow().to_snapshot()));
    let (remote_password, remote_ports) = {
        let session = session.borrow();
        (
            session.preferences.remote_password.clone(),
            freeshow_remote::RemotePorts {
                remote: session
                    .preferences
                    .remote_port
                    .unwrap_or(freeshow_remote::REMOTE_PORT),
                stage: session
                    .preferences
                    .stage_port
                    .unwrap_or(freeshow_remote::STAGE_PORT),
            },
        )
    };
    if let Some(notice) = busy_ports_notice(remote_ports.remote, remote_ports.stage) {
        tracing::warn!("{notice}");
        session.borrow_mut().notice = notice;
        refresh(&control, &session.borrow());
    }
    let (stage_frame_tx, stage_frame_rx) = mpsc::sync_channel(1);
    let max_connections = session.borrow().preferences.max_connections();
    freeshow_remote::spawn_limited(
        Arc::clone(&snapshot),
        command_tx.clone(),
        remote_password,
        remote_ports,
        stage_frame_tx,
        max_connections,
    );
    freeshow_osc::OscInput::default().start(freeshow_remote::ShowBus {
        snapshot: Arc::clone(&snapshot),
        commands: command_tx.clone(),
    });
    let deck_commands = command_tx.clone();
    let deck_snapshot = Arc::clone(&snapshot);
    let _ = crate::streamdeck::start_listener(move |action| {
        let channel = freeshow_remote::normalize_action_channel(action);
        let Ok(mut snap) = deck_snapshot.lock() else {
            return;
        };
        let (_, commands) = freeshow_remote::handle_message(
            &mut snap,
            &serde_json::json!({ "channel": channel, "data": null }),
        );
        for command in commands {
            let _ = deck_commands.send(command);
        }
    });
    freeshow_midi::MidiInput::default().start(freeshow_remote::ShowBus {
        snapshot: Arc::clone(&snapshot),
        commands: command_tx,
    });
    freeshow_timecode::MtcInput::default().start();
    freeshow_timecode::LtcInput.start();
    let mut rtmp_started = false;
    if let Ok(url) = std::env::var("FREESHOW_RTMP")
        && !url.trim().is_empty()
    {
        let output_id = session
            .borrow()
            .network_output("rtmp")
            .unwrap_or_else(|| "default".into());
        rtmp_started |= freeshow_record::ServiceCapture.start_rtmp_routed(
            &url,
            output_width,
            output_height,
            &output_id,
        );
    }
    for dest in session.borrow().rtmp_destinations() {
        rtmp_started |= freeshow_record::ServiceCapture.start_rtmp_routed(
            &dest.url,
            output_width,
            output_height,
            &dest.id,
        );
    }
    if rtmp_started {
        let frame = last_output.borrow();
        freeshow_record::ServiceCapture.submit(&frame.pixels, frame.width, frame.height);
    }
    let remote_session = Rc::clone(&session);
    let remote_ui = control.as_weak();
    let remote_output = Rc::clone(&output_slot);
    let remote_preview = Rc::clone(&preview_slot);
    let remote_stage = Rc::clone(&stage_slot);
    let remote_output_fade = Rc::clone(&output_fade);
    let remote_stage_fade = Rc::clone(&stage_fade);
    let remote_last_output = Rc::clone(&last_output);
    let remote_last_stage = Rc::clone(&last_stage);
    let remote_next_slide = Rc::clone(&next_slide);
    let remote_audio = Rc::clone(&audio_out);
    let remote_video = Rc::clone(&video_player);
    let remote_video_timer = Rc::clone(&video_timer);
    let remote_playback = LivePlayback {
        audio: Rc::clone(&remote_audio),
        video: Rc::clone(&remote_video),
        timer: Rc::clone(&remote_video_timer),
    };
    let import_snapshot = Arc::clone(&snapshot);
    let remote_timer = slint::Timer::default();
    let pump: Rc<dyn Fn()> = Rc::new(move || {
        let mut batch = Vec::new();
        while let Ok(command) = command_rx.try_recv() {
            batch.push(command);
        }
        batch.append(&mut remote_session.borrow_mut().ui_commands);
        if batch.is_empty() {
            let started = remote_session
                .borrow_mut()
                .start_due_events(chrono::Utc::now());
            sample_audio(&remote_session, &remote_audio);
            let looked_up = remote_session.borrow_mut().poll_stream_lookup();
            if looked_up == Some(false)
                && let Some(ui) = remote_ui.upgrade()
            {
                refresh(&ui, &remote_session.borrow());
            }
            let moved = {
                let mut session = remote_session.borrow_mut();
                session.run_due_cue_completion(std::time::Instant::now())
                    || session.refresh_output
                    || looked_up == Some(true)
            };
            let live_edited = !started
                && !moved
                && !remote_output_fade.running()
                && std::mem::take(&mut remote_session.borrow_mut().live_edited);
            if live_edited {
                // An edit to what is on air shows up right away, without a transition.
                let (audience, stage) = {
                    let session = remote_session.borrow();
                    (audience_frame(&session), stage_frame(&session))
                };
                *remote_last_output.borrow_mut() = audience.clone();
                submit_network(NetworkPicture::Audience, &audience);
                publish_audience(&remote_output, audience.clone());
                publish(&remote_preview, audience);
                *remote_last_stage.borrow_mut() = stage.clone();
                publish_stage(&remote_stage, stage);
            }
            if started || moved {
                remote_session.borrow_mut().refresh_output = false;
                if moved {
                    remote_next_slide.stop();
                    let plays = remote_session.borrow_mut().take_plays();
                    let fades = picture_fades_in(&plays[0], &remote_session.borrow());
                    if !fades && remote_session.borrow().background_transition {
                        stop_video(&remote_video_timer, &remote_video);
                    }
                    start_plays(
                        plays,
                        &remote_session,
                        &remote_playback,
                        &remote_output,
                        &remote_preview,
                        &remote_stage,
                        &remote_last_output,
                        &remote_last_stage,
                    );
                    if fades {
                        if let Some(ui) = remote_ui.upgrade() {
                            refresh(&ui, &remote_session.borrow());
                        }
                        if let Ok(mut stored) = snapshot.lock() {
                            *stored = remote_session.borrow().to_snapshot();
                        }
                        return;
                    }
                }
                let (audience, stage) = {
                    let session = remote_session.borrow();
                    (audience_frame(&session), stage_frame(&session))
                };
                if let Some(ui) = remote_ui.upgrade() {
                    refresh(&ui, &remote_session.borrow());
                }
                let (motion, background) = remote_session.borrow_mut().output_motion();
                let seconds = if background {
                    None
                } else {
                    remote_session.borrow().next_timer_seconds()
                };
                send_frames(
                    &remote_output_fade,
                    &remote_stage_fade,
                    &remote_output,
                    &remote_preview,
                    &remote_stage,
                    &remote_last_output,
                    &remote_last_stage,
                    audience,
                    stage,
                    motion,
                    background,
                    &remote_next_slide,
                    &remote_session,
                    remote_ui.clone(),
                    seconds,
                    &remote_playback,
                );
            }
            if let Ok(mut stored) = snapshot.lock() {
                *stored = remote_session.borrow().to_snapshot();
            }
            return;
        }
        let clear = matches!(batch.last(), Some(freeshow_remote::Command::Clear));
        sample_audio(&remote_session, &remote_audio);
        let output_refresh = std::mem::take(&mut remote_session.borrow_mut().refresh_output);
        let (audience, stage, timed_move) = {
            let mut session = remote_session.borrow_mut();
            for command in batch {
                session.apply_remote(command);
            }
            let timed_move = session.run_due_cue_completion(std::time::Instant::now());
            let audience = audience_frame(&session);
            let stage = stage_frame(&session);
            if let Ok(mut stored) = snapshot.lock() {
                *stored = session.to_snapshot();
            }
            (audience, stage, timed_move)
        };
        let plays = remote_session.borrow_mut().take_plays();
        let fades = picture_fades_in(&plays[0], &remote_session.borrow());
        if timed_move || output_refresh {
            remote_next_slide.stop();
            if !fades && remote_session.borrow().background_transition {
                stop_video(&remote_video_timer, &remote_video);
            }
        }
        start_plays(
            plays,
            &remote_session,
            &remote_playback,
            &remote_output,
            &remote_preview,
            &remote_stage,
            &remote_last_output,
            &remote_last_stage,
        );
        if fades {
            remote_next_slide.stop();
        }
        if clear && remote_session.borrow().blanked {
            remote_next_slide.stop();
            // A fade still running would paint the old slide back over the blank.
            remote_output_fade.stop();
            remote_stage_fade.stop();
            *remote_last_output.borrow_mut() = audience.clone();
            publish(&remote_output, audience.clone());
            publish(&remote_preview, audience);
            *remote_last_stage.borrow_mut() = stage.clone();
            publish(&remote_stage, stage);
        } else if !fades {
            let (motion, background) = remote_session.borrow_mut().output_motion();
            let seconds = if background {
                None
            } else {
                remote_session.borrow().next_timer_seconds()
            };
            send_frames(
                &remote_output_fade,
                &remote_stage_fade,
                &remote_output,
                &remote_preview,
                &remote_stage,
                &remote_last_output,
                &remote_last_stage,
                audience,
                stage,
                motion,
                background,
                &remote_next_slide,
                &remote_session,
                remote_ui.clone(),
                seconds,
                &remote_playback,
            );
        }
        if let Some(ui) = remote_ui.upgrade() {
            refresh(&ui, &remote_session.borrow());
        }
    });
    *live_pump.borrow_mut() = Some(Rc::clone(&pump));
    remote_timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(100),
        move || pump(),
    );

    let clock_session = Rc::clone(&session);
    let clock_stage = Rc::clone(&stage_slot);
    let clock_last = Rc::clone(&last_stage);
    let clock_output = Rc::clone(&output_slot);
    let clock_preview = Rc::clone(&preview_slot);
    let clock_last_output = Rc::clone(&last_output);
    let clock_audio = Rc::clone(&audio_out);
    let clock_video = Rc::clone(&video_player);
    let clock_video_timer = Rc::clone(&video_timer);
    let clock_timer = slint::Timer::default();
    let autosave_session = Rc::clone(&session);
    let autosave_timer = slint::Timer::default();
    if let Some(period) = session.borrow().preferences.autosave_interval() {
        autosave_timer.start(slint::TimerMode::Repeated, period, move || {
            let wrote = autosave_session.borrow().autosave();
            if wrote > 0 {
                tracing::info!("autosave wrote {wrote} files");
            }
        });
    }
    let import_session = Rc::clone(&session);
    let import_ui = control.as_weak();
    let import_timer = slint::Timer::default();
    import_timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(150),
        move || {
            let mut active = church_import.borrow_mut();
            let Some(job) = active.as_ref() else {
                return;
            };
            let Some(ui) = import_ui.upgrade() else {
                return;
            };
            let Some(result) = job.result() else {
                ui.set_import_running(true);
                ui.set_import_status(job.status().into());
                return;
            };
            let job = active.take().unwrap();
            ui.set_import_running(false);
            let mut current = import_session.borrow_mut();
            if job.control.is_cancelled() || current.root.as_ref() != Some(&job.root) {
                ui.set_import_status("ChurchApps import cancelled".into());
                return;
            }
            match result {
                Ok(prepared) => {
                    let report = prepared.publish(&job.root);
                    if report.project_id.is_some() {
                        current.reload_import_catalog(report.project_id);
                        if let Ok(mut snapshot) = import_snapshot.lock() {
                            *snapshot = current.to_snapshot();
                        }
                        refresh(&ui, &current);
                    }
                    let status = match (report.shows, report.errors > 0) {
                        (0, false) => String::new(),
                        (0, true) => "ChurchApps import failed. See the log.".into(),
                        (count, false) => format!("ChurchApps prepared {count} shows"),
                        (count, true) => format!(
                            "ChurchApps prepared {count} shows. See the log for skipped sources."
                        ),
                    };
                    ui.set_import_status(status.into());
                }
                Err(error) => {
                    tracing::error!("churchapps: {error}");
                    ui.set_import_status("ChurchApps import failed. See the log.".into());
                }
            }
        },
    );
    let spell_timer = Rc::new(slint::Timer::default());
    let lyrics_timer = Rc::new(slint::Timer::default());
    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        let spell_timer = Rc::clone(&spell_timer);
        let lyrics_timer = Rc::clone(&lyrics_timer);
        let edit_slot = Rc::clone(&edit_slot);
        control.on_lyrics_edited(move |_| {
            {
                let ui = ui.clone();
                let session = Rc::clone(&session);
                spell_timer.start(
                    slint::TimerMode::SingleShot,
                    std::time::Duration::from_millis(400),
                    move || {
                        if let Some(ui) = ui.upgrade() {
                            let note = spelling_note(&session.borrow(), ui.get_lyrics().as_str());
                            ui.set_spelling(note.into());
                        }
                    },
                );
            }
            // Slide text saves itself shortly after typing stops, like FreeShow.
            let ui = ui.clone();
            let session = Rc::clone(&session);
            let edit_slot = Rc::clone(&edit_slot);
            lyrics_timer.start(
                slint::TimerMode::SingleShot,
                std::time::Duration::from_millis(600),
                move || {
                    let Some(ui) = ui.upgrade() else {
                        return;
                    };
                    let text = ui.get_lyrics();
                    if session.borrow().lyrics() == text.as_str() {
                        return;
                    }
                    if !session.borrow_mut().save_lyrics(text.as_str()) {
                        session.borrow_mut().notice = "Could not save the slide text".into();
                    }
                    refresh(&ui, &session.borrow());
                    publish(&edit_slot, edit_frame(&session.borrow()));
                },
            );
        });
    }
    let notice_timer = Rc::new(slint::Timer::default());
    let library_done: Rc<dyn Fn(&ControlWindow)> = {
        let session = Rc::clone(&session);
        let edit_slot = Rc::clone(&edit_slot);
        let notice_timer = Rc::clone(&notice_timer);
        Rc::new(move |ui: &ControlWindow| {
            refresh(ui, &session.borrow());
            publish(&edit_slot, edit_frame(&session.borrow()));
            let ui = ui.as_weak();
            let session = Rc::clone(&session);
            notice_timer.start(
                slint::TimerMode::SingleShot,
                std::time::Duration::from_secs(8),
                move || {
                    session.borrow_mut().notice.clear();
                    if let Some(ui) = ui.upgrade() {
                        ui.set_notice("".into());
                    }
                },
            );
        })
    };
    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        let library_done = Rc::clone(&library_done);
        control.on_library_action(move |action, arg| {
            let _ = session
                .borrow_mut()
                .library_action(action.as_str(), arg.as_str());
            if let Some(ui) = ui.upgrade() {
                if action == "delete-show" || action == "restore-show" {
                    ui.set_text_mode(false);
                }
                library_done(&ui);
                if matches!(
                    action.as_str(),
                    "new-drawer-entry"
                        | "save-event"
                        | "delete-event"
                        | "save-player"
                        | "delete-player"
                        | "save-rss"
                ) {
                    fill_drawer_form(&ui, &session.borrow());
                }
            }
        });
    }
    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        control.on_sync_drawer_form(move || {
            if let Some(ui) = ui.upgrade() {
                fill_drawer_form(&ui, &session.borrow());
            }
        });
    }
    {
        // Files dropped on the operator window are imported in one batch.
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        let library_done = Rc::clone(&library_done);
        let dropped: Rc<RefCell<Vec<PathBuf>>> = Rc::new(RefCell::new(Vec::new()));
        let timer = Rc::new(slint::Timer::default());
        control.window().on_winit_window_event(move |_, event| {
            use slint::winit_030::{EventResult, winit::event::WindowEvent};
            let WindowEvent::DroppedFile(path) = event else {
                return EventResult::Propagate;
            };
            dropped.borrow_mut().push(path.clone());
            let ui = ui.clone();
            let session = Rc::clone(&session);
            let library_done = Rc::clone(&library_done);
            let dropped = Rc::clone(&dropped);
            timer.start(
                slint::TimerMode::SingleShot,
                std::time::Duration::from_millis(150),
                move || {
                    let paths = std::mem::take(&mut *dropped.borrow_mut());
                    let result = session.borrow_mut().import_files(&paths);
                    let message = match result {
                        Ok(message) => message,
                        Err(err) => format!("Import: {err}"),
                    };
                    tracing::info!("{message}");
                    session.borrow_mut().notice = message;
                    if let Some(ui) = ui.upgrade() {
                        library_done(&ui);
                    }
                },
            );
            EventResult::PreventDefault
        });
    }
    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        let library_done = Rc::clone(&library_done);
        let timer = Rc::new(slint::Timer::default());
        control.on_show_text_edited(move |_| {
            let ui = ui.clone();
            let session = Rc::clone(&session);
            let library_done = Rc::clone(&library_done);
            timer.start(
                slint::TimerMode::SingleShot,
                std::time::Duration::from_millis(700),
                move || {
                    let Some(ui) = ui.upgrade() else {
                        return;
                    };
                    let text = ui.get_show_text();
                    let _ = session
                        .borrow_mut()
                        .library_action("show-text", text.as_str());
                    library_done(&ui);
                },
            );
        });
    }
    let (feed_tx, feed_rx) = spawn_feed_worker();
    let (update_tx, update_rx) = mpsc::channel::<update::ReleaseUpdate>();
    if session.borrow().preferences.alert_updates()
        && std::env::var_os("FREESHOW_SKIP_UPDATE_CHECK").is_none()
    {
        let _ = std::thread::Builder::new()
            .name("update-check".into())
            .spawn(move || match update::check_default() {
                Ok(found) => {
                    tracing::info!("latest release {} (newer: {})", found.tag, found.newer);
                    let _ = update_tx.send(found);
                }
                Err(err) => tracing::info!("update check skipped: {err}"),
            });
    }
    let clock_ui = control.as_weak();
    let latest_update: Rc<RefCell<Option<update::ReleaseUpdate>>> = Rc::new(RefCell::new(None));
    let clock_update = Rc::clone(&latest_update);
    {
        let ui = control.as_weak();
        control.on_get_update(move || {
            let Some(found) = latest_update.borrow().clone() else {
                return;
            };
            if let Some(ui) = ui.upgrade() {
                ui.set_update_label("Downloading…".into());
            }
            let ui = ui.clone();
            let _ = std::thread::Builder::new()
                .name("update-download".into())
                .spawn(move || {
                    let label = match update::download_update(&found, &update::download_dir())
                        .and_then(|path| {
                            tracing::info!("update {} saved to {}", found.tag, path.display());
                            update::reveal(&path)
                        }) {
                        Ok(()) => "Update downloaded",
                        Err(err) => {
                            tracing::warn!("update download failed: {err}");
                            "Update failed"
                        }
                    };
                    let _ = ui.upgrade_in_event_loop(move |ui| ui.set_update_label(label.into()));
                });
        });
    }
    let mut icecast_sender: Option<freeshow_record::IcecastSender> = None;
    let mut last_icecast = None;
    let mut last_stage_second = 0;
    let mut last_timer_label = String::new();
    clock_timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(100),
        move || {
            {
                let mut current = clock_session.borrow_mut();
                if let Some(request) = current.take_feed_request(std::time::Instant::now()) {
                    let _ = feed_tx.send(request);
                }
                let mut fed = false;
                while let Ok(results) = feed_rx.try_recv() {
                    current.apply_feed_results(results);
                    fed = true;
                }
                // New headlines show up in an open RSS tab without waiting for a click.
                if fed
                    && current.tab == "rss"
                    && let Some(ui) = clock_ui.upgrade()
                {
                    ui.set_drawer_rows(drawer_rows(&current));
                }
                // The timer button in Functions counts down with the timer.
                if current.tab == "functions" {
                    let label = current.timer_label();
                    if label != last_timer_label
                        && let Some(ui) = clock_ui.upgrade()
                    {
                        ui.set_drawer_rows(drawer_rows(&current));
                        last_timer_label = label;
                    }
                }
                // Video stills decoded in the background replace their placeholder icons.
                if VIDEO_STILLS_READY.swap(false, std::sync::atomic::Ordering::Relaxed)
                    && current.tab == "media"
                    && let Some(ui) = clock_ui.upgrade()
                {
                    ui.set_drawer_rows(drawer_rows(&current));
                }
                if let Ok(update) = update_rx.try_recv() {
                    current.apply_update_alert(&update);
                    if let Some(ui) = clock_ui.upgrade() {
                        let ready = update.newer && update.download_url.is_some();
                        ui.set_update_label(if ready { "Get update" } else { "" }.into());
                        refresh(&ui, &current);
                    }
                    if update.newer {
                        *clock_update.borrow_mut() = Some(update);
                    }
                }
            }
            if let Some(ui) = clock_ui.upgrade() {
                show_now_playing(&ui, &clock_session.borrow());
            }
            let ran = clock_session.borrow_mut().run_due_actions();
            {
                let current = clock_session.borrow();
                let destination = current
                    .icecast
                    .as_ref()
                    .filter(|(_, enabled)| *enabled)
                    .map(|(destination, _)| destination);
                if destination != last_icecast.as_ref() {
                    last_icecast = destination.cloned();
                    if icecast_sender.is_none() && destination.is_some() {
                        match freeshow_record::IcecastSender::new() {
                            Ok(sender) => icecast_sender = Some(sender),
                            Err(err) => tracing::error!("could not start Icecast: {err}"),
                        }
                    }
                    if let Some(sender) = &icecast_sender {
                        sender.update(last_icecast.clone());
                    }
                }
            }
            if ran {
                let plays = clock_session.borrow_mut().take_plays();
                let playback = LivePlayback {
                    audio: Rc::clone(&clock_audio),
                    video: Rc::clone(&clock_video),
                    timer: Rc::clone(&clock_video_timer),
                };
                let fades = picture_fades_in(&plays[0], &clock_session.borrow());
                start_plays(
                    plays,
                    &clock_session,
                    &playback,
                    &clock_output,
                    &clock_preview,
                    &clock_stage,
                    &clock_last_output,
                    &clock_last,
                );
                if !fades {
                    let audience = audience_frame(&clock_session.borrow());
                    *clock_last_output.borrow_mut() = audience.clone();
                    publish_audience(&clock_output, audience.clone());
                    publish(&clock_preview, audience);
                }
            }
            // Stage clocks and timers count whole seconds, so the foldback is redrawn once a
            // second, after an operator change, or when asked, instead of every tick.
            let second = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs());
            let request = stage_frame_rx.try_recv().ok();
            if STAGE_DIRTY.replace(false) || ran || request.is_some() || second != last_stage_second
            {
                last_stage_second = second;
                let frame = stage_frame(&clock_session.borrow());
                if let Some(request) = request {
                    request.publish(&frame);
                }
                *clock_last.borrow_mut() = frame.clone();
                publish(&clock_stage, frame);
            }
        },
    );
    place(control.window(), 40, 40);
    apply_window_profile(
        control.window(),
        output.window(),
        stage.window(),
        &settings.borrow(),
    );
    {
        // Closing the operator window quits FreeShow even while an output is open,
        // after saving text the operator typed but the autosave has not caught yet.
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        control.window().on_close_requested(move || {
            if let Some(ui) = ui.upgrade() {
                let mut session = session.borrow_mut();
                let lyrics = ui.get_lyrics();
                if session.tab == "shows" && session.lyrics() != lyrics.as_str() {
                    session.save_lyrics(lyrics.as_str());
                }
                if ui.get_text_mode() {
                    let _ = session.library_action("show-text", ui.get_show_text().as_str());
                }
            }
            let _ = slint::quit_event_loop();
            slint::CloseRequestResponse::HideWindow
        });
    }
    tracing::info!("control window ready");
    let result = control.run();
    freeshow_record::ServiceCapture.stop_all();
    result?;
    Ok(())
}

/// A background thread for weather and RSS fetches so a slow network never stalls the UI clock.
fn spawn_feed_worker() -> (
    mpsc::Sender<session::FeedRequest>,
    mpsc::Receiver<session::FeedResults>,
) {
    let (request_tx, request_rx) = mpsc::channel::<session::FeedRequest>();
    let (result_tx, result_rx) = mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("feeds".into())
        .spawn(move || {
            for request in request_rx {
                let results = request.fetch(
                    |lat, lon| feeds::fetch_weather(lat, lon, feeds::OPEN_METEO),
                    feeds::fetch_rss,
                );
                if result_tx.send(results).is_err() {
                    break;
                }
            }
        });
    (request_tx, result_rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_text_stays_readable() {
        let rgb = |r, g, b| Rgb { r, g, b };
        // Papyrus: dark navy text on a dark navy accent becomes light.
        assert_eq!(
            readable_on(rgb(0x00, 0x02, 0x42), rgb(0x00, 0x03, 0x61)),
            rgb(240, 240, 255)
        );
        // The default theme keeps its own text on green.
        assert_eq!(
            readable_on(rgb(0xf0, 0xf0, 0xff), rgb(0x2e, 0xcc, 0x71)),
            rgb(0xf0, 0xf0, 0xff)
        );
        // Light text on a pale accent becomes dark.
        assert_eq!(
            readable_on(rgb(0xff, 0xff, 0xff), rgb(0xf8, 0xf8, 0xf0)),
            rgb(16, 16, 20)
        );
    }
}
