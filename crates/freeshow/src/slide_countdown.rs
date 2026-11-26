// SPDX-License-Identifier: GPL-3.0-only

use serde_json::Value;
use std::borrow::Cow;
use std::time::{Duration, Instant};

pub(crate) struct SlideCountdown {
    started: Instant,
    longest: u64,
    last_elapsed: u64,
    last_change: Option<Instant>,
}

impl SlideCountdown {
    pub fn on_take(slide: &Value, now: Instant) -> Option<Self> {
        let longest = slide["items"]
            .as_array()?
            .iter()
            .filter_map(duration)
            .max()?;
        Some(Self {
            started: now,
            longest,
            last_elapsed: 0,
            last_change: Some(now),
        })
    }

    pub fn update(slide: &Value, previous: Option<&Self>, now: Instant) -> Option<Self> {
        let mut updated =
            Self::on_take(slide, previous.map_or(now, |countdown| countdown.started))?;
        updated.last_elapsed = updated.elapsed(now);
        updated.last_change = Some(now);
        Some(updated)
    }

    fn elapsed(&self, now: Instant) -> u64 {
        now.saturating_duration_since(self.started)
            .as_secs()
            .min(self.longest)
    }

    pub fn tick(&mut self, now: Instant, output_delay_ms: u32) -> bool {
        let elapsed = self.elapsed(now);
        if self.last_elapsed != elapsed {
            self.last_elapsed = elapsed;
            self.last_change = Some(now);
            return true;
        }
        let delay = Duration::from_millis(output_delay_ms.into());
        if let Some(change) = self.last_change.take()
            && !delay.is_zero()
        {
            if now.saturating_duration_since(change) < delay {
                self.last_change = Some(change);
            }
            return true;
        }
        false
    }

    /// Substitute the display text in a render copy, preserving the saved text and style.
    pub fn render<'a>(&self, mut slide: Cow<'a, Value>, now: Instant) -> Cow<'a, Value> {
        let elapsed = self.elapsed(now);
        if elapsed == 0 {
            return slide;
        }
        if let Some(items) = slide.to_mut()["items"].as_array_mut() {
            for item in items {
                if let Some(total) = duration(item)
                    && let Some(runs) = item
                        .pointer_mut("/lines/0/text")
                        .and_then(Value::as_array_mut)
                {
                    let remaining = total.saturating_sub(elapsed);
                    let text = if total < 3600 {
                        format!("{:02}:{:02}", remaining / 60, remaining % 60)
                    } else {
                        format!(
                            "{:02}:{:02}:{:02}",
                            remaining / 3600,
                            remaining / 60 % 60,
                            remaining % 60
                        )
                    };
                    if let Some((first, rest)) = runs.split_first_mut() {
                        first["value"] = text.into();
                        for run in rest {
                            run["value"] = "".into();
                        }
                    }
                }
            }
        }
        slide
    }
}

pub(crate) fn is_countdown_item(item: &Value) -> bool {
    item["freeshowRustCountdown"] == true
}

fn duration(item: &Value) -> Option<u64> {
    if !is_countdown_item(item)
        || freeshow_data::item_hidden(item)
        || item["lines"].as_array()?.len() != 1
    {
        return None;
    }
    let runs = item.pointer("/lines/0/text")?.as_array()?;
    let mut raw = String::new();
    for run in runs {
        raw.push_str(run["value"].as_str()?);
    }
    let mut parts = raw.trim().split(':');
    let parse = |part: &str| {
        if part.len() <= 2 && part.bytes().all(|byte| byte.is_ascii_digit()) {
            part.parse::<u64>().ok()
        } else {
            None
        }
    };
    let major = parse(parts.next()?)?;
    let minute_or_second = parse(parts.next()?)?;
    let seconds = if let Some(part) = parts.next() {
        if parts.next().is_some() || minute_or_second >= 60 {
            return None;
        }
        let second = parse(part)?;
        (second < 60).then_some(major * 3600 + minute_or_second * 60 + second)?
    } else {
        (minute_or_second < 60).then_some(major * 60 + minute_or_second)?
    };
    (seconds > 0).then_some(seconds)
}
