// SPDX-License-Identifier: GPL-3.0-only

use roxmltree::Node;
use serde_json::{Value, json};

use super::{
    SlideSize,
    numbering::Numbering,
    theme::{Context, child},
};

pub(super) struct TextBody {
    pub align: String,
    pub insets: [f64; 4],
    pub nowrap: bool,
    pub font_scale: f64,
    pub lines: Vec<Value>,
}

impl TextBody {
    pub(super) fn read(shape: Node<'_, '_>, context: &Context, size: &SlideSize) -> Option<Self> {
        let body = child(shape, "txBody")?;
        let properties = child(body, "bodyPr");
        let autofit = properties.and_then(|node| child(node, "normAutofit"));
        let autofit_percent = |key| {
            autofit
                .and_then(|node| node.attribute(key))
                .and_then(percentage)
        };
        let font_scale = autofit_percent("fontScale")
            .filter(|value| (1.0..=100.0).contains(value))
            .unwrap_or(100.0)
            / 100.0;
        let line_reduction = autofit_percent("lnSpcReduction")
            .filter(|value| *value <= 100.0)
            .unwrap_or(0.0);
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
        let mut numbering = Numbering::default();
        for paragraph in body.children().filter(|node| node.has_tag_name("p")) {
            let para = child(paragraph, "pPr");
            let level = para
                .and_then(|para| para.attribute("lvl"))
                .and_then(|level| level.parse::<usize>().ok())
                .filter(|level| *level < 9)
                .unwrap_or(0);
            let level_style = list.and_then(|list| child(list, &format!("lvl{}pPr", level + 1)));
            let properties = [
                para,
                level_style,
                list.and_then(|list| child(list, "defPPr")),
            ];
            let alignment = match properties
                .into_iter()
                .flatten()
                .find_map(|props| props.attribute("algn"))
            {
                Some("ctr") => "center",
                Some("r") => "right",
                Some("just" | "justLow" | "dist" | "thaiDist") => "justify",
                _ => "left",
            };
            let defaults = properties.map(|node| node.and_then(|node| child(node, "defRPr")));
            let [height, before, after] = ["lnSpc", "spcBef", "spcAft"].map(|key| {
                properties.into_iter().flatten().find_map(|node| {
                    spacing(
                        child(node, key)?,
                        px_per_point,
                        if key == "lnSpc" { line_reduction } else { 0.0 },
                    )
                })
            });
            let [left, right, indent] = ["marL", "marR", "indent"].map(|key| {
                let emu = properties
                    .into_iter()
                    .flatten()
                    .find_map(|node| node.attribute(key)?.parse::<i32>().ok())
                    .unwrap_or(0);
                size.px_x(f64::from(emu)).round() as i32
            });
            let first = lines.len();
            let style = |run| run_style(run, defaults, font_ref, context, px_per_point, font_scale);
            let mut runs = Vec::new();
            let mut paragraph_has_text = false;
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
                    paragraph_has_text |= !value.trim().is_empty();
                    runs.push(json!({"value": value, "style": style(child(run, "rPr"))}));
                }
            }
            lines.push(line(alignment, runs, style(child(paragraph, "endParaRPr"))));
            has_text |= paragraph_has_text;
            let marker = bullet_property(properties, &["buNone", "buChar", "buAutoNum", "buBlip"]);
            let number = numbering.marker(marker, level, paragraph_has_text);
            if paragraph_has_text {
                let character = marker
                    .filter(|node| node.has_tag_name("buChar"))
                    .and_then(|node| node.attribute("char"));
                if let Some(value) = number
                    .as_deref()
                    .or(character)
                    .filter(|value| !value.trim().is_empty())
                {
                    lines[first]["pptxBullet"] = bullet(value, properties, context, px_per_point);
                }
            }
            let last = lines.len() - 1;
            for (index, line) in lines.iter_mut().enumerate().skip(first) {
                let before = before.as_deref().filter(|_| index == first);
                let after = after.as_deref().filter(|_| index == last);
                if height.is_some() || before.is_some() || after.is_some() {
                    line["pptxSpacing"] = json!([height, before, after]);
                }
                let first_indent = if index == first { indent } else { 0 };
                if [left, right, first_indent] != [0; 3] {
                    line["pptxMargins"] = json!([left, right, first_indent]);
                }
            }
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
                nowrap: properties.and_then(|node| node.attribute("wrap")) == Some("none"),
                font_scale,
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

fn bullet_property<'a, 'input>(
    properties: [Option<Node<'a, 'input>>; 3],
    names: &[&str],
) -> Option<Node<'a, 'input>> {
    properties.into_iter().flatten().find_map(|node| {
        node.children()
            .filter(Node::is_element)
            .find(|child| names.contains(&child.tag_name().name()))
    })
}

fn bullet(
    value: &str,
    properties: [Option<Node<'_, '_>>; 3],
    context: &Context,
    px_per_point: f64,
) -> Value {
    let property = |names: &[&str]| bullet_property(properties, names);
    let font = property(&["buFont", "buFontTx"])
        .and_then(|node| node.attribute("typeface"))
        .and_then(|name| context.font_family(name));
    let (value, font) = match (value, font) {
        ("\u{f0b7}", Some(font)) if font.eq_ignore_ascii_case("Symbol") => ("•", None),
        _ => (value, font),
    };
    let mut style = String::new();
    if let Some(font) = font {
        style.push_str(&format!("font-family:{font};"));
    }
    if let Some(color) = property(&["buClr", "buClrTx"]).and_then(|node| context.text_color(node)) {
        style.push_str(&format!("color:{color};"));
    }
    if let Some(node) = property(&["buSzPts", "buSzPct", "buSzTx"])
        && let Some(value) = node.attribute("val")
    {
        match node.tag_name().name() {
            "buSzPts" => {
                if let Ok(value) = value.parse::<u32>()
                    && (100..=400_000).contains(&value)
                {
                    let px = f64::from(value) * px_per_point / 100.0;
                    style.push_str(&format!("font-size:{px}px;"));
                }
            }
            "buSzPct" => {
                if let Some(value) =
                    percentage(value).filter(|value| (25.0..=400.0).contains(value))
                {
                    style.push_str(&format!("--pptx-bullet-scale:{value}%;"));
                }
            }
            _ => {}
        }
    }
    json!({"value": value, "style": style})
}

fn spacing(node: Node<'_, '_>, px_per_point: f64, reduction: f64) -> Option<String> {
    let value = node.first_element_child()?;
    let number = value.attribute("val")?;
    match value.tag_name().name() {
        "spcPts" => Some(format!(
            "{}px",
            f64::from(number.parse::<u32>().ok()?) * px_per_point / 100.0
        )),
        "spcPct" => percentage(number).map(|value| format!("{}%", (value - reduction).max(0.0))),
        _ => None,
    }
}

fn percentage(number: &str) -> Option<f64> {
    let percent = match number.strip_suffix('%') {
        Some(number) => number.parse::<f64>().ok()?,
        None => f64::from(number.parse::<u32>().ok()?) / 1000.0,
    };
    (percent.is_finite() && percent >= 0.0).then_some(percent)
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
    defaults: [Option<Node<'_, '_>>; 3],
    font_ref: Option<Node<'_, '_>>,
    context: &Context,
    px_per_point: f64,
    font_scale: f64,
) -> String {
    let layers = [run, defaults[0], defaults[1], defaults[2]];
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
        let px = f64::from(size) * px_per_point / 100.0 * font_scale;
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
    if let Some(baseline) = attribute("baseline")
        .and_then(super::theme::percentage)
        .filter(|value| (-1.0..=1.0).contains(value) && *value != 0.0)
    {
        css.push_str(&format!("vertical-align:{baseline}em;"));
    }
    css
}
