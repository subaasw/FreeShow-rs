// SPDX-License-Identifier: GPL-3.0-only

use roxmltree::Node;
use serde_json::{Value, json};

use super::{
    SlideSize,
    theme::{Context, child},
};

pub(super) struct TextBody {
    pub align: String,
    pub insets: [f64; 4],
    pub lines: Vec<Value>,
}

impl TextBody {
    pub(super) fn read(shape: Node<'_, '_>, context: &Context, size: &SlideSize) -> Option<Self> {
        let body = child(shape, "txBody")?;
        let properties = child(body, "bodyPr");
        let anchor = properties.and_then(|body| body.attribute("anchor"));
        let align = match anchor {
            Some("b") => "align-items:flex-end;",
            Some("ctr") => "align-items:center;",
            _ => "align-items:flex-start;",
        };
        let font_ref = child(shape, "style").and_then(|style| child(style, "fontRef"));
        let list = child(body, "lstStyle");
        let px_per_point = size.px_y(12_700.0);
        let mut lines = Vec::new();
        let mut has_text = false;
        for paragraph in body.children().filter(|node| node.has_tag_name("p")) {
            let para = child(paragraph, "pPr");
            let level = para
                .and_then(|para| para.attribute("lvl"))
                .and_then(|level| level.parse::<usize>().ok())
                .filter(|level| *level < 9)
                .unwrap_or(0);
            let level = list.and_then(|list| child(list, &format!("lvl{}pPr", level + 1)));
            let alignment = match [para, level]
                .into_iter()
                .flatten()
                .find_map(|props| props.attribute("algn"))
            {
                Some("ctr") => "center",
                Some("r") => "right",
                Some("just" | "justLow" | "dist" | "thaiDist") => "justify",
                _ => "left",
            };
            let defaults = [para, level].map(|node| node.and_then(|node| child(node, "defRPr")));
            let style = |run| run_style(run, defaults, font_ref, context, px_per_point);
            let mut runs = Vec::new();
            for run in paragraph.children().filter(Node::is_element) {
                let value = match run.tag_name().name() {
                    "r" | "fld" => child(run, "t").and_then(|text| text.text()),
                    "t" => run.text(),
                    "br" => {
                        lines.push(line(
                            alignment,
                            std::mem::take(&mut runs),
                            style(child(run, "rPr")),
                        ));
                        None
                    }
                    _ => None,
                };
                if let Some(value) = value.filter(|value| !value.is_empty()) {
                    has_text |= !value.trim().is_empty();
                    runs.push(json!({"value": value, "style": style(child(run, "rPr"))}));
                }
            }
            lines.push(line(alignment, runs, style(child(paragraph, "endParaRPr"))));
        }
        has_text.then(|| {
            let inset = |key, default| {
                properties
                    .and_then(|node| node.attribute(key))
                    .and_then(|value| value.parse::<i32>().ok())
                    .map_or(default, f64::from)
            };
            Self {
                align: align.into(),
                insets: [
                    size.px_x(inset("lIns", 91_440.0)),
                    size.px_y(inset("tIns", 45_720.0)),
                    size.px_x(inset("rIns", 91_440.0)),
                    size.px_y(inset("bIns", 45_720.0)),
                ],
                lines,
            }
        })
    }
}

fn line(alignment: &str, mut runs: Vec<Value>, blank_style: String) -> Value {
    if runs.is_empty() {
        // A space reserves the line height without painting a glyph.
        runs.push(json!({"value": " ", "style": blank_style}));
    }
    json!({"align": alignment, "text": runs})
}

fn run_style(
    run: Option<Node<'_, '_>>,
    defaults: [Option<Node<'_, '_>>; 2],
    font_ref: Option<Node<'_, '_>>,
    context: &Context,
    px_per_point: f64,
) -> String {
    let layers = [run, defaults[0], defaults[1]];
    let attribute = |key| {
        layers
            .into_iter()
            .flatten()
            .find_map(|node| node.attribute(key))
    };
    let property = |key| {
        layers
            .into_iter()
            .flatten()
            .find_map(|node| child(node, key))
    };
    let mut css = String::new();
    let typeface = property("latin")
        .and_then(|font| font.attribute("typeface"))
        .or_else(|| match font_ref.and_then(|font| font.attribute("idx")) {
            Some("major") => Some("+mj-lt"),
            Some("minor") => Some("+mn-lt"),
            _ => None,
        });
    if let Some(family) = typeface.and_then(|font| context.font_family(font)) {
        css.push_str(&format!("font-family:{family};"));
    }
    if let Some(size) = attribute("sz")
        .and_then(|size| size.parse::<u32>().ok())
        .filter(|size| (100..=400_000).contains(size))
    {
        let px = f64::from(size) * px_per_point / 100.0;
        css.push_str(&format!("font-size:{px}px;"));
    }
    if let Some(color) = property("solidFill")
        .or(font_ref)
        .and_then(|node| context.text_color(node))
    {
        css.push_str(&format!("color:{color};"));
    }
    if let Some(color) = property("highlight").and_then(|node| context.text_color(node)) {
        css.push_str(&format!("background-color:{color};"));
    }
    for (key, css_key, enabled) in [("b", "font-weight", "bold"), ("i", "font-style", "italic")] {
        let value = match attribute(key) {
            Some("1" | "true") => enabled,
            Some("0" | "false") => "normal",
            _ => continue,
        };
        css.push_str(&format!("{css_key}:{value};"));
    }
    let underline = attribute("u");
    let strike = attribute("strike");
    if underline.is_some() || strike.is_some() {
        let value = match (
            underline.is_some_and(|u| u != "none"),
            matches!(strike, Some("sngStrike" | "dblStrike")),
        ) {
            (true, true) => "underline line-through",
            (true, false) => "underline",
            (false, true) => "line-through",
            (false, false) => "none",
        };
        css.push_str(&format!("text-decoration:{value};"));
    }
    if let Some(spacing) = attribute("spc").and_then(|value| value.parse::<i32>().ok()) {
        let px = f64::from(spacing.clamp(-4000, 4000)) * px_per_point / 100.0;
        css.push_str(&format!("letter-spacing:{px}px;"));
    }
    css
}
