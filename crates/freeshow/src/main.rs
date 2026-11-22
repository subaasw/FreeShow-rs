// SPDX-License-Identifier: GPL-3.0-only

mod church_import;
mod desktop;
mod lang;
mod live;
mod present;
mod runtime;
mod service_check;
mod session;
mod stage;
mod theme;

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
use live::{OpeningFade, audience_frame, motion_frame, stage_frame};
use present::{FrameSlot, attach_frame};
use session::{DrawerAction, PendingPlay, Session};
use slint::winit_030::WinitWindowAccessor;

slint::slint! {
    import { LineEdit, Slider, TextEdit } from "std-widgets.slint";

    export global OperatorStyle {
        in-out property <float> font-scale: 1;
        in-out property <color> hover: rgba(255, 255, 255, 0.05);
        in-out property <color> focus-fill: rgba(255, 255, 255, 0.1);
        in-out property <length> chrome-radius: 4px;
    }

    struct DrawerTab { id: string, label: string, short: string }

    export struct DrawerRowData {
        name: string,
        thumb: image,
        show-thumb: bool,
    }

    export struct SlideRowData {
        label: string,
        index: int,
        has-color: bool,
        group-color: color,
        group-text: color,
    }

    export struct SlideGridRow {
        items: [SlideRowData],
    }

    component RailButton inherits Rectangle {
        in property <string> label;
        in property <bool> active;
        in property <color> primary;
        in property <color> accent;
        in property <color> text-color;
        in property <color> accent-text;
        callback clicked();
        height: 58px;
        border-radius: OperatorStyle.chrome-radius;
        background: root.active ? root.accent : root.primary;
        Rectangle {
            width: 100%;
            height: 100%;
            border-radius: parent.border-radius;
            background: root.active ? transparent : pointer.has-hover ? OperatorStyle.hover : (key-focus.has-focus ? OperatorStyle.focus-fill : transparent);
        }
        Text {
            text: root.label;
            color: root.active ? root.accent-text : root.text-color;
            font-size: 12px * OperatorStyle.font-scale;
            horizontal-alignment: center;
            vertical-alignment: center;
            wrap: word-wrap;
            width: parent.width - 8px;
        }
        key-focus := FocusScope {
            width: 100%;
            height: 100%;
        }
        pointer := TouchArea {
            width: 100%;
            height: 100%;
            clicked => {
                key-focus.focus();
                root.clicked();
            }
        }
    }

    component BoxButton inherits Rectangle {
        in property <string> label;
        in property <color> accent;
        in property <color> text-color;
        in property <color> accent-text;
        callback clicked();
        horizontal-stretch: 1;
        height: 28px;
        border-radius: OperatorStyle.chrome-radius;
        background: root.accent;
        Rectangle {
            width: 100%;
            height: 100%;
            border-radius: parent.border-radius;
            background: pointer.has-hover ? OperatorStyle.hover : (key-focus.has-focus ? OperatorStyle.focus-fill : transparent);
        }
        Text {
            text: root.label;
            color: root.accent-text;
            font-size: 12px * OperatorStyle.font-scale;
            horizontal-alignment: center;
            vertical-alignment: center;
        }
        key-focus := FocusScope {
            width: 100%;
            height: 100%;
        }
        pointer := TouchArea {
            width: 100%;
            height: 100%;
            clicked => {
                key-focus.focus();
                root.clicked();
            }
        }
    }

    component HeaderButton inherits Rectangle {
        in property <string> label;
        in property <color> accent;
        in property <color> text-color;
        in property <color> accent-text;
        callback clicked();
        width: 88px;
        height: 32px;
        border-radius: OperatorStyle.chrome-radius;
        background: root.accent;
        Rectangle {
            width: 100%;
            height: 100%;
            border-radius: parent.border-radius;
            background: pointer.has-hover ? OperatorStyle.hover : (key-focus.has-focus ? OperatorStyle.focus-fill : transparent);
        }
        Text {
            text: root.label;
            color: root.accent-text;
            font-size: 13px * OperatorStyle.font-scale;
            horizontal-alignment: center;
            vertical-alignment: center;
        }
        key-focus := FocusScope {
            width: 100%;
            height: 100%;
        }
        pointer := TouchArea {
            width: 100%;
            height: 100%;
            clicked => {
                key-focus.focus();
                root.clicked();
            }
        }
    }

    component ListRow inherits Rectangle {
        in property <string> label;
        in property <bool> active;
        in property <image> thumb;
        in property <bool> show-thumb;
        in property <color> accent;
        in property <color> text-color;
        in property <color> accent-text;
        in property <bool> has-color: false;
        in property <bool> full-color: false;
        in property <color> group-color: #00000000;
        in property <color> group-text: #f0f0ff;
        callback clicked();
        height: root.show-thumb ? 48px : 36px;
        border-radius: OperatorStyle.chrome-radius;
        background: root.active ? root.accent : (root.full-color && root.has-color ? root.group-color : (pointer.has-hover ? OperatorStyle.hover : (key-focus.has-focus ? OperatorStyle.focus-fill : transparent)));
        HorizontalLayout {
            padding-left: 8px;
            padding-right: 8px;
            spacing: 8px;
            alignment: center;
            if root.show-thumb: Image {
                source: root.thumb;
                width: 56px;
                height: 40px;
                image-fit: contain;
            }
            Text {
                text: root.label;
                color: root.active ? root.accent-text : (root.has-color ? root.group-text : root.text-color);
                font-size: 13px * OperatorStyle.font-scale;
                vertical-alignment: center;
                horizontal-stretch: 1;
                overflow: elide;
            }
        }
        if root.has-color && !root.full-color && !root.active: Rectangle {
            y: parent.height - 2px;
            height: 2px;
            width: 100%;
            background: root.group-color;
        }
        key-focus := FocusScope {
            width: 100%;
            height: 100%;
        }
        pointer := TouchArea {
            width: 100%;
            height: 100%;
            clicked => {
                key-focus.focus();
                root.clicked();
            }
        }
    }

    export component ControlWindow inherits Window {
        title: "FreeShow";
        in property <string> theme-font-family;
        default-font-family: root.theme-font-family;
        default-font-size: 14px * OperatorStyle.font-scale;
        in property <color> primary: #242832;
        in property <color> primary-lighter: #2f3542;
        in property <color> primary-darker: #191923;
        in property <color> primary-darkest: #12121c;
        in property <color> text-color: #f0f0ff;
        in property <color> accent: #F0008C;
        in property <color> accent-text: #f0f0ff;
        in-out property <string> active-tab: "shows";
        in-out property <string> panel-title: "Shows";
        in property <string> data-path: "Data folder not set";
        in-out property <[DrawerRowData]> drawer-rows;
        in-out property <[SlideGridRow]> slide-grid;
        in-out property <int> slides-columns: 1;
        in-out property <bool> full-colors: false;
        in-out property <[string]> project-names;
        in-out property <string> project-title: "Project";
        in-out property <string> lyrics: "";
        in-out property <string> group: "";
        in-out property <float> volume: 100;
        in-out property <int> selected-drawer: -1;
        in-out property <int> selected-slide: -1;
        in-out property <int> selected-project: -1;
        in-out property <image> preview;
        in property <string> import-status;
        in-out property <string> chrome-status: "";
        in property <bool> import-running;
        callback cancel-import();
        callback open-output();
        callback open-stage();
        callback choose-screen();
        in-out property <string> screen-label: "Screen";
        in-out property <string> search-query: "";
        in-out property <int> drawer-width: 240;
        in-out property <int> project-width: 260;
        in-out property <int> drawer-height: 300;
        in-out property <bool> focus-mode: false;
        in-out property <bool> guide-visible: false;
        in-out property <string> guide-text: "";
        in-out property <bool> show-shortcuts: false;
        in-out property <string> shortcut-text: "";
        in property <bool> show-labels: true;
        callback dismiss-guide();
        callback save-shortcuts(string);
        callback save-layouts(string);
        callback save-variables(string);
        in-out property <bool> show-layouts: false;
        in-out property <string> layout-text: "";
        in-out property <bool> show-variables: false;
        in-out property <string> variable-text: "";
        callback save-timeline(string);
        callback play-timeline();
        in-out property <bool> show-timeline: false;
        in-out property <string> timeline-text: "";
        callback hotkey(string, bool, bool, bool);
        callback set-search(string);
        callback select-tab(string);
        callback choose-drawer(int);
        callback choose-slide(int);
        callback save-lyrics(string);
        callback save-group(string);
        callback nudge-box(int, int, int, int);
        in-out property <string> item-label: "Box 1 of 1";
        in-out property <string> font-family: "";
        in-out property <bool> item-bold: false;
        callback prev-item();
        callback next-item();
        callback add-item();
        callback remove-item();
        callback adjust-font(int);
        callback toggle-bold();
        callback set-font-family(string);
        callback go-slide(string);
        callback move-project(int);
        callback choose-project(int);
        callback add-to-project();
        callback create-show();
        callback add-slide();
        callback create-project();
        callback search(string);
        callback set-volume(float);

        preferred-width: 1280px;
        preferred-height: 800px;
        background: root.primary-darkest;

        in-out property <[DrawerTab]> tabs: [
            { id: "shows", label: "Shows", short: "S" },
            { id: "media", label: "Media", short: "M" },
            { id: "player", label: "Player", short: "P" },
            { id: "audio", label: "Audio", short: "A" },
            { id: "overlays", label: "Overlays", short: "O" },
            { id: "templates", label: "Templates", short: "T" },
            { id: "scripture", label: "Scripture", short: "B" },
            { id: "calendar", label: "Calendar", short: "C" },
            { id: "functions", label: "Functions", short: "F" },
        ];

        forward-focus: window-keys;
        window-keys := FocusScope {
            key-pressed(event) => {
                if (event.text == Key.F11) {
                    root.hotkey("F11", event.modifiers.control, event.modifiers.alt, event.modifiers.shift);
                } else if (event.text == Key.F1) {
                    root.hotkey("F1", event.modifiers.control, event.modifiers.alt, event.modifiers.shift);
                } else if (event.text == Key.RightArrow) {
                    root.hotkey("Right", event.modifiers.control, event.modifiers.alt, event.modifiers.shift);
                } else if (event.text == Key.LeftArrow) {
                    root.hotkey("Left", event.modifiers.control, event.modifiers.alt, event.modifiers.shift);
                } else {
                    root.hotkey(event.text, event.modifiers.control, event.modifiers.alt, event.modifiers.shift);
                }
                accept
            }
        VerticalLayout {
            spacing: 0;
            Rectangle {
                height: 48px;
                background: root.primary-darker;
                HorizontalLayout {
                    padding-left: 16px;
                    padding-right: 12px;
                    spacing: 8px;
                    alignment: center;
                    Text {
                        text: "FreeShow";
                        color: root.text-color;
                        font-size: 18px * OperatorStyle.font-scale;
                        font-weight: 700;
                        vertical-alignment: center;
                    }
                    LineEdit {
                        width: 220px;
                        height: 28px;
                        text <=> root.search-query;
                        placeholder-text: "Search";
                        edited(value) => { root.set-search(value); }
                    }
                    Rectangle { horizontal-stretch: 1; background: root.primary-darker; }
                    HeaderButton {
                        label: root.screen-label;
                        accent: root.accent;
                        text-color: root.text-color;
                        accent-text: root.accent-text;
                        clicked => { root.choose-screen(); }
                    }
                    HeaderButton {
                        label: root.show-labels ? "Output" : "O";
                        accent: root.accent;
                        text-color: root.text-color;
                        accent-text: root.accent-text;
                        clicked => { root.open-output(); }
                    }
                    HeaderButton {
                        label: root.show-labels ? "Stage" : "S";
                        accent: root.accent;
                        text-color: root.text-color;
                        accent-text: root.accent-text;
                        clicked => { root.open-stage(); }
                    }
                }
            }
            HorizontalLayout {
                spacing: 0;
                Rectangle {
                    width: 96px;
                    background: root.primary-darker;
                    VerticalLayout {
                        alignment: start;
                        spacing: 0;
                        for tab in root.tabs: RailButton {
                            label: root.show-labels ? tab.label : tab.short;
                            active: root.active-tab == tab.id;
                            primary: root.primary;
                            accent: root.accent;
                            text-color: root.text-color;
                            accent-text: root.accent-text;
                            clicked => {
                                root.active-tab = tab.id;
                                root.panel-title = tab.label;
                                root.select-tab(tab.id);
                            }
                        }
                    }
                }
                VerticalLayout {
                    horizontal-stretch: 1;
                    spacing: 0;
                    Rectangle {
                        height: 40px;
                        background: root.primary;
                        HorizontalLayout {
                            padding-left: 12px;
                            padding-right: 8px;
                            spacing: 8px;
                            alignment: center;
                            Text {
                                text: root.panel-title;
                                color: root.text-color;
                                font-size: 16px * OperatorStyle.font-scale;
                                vertical-alignment: center;
                            }
                            Rectangle { horizontal-stretch: 1; background: root.primary; }
                            HeaderButton {
                                label: "Show";
                                accent: root.primary-lighter;
                                text-color: root.text-color;
                                accent-text: root.text-color;
                                clicked => { root.create-show(); }
                            }
                            HeaderButton {
                                label: "Slide";
                                accent: root.primary-lighter;
                                text-color: root.text-color;
                                accent-text: root.text-color;
                                clicked => { root.add-slide(); }
                            }
                            HeaderButton {
                                label: "Save";
                                accent: root.primary-lighter;
                                text-color: root.text-color;
                                accent-text: root.text-color;
                                clicked => { root.save-lyrics(root.lyrics); }
                            }
                            Slider {
                                width: 120px;
                                maximum: 100;
                                value <=> root.volume;
                                changed(value) => { root.set-volume(value); }
                            }
                            HeaderButton {
                                label: "Go";
                                accent: root.accent;
                                text-color: root.text-color;
                                accent-text: root.accent-text;
                                clicked => { root.go-slide(root.lyrics); }
                            }
                        }
                    }
                    if root.active-tab == "scripture": Rectangle {
                        height: 40px;
                        background: root.primary-darker;
                        LineEdit {
                            width: parent.width - 16px;
                            x: 8px;
                            y: 6px;
                            placeholder-text: "Search scripture";
                            edited(value) => { root.search(value); }
                        }
                    }
                    HorizontalLayout {
                        vertical-stretch: 1;
                        spacing: 0;
                        Flickable {
                            width: root.slides-columns > 1 ? 360px : 280px;
                            VerticalLayout {
                                alignment: start;
                                spacing: 2px;
                                for row in root.slide-grid: HorizontalLayout {
                                    spacing: 2px;
                                    for slide in row.items: ListRow {
                                        width: ((root.slides-columns > 1 ? 360px : 280px) / root.slides-columns);
                                        label: slide.label;
                                        active: root.selected-slide == slide.index;
                                        accent: root.accent;
                                        text-color: root.text-color;
                                        accent-text: root.accent-text;
                                        has-color: slide.has-color;
                                        full-color: root.full-colors;
                                        group-color: slide.group-color;
                                        group-text: slide.group-text;
                                        clicked => { root.choose-slide(slide.index); }
                                    }
                                }
                            }
                        }
                        VerticalLayout {
                            horizontal-stretch: 1;
                            padding: 12px;
                            spacing: 8px;
                            alignment: start;
                            Rectangle {
                                height: 180px;
                                background: black;
                                Image {
                                    source: root.preview;
                                    width: 100%;
                                    height: 100%;
                                    image-fit: contain;
                                }
                            }
                            LineEdit {
                                text <=> root.group;
                                placeholder-text: "Group";
                                edited(value) => { root.save-group(value); }
                            }
                            HorizontalLayout {
                                spacing: 6px;
                                Text {
                                    text: root.item-label;
                                    color: root.text-color;
                                    font-size: 12px * OperatorStyle.font-scale;
                                    vertical-alignment: center;
                                }
                                BoxButton { label: "Prev box"; accent: root.primary-lighter; text-color: root.text-color; accent-text: root.text-color; clicked => { root.prev-item(); } }
                                BoxButton { label: "Next box"; accent: root.primary-lighter; text-color: root.text-color; accent-text: root.text-color; clicked => { root.next-item(); } }
                                BoxButton { label: "Add box"; accent: root.primary-lighter; text-color: root.text-color; accent-text: root.text-color; clicked => { root.add-item(); } }
                                BoxButton { label: "Remove"; accent: root.primary-lighter; text-color: root.text-color; accent-text: root.text-color; clicked => { root.remove-item(); } }
                            }
                            HorizontalLayout {
                                spacing: 6px;
                                BoxButton { label: "Larger"; accent: root.primary-lighter; text-color: root.text-color; accent-text: root.text-color; clicked => { root.adjust-font(10); } }
                                BoxButton { label: "Smaller"; accent: root.primary-lighter; text-color: root.text-color; accent-text: root.text-color; clicked => { root.adjust-font(-10); } }
                                BoxButton { label: root.item-bold ? "Unbold" : "Bold"; accent: root.primary-lighter; text-color: root.text-color; accent-text: root.text-color; clicked => { root.toggle-bold(); } }
                                LineEdit {
                                    horizontal-stretch: 1;
                                    text <=> root.font-family;
                                    placeholder-text: "Font family";
                                    edited(value) => { root.set-font-family(value); }
                                }
                            }
                            HorizontalLayout {
                                spacing: 6px;
                                BoxButton { label: "Left"; accent: root.primary-lighter; text-color: root.text-color; accent-text: root.text-color; clicked => { root.nudge-box(-40, 0, 0, 0); } }
                                BoxButton { label: "Right"; accent: root.primary-lighter; text-color: root.text-color; accent-text: root.text-color; clicked => { root.nudge-box(40, 0, 0, 0); } }
                                BoxButton { label: "Up"; accent: root.primary-lighter; text-color: root.text-color; accent-text: root.text-color; clicked => { root.nudge-box(0, -40, 0, 0); } }
                                BoxButton { label: "Down"; accent: root.primary-lighter; text-color: root.text-color; accent-text: root.text-color; clicked => { root.nudge-box(0, 40, 0, 0); } }
                            }
                            HorizontalLayout {
                                spacing: 6px;
                                BoxButton { label: "Wider"; accent: root.primary-lighter; text-color: root.text-color; accent-text: root.text-color; clicked => { root.nudge-box(0, 0, 40, 0); } }
                                BoxButton { label: "Narrow"; accent: root.primary-lighter; text-color: root.text-color; accent-text: root.text-color; clicked => { root.nudge-box(0, 0, -40, 0); } }
                                BoxButton { label: "Taller"; accent: root.primary-lighter; text-color: root.text-color; accent-text: root.text-color; clicked => { root.nudge-box(0, 0, 0, 40); } }
                                BoxButton { label: "Shorter"; accent: root.primary-lighter; text-color: root.text-color; accent-text: root.text-color; clicked => { root.nudge-box(0, 0, 0, -40); } }
                            }
                            TextEdit {
                                text <=> root.lyrics;
                                vertical-stretch: 1;
                                wrap: word-wrap;
                            }
                        }
                    }
                }
                Rectangle {
                    width: root.focus-mode ? 0px : root.project-width * 1px;
                    background: root.primary;
                    VerticalLayout {
                        padding: 12px;
                        spacing: 8px;
                        alignment: start;
                        Text {
                            text: root.project-title;
                            color: root.text-color;
                            font-size: 16px * OperatorStyle.font-scale;
                            font-weight: 700;
                        }
                        HorizontalLayout {
                            spacing: 6px;
                            BoxButton { label: "Up"; accent: root.primary-lighter; text-color: root.text-color; accent-text: root.text-color; clicked => { root.move-project(-1); } }
                            BoxButton { label: "Down"; accent: root.primary-lighter; text-color: root.text-color; accent-text: root.text-color; clicked => { root.move-project(1); } }
                            BoxButton { label: "Add"; accent: root.accent; text-color: root.text-color; accent-text: root.accent-text; clicked => { root.add-to-project(); } }
                            BoxButton { label: "New"; accent: root.accent; text-color: root.text-color; accent-text: root.accent-text; clicked => { root.create-project(); } }
                        }
                        if root.project-names.length == 0: Text {
                            text: "No project open";
                            color: root.text-color;
                            font-size: 14px * OperatorStyle.font-scale;
                        }
                        if root.project-names.length > 0: VerticalLayout {
                            spacing: 0;
                            alignment: start;
                            for item[index] in root.project-names: ListRow {
                                label: item;
                                active: root.selected-project == index;
                                accent: root.accent;
                                text-color: root.text-color;
                                accent-text: root.accent-text;
                                clicked => {
                                    root.selected-project = index;
                                    root.choose-project(index);
                                }
                            }
                        }
                    }
                }
            }
            if root.guide-visible: Rectangle {
                height: 160px;
                background: root.primary-darker;
                HorizontalLayout {
                    padding: 12px;
                    spacing: 12px;
                    Text {
                        text: root.guide-text;
                        color: root.text-color;
                        font-size: 14px * OperatorStyle.font-scale;
                        wrap: word-wrap;
                        vertical-alignment: top;
                        horizontal-stretch: 1;
                    }
                    HeaderButton {
                        label: "Dismiss";
                        accent: root.accent;
                        text-color: root.text-color;
                        accent-text: root.accent-text;
                        clicked => { root.dismiss-guide(); }
                    }
                }
            }
            if root.show-shortcuts: Rectangle {
                height: 180px;
                background: root.primary;
                HorizontalLayout {
                    padding: 8px;
                    spacing: 8px;
                    TextEdit {
                        text <=> root.shortcut-text;
                        vertical-stretch: 1;
                        horizontal-stretch: 1;
                        wrap: word-wrap;
                    }
                    VerticalLayout {
                        spacing: 8px;
                        alignment: start;
                        HeaderButton {
                            label: "Save";
                            accent: root.accent;
                            text-color: root.text-color;
                            accent-text: root.accent-text;
                            clicked => { root.save-shortcuts(root.shortcut-text); }
                        }
                        HeaderButton {
                            label: "Close";
                            accent: root.primary-lighter;
                            text-color: root.text-color;
                            accent-text: root.text-color;
                            clicked => { root.show-shortcuts = false; }
                        }
                    }
                }
            }
            if root.show-layouts: Rectangle {
                height: 180px;
                background: root.primary;
                HorizontalLayout {
                    padding: 8px;
                    spacing: 8px;
                    TextEdit {
                        text <=> root.layout-text;
                        vertical-stretch: 1;
                        horizontal-stretch: 1;
                        wrap: word-wrap;
                    }
                    VerticalLayout {
                        spacing: 8px;
                        alignment: start;
                        HeaderButton {
                            label: "Save";
                            accent: root.accent;
                            text-color: root.text-color;
                            accent-text: root.accent-text;
                            clicked => { root.save-layouts(root.layout-text); }
                        }
                        HeaderButton {
                            label: "Close";
                            accent: root.primary-lighter;
                            text-color: root.text-color;
                            accent-text: root.text-color;
                            clicked => { root.show-layouts = false; }
                        }
                    }
                }
            }
            if root.show-variables: Rectangle {
                height: 180px;
                background: root.primary;
                HorizontalLayout {
                    padding: 8px;
                    spacing: 8px;
                    TextEdit {
                        text <=> root.variable-text;
                        vertical-stretch: 1;
                        horizontal-stretch: 1;
                        wrap: word-wrap;
                    }
                    VerticalLayout {
                        spacing: 8px;
                        alignment: start;
                        HeaderButton {
                            label: "Save";
                            accent: root.accent;
                            text-color: root.text-color;
                            accent-text: root.accent-text;
                            clicked => { root.save-variables(root.variable-text); }
                        }
                        HeaderButton {
                            label: "Close";
                            accent: root.primary-lighter;
                            text-color: root.text-color;
                            accent-text: root.text-color;
                            clicked => { root.show-variables = false; }
                        }
                    }
                }
            }
            if root.show-timeline: Rectangle {
                height: 180px;
                background: root.primary;
                HorizontalLayout {
                    padding: 8px;
                    spacing: 8px;
                    TextEdit {
                        text <=> root.timeline-text;
                        vertical-stretch: 1;
                        horizontal-stretch: 1;
                        wrap: word-wrap;
                    }
                    VerticalLayout {
                        spacing: 8px;
                        alignment: start;
                        HeaderButton {
                            label: "Save";
                            accent: root.accent;
                            text-color: root.text-color;
                            accent-text: root.accent-text;
                            clicked => { root.save-timeline(root.timeline-text); }
                        }
                        HeaderButton {
                            label: "Play";
                            accent: root.accent;
                            text-color: root.text-color;
                            accent-text: root.accent-text;
                            clicked => { root.play-timeline(); }
                        }
                        HeaderButton {
                            label: "Close";
                            accent: root.primary-lighter;
                            text-color: root.text-color;
                            accent-text: root.text-color;
                            clicked => { root.show-timeline = false; }
                        }
                    }
                }
            }
            if !root.focus-mode: Rectangle {
                height: root.drawer-height * 1px;
                background: root.primary;
                Flickable {
                    HorizontalLayout {
                        spacing: 0;
                        alignment: start;
                        for item[index] in root.drawer-rows: ListRow {
                            width: 220px;
                            label: item.name;
                            thumb: item.thumb;
                            show-thumb: item.show-thumb;
                            active: root.selected-drawer == index;
                            accent: root.accent;
                            text-color: root.text-color;
                            accent-text: root.accent-text;
                            clicked => { root.choose-drawer(index); }
                        }
                    }
                }
            }
            Rectangle {
                height: 28px;
                background: root.primary-darker;
                HorizontalLayout {
                    padding-left: 12px;
                    alignment: center;
                    Text {
                        text: root.chrome-status;
                        color: root.text-color;
                        font-size: 12px * OperatorStyle.font-scale;
                        vertical-alignment: center;
                    }
                    Text {
                        text: root.import-status == "" ? root.data-path : root.import-status;
                        overflow: elide;
                        horizontal-stretch: 1;
                        color: root.text-color;
                        font-size: 12px * OperatorStyle.font-scale;
                        vertical-alignment: center;
                    }
                    if root.import-running: HeaderButton {
                        label: "Cancel import";
                        accent: root.accent;
                        text-color: root.text-color;
                        accent-text: root.accent-text;
                        clicked => { root.cancel-import(); }
                    }
                }
            }
        }
        }
    }

    export component OutputWindow inherits Window {
        in-out property <image> frame;
        title: "FreeShow Output";
        background: #000;
        preferred-width: 960px;
        preferred-height: 540px;
        Image {
            source: root.frame;
            width: 100%;
            height: 100%;
            image-fit: contain;
        }
    }

    export component StageWindow inherits Window {
        in-out property <image> frame;
        title: "FreeShow Stage";
        background: #000;
        preferred-width: 960px;
        preferred-height: 540px;
        Image {
            source: root.frame;
            width: 100%;
            height: 100%;
            image-fit: contain;
        }
    }
}

