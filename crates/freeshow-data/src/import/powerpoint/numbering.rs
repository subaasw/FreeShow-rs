// SPDX-License-Identifier: GPL-3.0-only

use roxmltree::Node;

#[derive(Default)]
pub(super) struct Numbering<'a> {
    levels: [Option<Sequence<'a>>; 9],
    previous_level: usize,
}

struct Sequence<'a> {
    scheme: &'a str,
    start: u32,
    next: u32,
}

impl<'a> Numbering<'a> {
    pub(super) fn marker(
        &mut self,
        bullet: Option<Node<'a, '_>>,
        level: usize,
        has_text: bool,
    ) -> Option<String> {
        let numbered = bullet.filter(|node| node.has_tag_name("buAutoNum"));
        if numbered.is_some() {
            self.levels[level + 1..].fill_with(|| None);
        } else {
            // PowerPoint treats the first plain paragraph after a nested item as a break in that inner list.
            self.levels[level.max(self.previous_level)..].fill_with(|| None);
        }
        self.previous_level = level;
        let bullet = numbered.filter(|_| has_text)?;
        let scheme = bullet.attribute("type").unwrap_or("arabicPeriod");
        let start = bullet
            .attribute("startAt")
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|value| (1..=32_767).contains(value))
            .unwrap_or(1);
        let sequence = &mut self.levels[level];
        if sequence
            .as_ref()
            .is_none_or(|list| list.scheme != scheme || list.start != start)
        {
            *sequence = Some(Sequence {
                scheme,
                start,
                next: start,
            });
        }
        let sequence = sequence.as_mut().unwrap();
        let marker = format_marker(scheme, sequence.next);
        sequence.next = sequence.next.saturating_add(1);
        Some(marker)
    }
}

fn format_marker(scheme: &str, number: u32) -> String {
    let (kind, prefix, suffix) = if let Some(kind) = scheme.strip_suffix("ParenBoth") {
        (kind, "(", ")")
    } else if let Some(kind) = scheme.strip_suffix("ParenR") {
        (kind, "", ")")
    } else if let Some(kind) = scheme.strip_suffix("Plain") {
        (kind, "", "")
    } else {
        (scheme.strip_suffix("Period").unwrap_or(scheme), "", ".")
    };
    let value = match kind {
        "alphaLc" | "alphaUc" if number <= 32_767 => {
            let first = if kind == "alphaLc" { b'a' } else { b'A' };
            let index = number - 1;
            let letter = char::from(first + (index % 26) as u8);
            std::iter::repeat_n(letter, (index / 26 + 1) as usize).collect()
        }
        "romanLc" | "romanUc" if number <= 32_767 => {
            let roman = roman(number);
            if kind == "romanLc" {
                roman.to_ascii_lowercase()
            } else {
                roman
            }
        }
        _ => number.to_string(),
    };
    format!("{prefix}{value}{suffix}")
}

fn roman(mut number: u32) -> String {
    let mut result = String::new();
    for (value, digits) in [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ] {
        for _ in 0..(number / value) {
            result.push_str(digits);
        }
        number %= value;
    }
    result
}
