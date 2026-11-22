// SPDX-License-Identifier: GPL-3.0-only

// The rv.data wire fields used by this importer, from FreeShow public/proto.
// Unknown fields are skipped. Keeping this subset requires no protoc at build time.
use prost::Message;

#[derive(Clone, PartialEq, Message)]
pub(super) struct Presentation {
    #[prost(message, optional, tag = "2")]
    pub uuid: Option<Uuid>,
    #[prost(string, tag = "3")]
    pub name: String,
    #[prost(string, tag = "7")]
    pub notes: String,
    #[prost(message, optional, tag = "10")]
    pub selected: Option<Uuid>,
    #[prost(message, repeated, tag = "11")]
    pub arrangements: Vec<Arrangement>,
    #[prost(message, repeated, tag = "12")]
    pub groups: Vec<CueGroup>,
    #[prost(message, repeated, tag = "13")]
    pub cues: Vec<Cue>,
    #[prost(message, optional, tag = "14")]
    pub ccli: Option<Ccli>,
    #[prost(message, optional, tag = "17")]
    pub timeline: Option<Timeline>,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct Timeline {
    #[prost(message, repeated, tag = "1")]
    pub cues: Vec<TimelineCue>,
    #[prost(double, tag = "5")]
    pub duration: f64,
    #[prost(bool, tag = "6")]
    pub r#loop: bool,
    #[prost(message, repeated, tag = "11")]
    pub cues_v2: Vec<TimelineCue>,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct TimelineCue {
    #[prost(double, tag = "1")]
    pub trigger_time: f64,
    #[prost(message, optional, tag = "2")]
    pub cue_id: Option<Uuid>,
    #[prost(string, tag = "3")]
    pub name: String,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct Uuid {
    #[prost(string, tag = "1")]
    pub value: String,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct Arrangement {
    #[prost(message, optional, tag = "1")]
    pub uuid: Option<Uuid>,
    #[prost(string, tag = "2")]
    pub name: String,
    #[prost(message, repeated, tag = "3")]
    pub groups: Vec<Uuid>,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct CueGroup {
    #[prost(message, optional, tag = "1")]
    pub group: Option<Group>,
    #[prost(message, repeated, tag = "2")]
    pub cues: Vec<Uuid>,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct Group {
    #[prost(message, optional, tag = "1")]
    pub uuid: Option<Uuid>,
    #[prost(string, tag = "2")]
    pub name: String,
    #[prost(message, optional, tag = "3")]
    pub color: Option<Color>,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct Color {
    #[prost(float, tag = "1")]
    pub red: f32,
    #[prost(float, tag = "2")]
    pub green: f32,
    #[prost(float, tag = "3")]
    pub blue: f32,
    #[prost(float, tag = "4")]
    pub alpha: f32,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct Cue {
    #[prost(message, optional, tag = "1")]
    pub uuid: Option<Uuid>,
    #[prost(string, tag = "2")]
    pub name: String,
    #[prost(enumeration = "CompletionTarget", tag = "3")]
    pub completion_target: i32,
    #[prost(enumeration = "CompletionAction", tag = "5")]
    pub completion_action: i32,
    #[prost(message, repeated, tag = "10")]
    pub actions: Vec<Action>,
    #[prost(bool, tag = "12")]
    pub enabled: bool,
    #[prost(double, tag = "13")]
    pub completion_time: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
pub(super) enum CompletionTarget {
    None = 0,
    Next = 1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
pub(super) enum CompletionAction {
    First = 0,
    AfterTime = 3,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct Action {
    #[prost(double, tag = "4")]
    pub delay: f64,
    #[prost(bool, tag = "6")]
    pub enabled: bool,
    #[prost(enumeration = "ActionType", tag = "9")]
    pub kind: i32,
    #[prost(message, optional, tag = "20")]
    pub media: Option<MediaAction>,
    #[prost(message, optional, tag = "23")]
    pub slide: Option<SlideAction>,
    #[prost(message, optional, tag = "26")]
    pub clear: Option<ClearAction>,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct ClearAction {
    #[prost(enumeration = "ClearTarget", tag = "2")]
    pub target: i32,
    #[prost(int32, tag = "4")]
    pub destination: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
pub(super) enum ClearTarget {
    All = 0,
    Audio = 1,
    Background = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
pub(super) enum ActionType {
    Unknown = 0,
    StageLayout = 1,
    Media = 2,
    Timer = 3,
    Communication = 4,
    Clear = 5,
    Prop = 6,
    Mask = 7,
    Message = 8,
    SocialMedia = 9,
    Multiscreen = 10,
    PresentationSlide = 11,
    ForegroundMedia = 12,
    BackgroundMedia = 13,
    PresentationDocument = 14,
    PropSlide = 15,
    ExternalPresentation = 17,
    AudienceLook = 18,
    AudioInput = 19,
    AudioBinPlaylist = 20,
    MediaBinPlaylist = 21,
    SlideDestination = 22,
    Macro = 23,
    ClearGroup = 24,
}

impl ActionType {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::StageLayout => "stage layout",
            Self::Media => "media",
            Self::Timer => "timer",
            Self::Communication => "communication",
            Self::Clear => "clear",
            Self::Prop => "prop",
            Self::Mask => "mask",
            Self::Message => "message",
            Self::SocialMedia => "social media",
            Self::Multiscreen => "multiscreen",
            Self::PresentationSlide => "slide",
            Self::ForegroundMedia => "foreground media",
            Self::BackgroundMedia => "background media",
            Self::PresentationDocument => "presentation document",
            Self::PropSlide => "prop slide",
            Self::ExternalPresentation => "external presentation",
            Self::AudienceLook => "audience look",
            Self::AudioInput => "audio input",
            Self::AudioBinPlaylist => "audio playlist",
            Self::MediaBinPlaylist => "media playlist",
            Self::SlideDestination => "slide destination",
            Self::Macro => "macro",
            Self::ClearGroup => "clear group",
        }
    }
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct SlideAction {
    #[prost(message, optional, tag = "2")]
    pub presentation: Option<PresentationSlide>,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct PresentationSlide {
    #[prost(message, optional, tag = "1")]
    pub slide: Option<Slide>,
    #[prost(message, optional, tag = "2")]
    pub notes: Option<Notes>,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct Notes {
    #[prost(bytes, tag = "1")]
    pub rtf: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct Slide {
    #[prost(message, repeated, tag = "1")]
    pub elements: Vec<SlideElement>,
    #[prost(bool, tag = "4")]
    pub draws_background: bool,
    #[prost(message, optional, tag = "5")]
    pub background: Option<Color>,
    #[prost(message, optional, tag = "6")]
    pub size: Option<Size>,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct SlideElement {
    #[prost(message, optional, tag = "1")]
    pub element: Option<Element>,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct Element {
    #[prost(message, optional, tag = "3")]
    pub bounds: Option<Rect>,
    #[prost(double, tag = "4")]
    pub rotation: f64,
    #[prost(message, optional, tag = "8")]
    pub path: Option<Path>,
    #[prost(message, optional, tag = "9")]
    pub fill: Option<Fill>,
    #[prost(message, optional, tag = "13")]
    pub text: Option<Text>,
    #[prost(bool, tag = "16")]
    pub hidden: bool,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct Path {
    #[prost(message, optional, tag = "3")]
    pub shape: Option<Shape>,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct Shape {
    #[prost(enumeration = "ShapeType", tag = "1")]
    pub kind: i32,
    #[prost(message, optional, tag = "2")]
    pub rounded_rectangle: Option<RoundedRectangle>,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct RoundedRectangle {
    #[prost(double, tag = "1")]
    pub roundness: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
pub(super) enum ShapeType {
    Unknown = 0,
    Rectangle = 1,
    Ellipse = 2,
    IsoscelesTriangle = 3,
    RightTriangle = 4,
    Rhombus = 5,
    Star = 6,
    Polygon = 7,
    Custom = 8,
    RightArrow = 9,
    DoubleArrow = 10,
    RoundedRectangle = 11,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct Fill {
    #[prost(message, optional, tag = "1")]
    pub color: Option<Color>,
    #[prost(bool, tag = "4")]
    pub enabled: bool,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct Rect {
    #[prost(message, optional, tag = "1")]
    pub origin: Option<Point>,
    #[prost(message, optional, tag = "2")]
    pub size: Option<Size>,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct Point {
    #[prost(double, tag = "1")]
    pub x: f64,
    #[prost(double, tag = "2")]
    pub y: f64,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct Size {
    #[prost(double, tag = "1")]
    pub width: f64,
    #[prost(double, tag = "2")]
    pub height: f64,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct Text {
    #[prost(message, optional, tag = "3")]
    pub attributes: Option<Attributes>,
    #[prost(bytes, tag = "5")]
    pub rtf: Vec<u8>,
    #[prost(int32, tag = "6")]
    pub vertical: i32,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct Attributes {
    #[prost(message, optional, tag = "1")]
    pub font: Option<Font>,
    #[prost(message, optional, tag = "3")]
    pub color: Option<Color>,
    #[prost(message, optional, tag = "4")]
    pub underline: Option<Decoration>,
    #[prost(message, optional, tag = "6")]
    pub paragraph: Option<Paragraph>,
    #[prost(double, tag = "7")]
    pub kerning: f64,
    #[prost(message, optional, tag = "9")]
    pub strikethrough: Option<Decoration>,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct Font {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(double, tag = "2")]
    pub size: f64,
    #[prost(bool, tag = "4")]
    pub italic: bool,
    #[prost(bool, tag = "8")]
    pub bold: bool,
    #[prost(string, tag = "9")]
    pub family: String,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct Decoration {
    #[prost(int32, tag = "1")]
    pub style: i32,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct Paragraph {
    #[prost(int32, tag = "1")]
    pub alignment: i32,
    #[prost(double, tag = "5")]
    pub line_height: f64,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct MediaAction {
    #[prost(message, optional, tag = "5")]
    pub element: Option<Media>,
    #[prost(message, optional, tag = "7")]
    pub video: Option<Video>,
    #[prost(int32, tag = "10")]
    pub layer: i32,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct Media {
    #[prost(message, optional, tag = "2")]
    pub url: Option<Url>,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct Url {
    #[prost(string, tag = "1")]
    pub absolute: String,
    #[prost(string, tag = "2")]
    pub relative: String,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct Video {
    #[prost(int32, tag = "1")]
    pub playback: i32,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct Ccli {
    #[prost(string, tag = "1")]
    pub author: String,
    #[prost(string, tag = "3")]
    pub title: String,
    #[prost(string, tag = "4")]
    pub publisher: String,
    #[prost(uint32, tag = "5")]
    pub year: u32,
    #[prost(uint32, tag = "6")]
    pub number: u32,
}