fn color(rgb: Rgb) -> slint::Color {
    slint::Color::from_rgb_u8(rgb.r, rgb.g, rgb.b)
}

fn color_rgba(color: freeshow_core::Rgba) -> slint::Color {
    slint::Color::from_argb_u8(color.a, color.r, color.g, color.b)
}

fn apply_theme(ui: &ControlWindow, theme: &theme::OperatorTheme) {
    let palette = &theme.palette;
    ui.set_primary(color(palette.primary));
    ui.set_primary_lighter(color(palette.primary_lighter));
    ui.set_primary_darker(color(palette.primary_darker));
    ui.set_primary_darkest(color(palette.primary_darkest));
    ui.set_text_color(color(palette.text));
    ui.set_accent(color(palette.secondary));
    ui.set_accent_text(color(palette.secondary_text));
    let style = ui.global::<OperatorStyle>();
    style.set_hover(color_rgba(palette.hover));
    style.set_focus_fill(color_rgba(palette.focus));
    style.set_chrome_radius(theme.border_radius);
}

fn slide_grid(session: &Session) -> slint::ModelRc<SlideGridRow> {
    let columns = session.slides_list_columns().max(1);
    let full_colors = session.preferences.full_colors();
    let rows = session
        .slide_rows()
        .into_iter()
        .enumerate()
        .collect::<Vec<_>>();
    let grid = rows
        .chunks(columns)
        .map(|chunk| SlideGridRow {
            items: slint::ModelRc::new(slint::VecModel::from(
                chunk
                    .iter()
                    .map(|(index, row)| slide_row_data(*index, row, full_colors))
                    .collect::<Vec<_>>(),
            )),
        })
        .collect::<Vec<_>>();
    slint::ModelRc::new(slint::VecModel::from(grid))
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
        }
    } else {
        SlideRowData {
            label: row.text.as_str().into(),
            index: index as i32,
            has_color: false,
            group_color: slint::Color::from_argb_u8(0, 0, 0, 0),
            group_text: default_text,
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
            })
            .collect::<Vec<_>>()
    } else {
        session
            .drawer()
            .into_iter()
            .map(|name| DrawerRowData {
                name: name.into(),
                thumb: slint::Image::default(),
                show_thumb: false,
            })
            .collect()
    };
    slint::ModelRc::new(slint::VecModel::from(rows))
}

const THUMB_WIDTH: u32 = 56;
const THUMB_HEIGHT: u32 = 40;

fn media_thumb(entry: &MediaEntry) -> slint::Image {
    if entry.exists
        && entry.kind == MediaEntryKind::Image
        && let Some(image) = std::fs::read(&entry.path)
            .ok()
            .and_then(|bytes| decode_image(&bytes))
    {
        return slint_image(&fit_raster(&image, THUMB_WIDTH, THUMB_HEIGHT));
    }
    slint_image(&kind_thumb(entry))
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

fn refresh(ui: &ControlWindow, session: &Session) {
    ui.set_drawer_rows(drawer_rows(session));
    ui.set_slide_grid(slide_grid(session));
    ui.set_slides_columns(session.slides_list_columns() as i32);
    ui.set_full_colors(session.preferences.full_colors());
    ui.set_project_names(strings(&session.project_labels()));
    ui.set_project_title(session.project_title().into());
    ui.set_lyrics(session.lyrics().into());
    ui.set_group(session.group().into());
    ui.set_item_label(session.item_label().into());
    let typography = session.item_typography();
    ui.set_font_family(typography.family.into());
    ui.set_item_bold(typography.bold);
    ui.set_selected_drawer(match session.tab.as_str() {
        "shows" => selected(session.visible_drawer_index(session.show_index)),
        "scripture" => selected(session.visible_drawer_index(session.bible_index)),
        "media" => selected(session.visible_drawer_index(session.media_index)),
        "audio" => selected(session.visible_drawer_index(session.audio_index)),
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
    ui.set_selected_project(session.project_item as i32);
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
        for target in session.ndi_outputs() {
            let sender = freeshow_ndi::NdiSender::open_routed(
                &target.name,
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
            let Some(sender) = freeshow_record::WebrtcSender::open(
                &target.url,
                target.token.as_deref(),
                session.output_width,
                session.output_height,
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
    }
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
        let frame = to;
        submit_network(picture, &frame);
        for slot in &slots {
            slot.publish(frame.clone());
        }
        return;
    }
    let steps = (motion.duration_ms / 40).clamp(1, 250) as u16;
    let step = Rc::new(std::cell::Cell::new(0u16));
    let timer_cb = Rc::clone(&timer);
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(40),
        move || {
            let next = step.get().saturating_add(1);
            step.set(next);
            let frame = motion_frame(&from, &to, &motion, f32::from(next) / f32::from(steps));
            submit_network(picture, &frame);
            for slot in &slots {
                slot.publish(frame.clone());
            }
            if next >= steps {
                timer_cb.stop();
            }
        },
    );
}

fn sample_audio(session: &RefCell<Session>, audio: &RefCell<Option<AudioOutput>>) {
    let mut session = session.borrow_mut();
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
                },
            ) {
                Ok(output) => {
                    *playback.audio.borrow_mut() = Some(output);
                }
                Err(err) => tracing::error!("could not play audio: {err}"),
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
    let Some(seconds) = seconds else {
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
            if !current.advance_show() {
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

    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("femtovg-wgpu".into())
        .require_wgpu_30(slint::wgpu_30::WGPUConfiguration::default())
        .select()?;

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
    control.set_theme_font_family(theme.font_family.into());
    control
        .global::<OperatorStyle>()
        .set_font_scale(theme.font_scale);
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

    let output_slot = Rc::new(output_slot);
    let stage_slot = Rc::new(stage_slot);
    let preview_slot = Rc::new(preview_slot);
    let output_fade = Rc::new(slint::Timer::default());
    let stage_fade = Rc::new(slint::Timer::default());
    let next_slide = Rc::new(slint::Timer::default());
    let last_output = Rc::new(RefCell::new(clear_frame(output_width, output_height)));
    let last_stage = Rc::new(RefCell::new(clear_frame(output_width, output_height)));
    if session.slide_index.is_some() {
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
        apply_window_behavior(output.window(), session.always_on_top, session.kiosk);
    }
    if settings.show_stage {
        stage.show()?;
        place(stage.window(), 120, 140);
    }

    let session = Rc::new(RefCell::new(session));
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

    let output_toggle = output.as_weak();
    let settings_for_output = Rc::clone(&settings);
    let settings_path_for_output = settings_path.clone();
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
        }
        settings_for_output.borrow_mut().show_output = !visible;
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
        if let Err(err) = settings_for_stage.borrow().save(&settings_path_for_stage) {
            tracing::error!("could not save settings: {err}");
        }
    });

    let ui = control.as_weak();
    let session_for_tab = Rc::clone(&session);
    control.on_select_tab(move |tab| {
        session_for_tab.borrow_mut().tab = tab.to_string();
        if let Some(ui) = ui.upgrade() {
            refresh(&ui, &session_for_tab.borrow());
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

    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
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
                apply_window_behavior(
                    ui.window(),
                    session.borrow().always_on_top,
                    session.borrow().kiosk,
                );
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
                        current.blanked = true;
                        current.clear_output();
                        stop_video(&video_timer, &video_player);
                        *audio_out.borrow_mut() = None;
                        let blank = clear_frame(output_width, output_height);
                        next_slide.stop();
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
                            publish(&preview_slot, audience_frame(&current));
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
                DrawerAction::Present => {
                    session.borrow_mut().blanked = false;
                    stop_video(&video_timer, &video_player);
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
        control.on_choose_slide(move |index| {
            session.borrow_mut().select_slide(index.max(0) as usize);
            if let Some(ui) = ui.upgrade() {
                refresh(&ui, &session.borrow());
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
        let preview_slot = Rc::clone(&preview_slot);
        control.on_prev_item(move || {
            session.borrow_mut().prev_item();
            if let Some(ui) = ui.upgrade() {
                refresh(&ui, &session.borrow());
                publish(&preview_slot, audience_frame(&session.borrow()));
            }
        });
    }
    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        let preview_slot = Rc::clone(&preview_slot);
        control.on_next_item(move || {
            session.borrow_mut().next_item();
            if let Some(ui) = ui.upgrade() {
                refresh(&ui, &session.borrow());
                publish(&preview_slot, audience_frame(&session.borrow()));
            }
        });
    }
    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        let preview_slot = Rc::clone(&preview_slot);
        control.on_add_item(move || {
            if !session.borrow_mut().add_item_box() {
                tracing::error!("could not add a text box");
            }
            if let Some(ui) = ui.upgrade() {
                refresh(&ui, &session.borrow());
                publish(&preview_slot, audience_frame(&session.borrow()));
            }
        });
    }
    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        let preview_slot = Rc::clone(&preview_slot);
        control.on_remove_item(move || {
            if !session.borrow_mut().remove_item_box() {
                tracing::error!("could not remove the text box");
            }
            if let Some(ui) = ui.upgrade() {
                refresh(&ui, &session.borrow());
                publish(&preview_slot, audience_frame(&session.borrow()));
            }
        });
    }
    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        let preview_slot = Rc::clone(&preview_slot);
        control.on_adjust_font(move |delta| {
            if !session.borrow_mut().adjust_font(delta) {
                tracing::error!("could not change the font size");
            }
            if let Some(ui) = ui.upgrade() {
                refresh(&ui, &session.borrow());
                publish(&preview_slot, audience_frame(&session.borrow()));
            }
        });
    }
    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        let preview_slot = Rc::clone(&preview_slot);
        control.on_toggle_bold(move || {
            if !session.borrow_mut().toggle_bold() {
                tracing::error!("could not change bold");
            }
            if let Some(ui) = ui.upgrade() {
                refresh(&ui, &session.borrow());
                publish(&preview_slot, audience_frame(&session.borrow()));
            }
        });
    }
    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        let preview_slot = Rc::clone(&preview_slot);
        control.on_set_font_family(move |family| {
            if session.borrow().item_typography().family == family.as_str() {
                return;
            }
            if !session.borrow_mut().set_font_family(family.as_str()) {
                tracing::error!("could not change the font family");
            }
            if let Some(ui) = ui.upgrade() {
                refresh(&ui, &session.borrow());
                publish(&preview_slot, audience_frame(&session.borrow()));
            }
        });
    }

    {
        let session = Rc::clone(&session);
        let preview_slot = Rc::clone(&preview_slot);
        control.on_nudge_box(move |dx, dy, dw, dh| {
            if !session.borrow_mut().nudge_box(dx, dy, dw, dh) {
                tracing::error!("could not move the text box");
                return;
            }
            publish(&preview_slot, audience_frame(&session.borrow()));
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
                session.save_lyrics(text.as_str());
            }
            session.blanked = false;
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
        control.on_choose_project(move |index| {
            session.borrow_mut().project_item = index.max(0) as usize;
            if let Some(ui) = ui.upgrade() {
                refresh(&ui, &session.borrow());
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
        let preview_slot = Rc::clone(&preview_slot);
        control.on_create_show(move || {
            if !session.borrow_mut().create_show() {
                tracing::error!("could not create a show");
                return;
            }
            publish(&preview_slot, audience_frame(&session.borrow()));
            if let Some(ui) = ui.upgrade() {
                refresh(&ui, &session.borrow());
            }
        });
    }

    {
        let ui = control.as_weak();
        let session = Rc::clone(&session);
        let preview_slot = Rc::clone(&preview_slot);
        control.on_add_slide(move || {
            if !session.borrow_mut().add_slide() {
                tracing::error!("could not add a slide");
                return;
            }
            publish(&preview_slot, audience_frame(&session.borrow()));
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
    freeshow_midi::MidiInput::default().start(freeshow_remote::ShowBus {
        snapshot: Arc::clone(&snapshot),
        commands: command_tx,
    });
    freeshow_timecode::MtcInput::default().start();
    freeshow_timecode::LtcInput.start();
    if let Ok(url) = std::env::var("FREESHOW_RTMP")
        && !url.trim().is_empty()
    {
        let output_id = session
            .borrow()
            .network_output("rtmp")
            .unwrap_or_else(|| "default".into());
        let _ = freeshow_record::ServiceCapture.start_rtmp_routed(
            &url,
            output_width,
            output_height,
            &output_id,
        );
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
    remote_timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(100),
        move || {
            let mut batch = Vec::new();
            while let Ok(command) = command_rx.try_recv() {
                batch.push(command);
            }
            if batch.is_empty() {
                let started = remote_session
                    .borrow_mut()
                    .start_due_events(chrono::Utc::now());
                sample_audio(&remote_session, &remote_audio);
                let moved = remote_session.borrow().refresh_output;
                if started || moved {
                    remote_session.borrow_mut().refresh_output = false;
                    if moved {
                        let plays = remote_session.borrow_mut().take_plays();
                        let fades = picture_fades_in(&plays[0], &remote_session.borrow());
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
            remote_session.borrow_mut().refresh_output = false;
            let (audience, stage) = {
                let mut session = remote_session.borrow_mut();
                for command in batch {
                    session.apply_remote(command);
                }
                let audience = audience_frame(&session);
                let stage = stage_frame(&session);
                if let Ok(mut stored) = snapshot.lock() {
                    *stored = session.to_snapshot();
                }
                (audience, stage)
            };
            let plays = remote_session.borrow_mut().take_plays();
            let fades = picture_fades_in(&plays[0], &remote_session.borrow());
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
            if clear {
                stop_video(&remote_video_timer, &remote_video);
                *remote_audio.borrow_mut() = None;
                remote_next_slide.stop();
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
        },
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
    let mut icecast_sender: Option<freeshow_record::IcecastSender> = None;
    let mut last_icecast = None;
    clock_timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(100),
        move || {
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
            let frame = stage_frame(&clock_session.borrow());
            if let Ok(request) = stage_frame_rx.try_recv() {
                request.publish(&frame);
            }
            *clock_last.borrow_mut() = frame.clone();
            publish(&clock_stage, frame);
        },
    );
    place(control.window(), 40, 40);
    apply_window_profile(
        control.window(),
        output.window(),
        stage.window(),
        &settings.borrow(),
    );
    tracing::info!("control window ready");
    let result = control.run();
    freeshow_record::ServiceCapture.stop_all();
    result?;
    Ok(())
}
