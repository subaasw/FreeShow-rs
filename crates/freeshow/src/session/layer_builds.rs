// SPDX-License-Identifier: GPL-3.0-only

use serde_json::Value;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use super::Session;

pub(crate) const BUILD_KEY: &str = "freeshowRustBuild";

#[derive(Clone, Copy)]
enum Effect {
    Fade,
    Scale,
    Left,
    Right,
    Up,
    Down,
}

impl Effect {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "fade" => Self::Fade,
            "scale" => Self::Scale,
            "left" => Self::Left,
            "right" => Self::Right,
            "up" => Self::Up,
            "down" => Self::Down,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Self::Fade => "fade",
            Self::Scale => "scale",
            Self::Left => "left",
            Self::Right => "right",
            Self::Up => "up",
            Self::Down => "down",
        }
    }
}

struct Phase {
    effect: Effect,
    delay_ms: u32,
    duration_ms: u32,
}

impl Phase {
    fn read(value: &Value) -> Option<Self> {
        let effect = Effect::parse(value["effect"].as_str()?)?;
        let delay_ms = value.get("delayMs").map_or(Some(0), Value::as_u64)?;
        let duration_ms = value["durationMs"].as_u64()?;
        (delay_ms <= 60_000 && duration_ms <= 10_000).then_some(Self {
            effect,
            delay_ms: delay_ms as u32,
            duration_ms: duration_ms as u32,
        })
    }

    fn end(&self) -> u32 {
        self.delay_ms + self.duration_ms
    }

    fn progress(&self, elapsed: u32) -> f32 {
        if elapsed < self.delay_ms {
            return 0.0;
        }
        if elapsed >= self.end() {
            return 1.0;
        }
        let fraction = (elapsed - self.delay_ms) as f32 / self.duration_ms as f32;
        freeshow_output::ease_progress("sine", fraction)
    }

    fn visual(&self, visible: f32) -> BuildVisual {
        let offset = (1.0 - visible) * 0.08;
        let [x, y] = match self.effect {
            Effect::Fade | Effect::Scale => [0.0, 0.0],
            Effect::Left => [-offset, 0.0],
            Effect::Right => [offset, 0.0],
            Effect::Up => [0.0, -offset],
            Effect::Down => [0.0, offset],
        };
        BuildVisual {
            opacity: visible,
            x,
            y,
            scale: if matches!(self.effect, Effect::Scale) {
                0.55 + visible * 0.45
            } else {
                1.0
            },
        }
    }
}

struct LayerBuild {
    step: u16,
    after_previous: bool,
    enter: Phase,
    exit: Option<Phase>,
}

impl LayerBuild {
    fn supported(item: &Value) -> bool {
        item.is_object()
            && item["clickReveal"].as_bool() != Some(true)
            && item["lineReveal"].as_bool() != Some(true)
    }

    fn read(item: &Value) -> Option<Self> {
        if !Self::supported(item) {
            return None;
        }
        let value = item.get(BUILD_KEY)?;
        Some(Self {
            step: u16::try_from(value["step"].as_u64()?).ok()?,
            after_previous: value["afterPrevious"] == true,
            enter: Phase::read(&value["enter"])?,
            exit: value.get("exit").and_then(Phase::read),
        })
    }

    fn visual(&self, elapsed: u32) -> BuildVisual {
        if elapsed < self.enter.end() {
            return self.enter.visual(self.enter.progress(elapsed));
        }
        if let Some(exit) = &self.exit {
            let since_enter = elapsed - self.enter.end();
            return exit.visual(1.0 - exit.progress(since_enter));
        }
        BuildVisual::SHOWN
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct BuildVisual {
    pub opacity: f32,
    pub x: f32,
    pub y: f32,
    pub scale: f32,
}

impl BuildVisual {
    const SHOWN: Self = Self {
        opacity: 1.0,
        x: 0.0,
        y: 0.0,
        scale: 1.0,
    };
    const HIDDEN: Self = Self {
        opacity: 0.0,
        x: 0.0,
        y: 0.0,
        scale: 1.0,
    };
}

#[derive(Clone, Copy)]
enum Start {
    At(Instant),
    Finished,
}

impl Start {
    fn after(self, millis: u32) -> Self {
        match self {
            Self::At(time) => Self::At(time + Duration::from_millis(millis.into())),
            Self::Finished => Self::Finished,
        }
    }
}

struct BuildStep {
    number: u16,
    after_previous: bool,
    entrance_ms: u32,
    total_ms: u32,
    start: Option<Start>,
}

impl BuildStep {
    fn manual(&self) -> bool {
        self.number > 0 && !self.after_previous
    }

    fn trigger_label(&self) -> &'static str {
        match (self.number, self.after_previous) {
            (0, _) => "On Take",
            (_, true) => "After Previous",
            _ => "On Click",
        }
    }
}

pub(crate) struct LayerBuilds {
    layers: Vec<Option<LayerBuild>>,
    steps: Vec<BuildStep>,
    take_start: Start,
    last_tick: Instant,
    last_change: Option<Instant>,
}

impl LayerBuilds {
    pub fn on_take(slide: &Value, now: Instant) -> Option<Self> {
        let layers: Vec<_> = slide["items"]
            .as_array()?
            .iter()
            .map(|item| {
                (!freeshow_data::item_hidden(item))
                    .then(|| LayerBuild::read(item))
                    .flatten()
            })
            .collect();
        let mut steps = BTreeMap::new();
        for layer in layers.iter().flatten() {
            let step = steps.entry(layer.step).or_insert(BuildStep {
                number: layer.step,
                after_previous: false,
                entrance_ms: 0,
                total_ms: 0,
                start: None,
            });
            step.after_previous |= layer.step > 0 && layer.after_previous;
            step.entrance_ms = step.entrance_ms.max(layer.enter.end());
            step.total_ms = step
                .total_ms
                .max(layer.enter.end() + layer.exit.as_ref().map_or(0, Phase::end));
        }
        if steps.is_empty() {
            return None;
        }
        let mut builds = Self {
            layers,
            steps: steps.into_values().collect(),
            take_start: Start::At(now),
            last_tick: now,
            last_change: Some(now),
        };
        builds.schedule_automatic();
        Some(builds)
    }

    fn schedule_automatic(&mut self) {
        let mut entrance_finished = Some(self.take_start);
        for step in &mut self.steps {
            if !step.manual() {
                step.start = entrance_finished;
            }
            entrance_finished = step.start.map(|start| start.after(step.entrance_ms));
        }
    }

    pub fn next(&mut self, now: Instant) -> bool {
        let Some(step) = self
            .steps
            .iter_mut()
            .find(|step| step.manual() && step.start.is_none())
        else {
            return false;
        };
        step.start = Some(Start::At(now));
        self.schedule_automatic();
        self.last_change = Some(now);
        true
    }

    pub fn restart(&mut self, now: Instant) {
        self.take_start = Start::At(now);
        for step in &mut self.steps {
            step.start = None;
        }
        self.schedule_automatic();
        self.last_tick = now;
        self.last_change = Some(now);
    }

    pub fn previous(&mut self, now: Instant) -> bool {
        let Some(step) = self
            .steps
            .iter_mut()
            .rev()
            .find(|step| step.manual() && step.start.is_some())
        else {
            return false;
        };
        step.start = None;
        self.schedule_automatic();
        self.last_change = Some(now);
        true
    }

    pub fn finish(&mut self) {
        self.take_start = Start::Finished;
        for step in &mut self.steps {
            step.start = Some(Start::Finished);
        }
    }

    pub fn waiting_for_next(&self) -> bool {
        self.next_step().is_some()
    }

    pub fn next_step(&self) -> Option<u16> {
        self.steps
            .iter()
            .find(|step| step.manual() && step.start.is_none())
            .map(|step| step.number)
    }

    pub fn updated(&self, slide: &Value, now: Instant) -> Option<Self> {
        let mut next = Self::on_take(slide, now)?;
        next.take_start = self.take_start;
        if next.steps[0].number == 0 && self.steps[0].number != 0 {
            next.take_start = Start::Finished;
        }
        for step in next.steps.iter_mut().filter(|step| step.manual()) {
            step.start = self
                .steps
                .binary_search_by_key(&step.number, |step| step.number)
                .ok()
                .and_then(|index| self.steps[index].start)
                .filter(|start| !matches!(start, Start::At(time) if *time > now));
        }
        next.schedule_automatic();
        Some(next)
    }

    pub fn visual(&self, index: usize, now: Instant) -> BuildVisual {
        let Some(Some(layer)) = self.layers.get(index) else {
            return BuildVisual::SHOWN;
        };
        let step = self
            .steps
            .binary_search_by_key(&layer.step, |step| step.number)
            .expect("every layer has a build step");
        match self.steps[step].start {
            None => BuildVisual::HIDDEN,
            Some(Start::At(start)) if now < start => BuildVisual::HIDDEN,
            Some(Start::At(start)) => layer.visual(
                now.saturating_duration_since(start)
                    .as_millis()
                    .min(u128::from(u32::MAX)) as u32,
            ),
            Some(Start::Finished) => layer.visual(u32::MAX),
        }
    }

    pub fn sample(
        &self,
        index: usize,
        now: Instant,
        source_width: f32,
        source_height: f32,
    ) -> Option<freeshow_output::ItemSample> {
        let visual = self.visual(index, now);
        (visual.opacity > 0.0).then_some(freeshow_output::ItemSample {
            opacity: visual.opacity,
            x: visual.x * source_width,
            y: visual.y * source_height,
            scale: visual.scale,
        })
    }

    pub fn can_rewind(&self) -> bool {
        self.steps
            .iter()
            .any(|step| step.manual() && step.start.is_some())
    }

    pub fn automatic_entrance_remaining(&self, now: Instant) -> Duration {
        self.steps
            .iter()
            .filter(|step| step.after_previous)
            .filter_map(|step| {
                let Start::At(end) = step.start?.after(step.entrance_ms) else {
                    return None;
                };
                Some(end.saturating_duration_since(now))
            })
            .max()
            .unwrap_or_default()
    }

    /// Includes delayed changes and excludes steps waiting for a click.
    pub fn motion_remaining(&self, now: Instant) -> bool {
        self.steps.iter().any(|step| {
            let Some(Start::At(end)) = step.start.map(|start| start.after(step.total_ms)) else {
                return false;
            };
            now < end
        })
    }

    /// A frame is needed only while a value changes, including its final sample.
    pub fn tick(&mut self, now: Instant, output_delay_ms: u32) -> bool {
        let previous = std::mem::replace(&mut self.last_tick, now);
        let motion = self.layers.iter().enumerate().any(|(index, layer)| {
            layer.is_some() && self.visual(index, previous) != self.visual(index, now)
        });
        if motion {
            self.last_change = Some(now);
            return true;
        }
        let delay = Duration::from_millis(output_delay_ms.into());
        if let Some(change) = self.last_change {
            if now.saturating_duration_since(change) >= delay {
                self.last_change = None;
            }
            return !delay.is_zero();
        }
        false
    }
}

pub(crate) struct BuildPhaseForm {
    pub effect: &'static str,
    pub duration_ms: i32,
    pub delay_ms: i32,
}

impl From<Option<&Phase>> for BuildPhaseForm {
    fn from(phase: Option<&Phase>) -> Self {
        Self {
            effect: phase.map_or("none", |phase| phase.effect.name()),
            duration_ms: phase.map_or(400, |phase| phase.duration_ms as i32),
            delay_ms: phase.map_or(0, |phase| phase.delay_ms as i32),
        }
    }
}

pub(crate) struct LayerBuildForm {
    pub allowed: bool,
    pub preview_available: bool,
    pub step: i32,
    pub after_previous: bool,
    pub enter: BuildPhaseForm,
    pub exit: BuildPhaseForm,
}

pub(crate) struct BuildOrderRow {
    pub layer: usize,
    pub step: u16,
    pub name: String,
    pub trigger: &'static str,
}

impl Session {
    pub(crate) fn build_order(&self) -> Vec<BuildOrderRow> {
        let Some((show, slide_id)) = self.selected_layer_slide() else {
            return Vec::new();
        };
        let slide = &show.show["slides"][&slide_id];
        if slide["settings"]["actionOnly"] == true {
            return Vec::new();
        }
        let Some(builds) = LayerBuilds::on_take(slide, Instant::now()) else {
            return Vec::new();
        };
        let mut rows = builds
            .layers
            .iter()
            .enumerate()
            .rev()
            .filter_map(|(layer, build)| {
                let build = build.as_ref()?;
                let index = builds
                    .steps
                    .binary_search_by_key(&build.step, |step| step.number)
                    .ok()?;
                Some(BuildOrderRow {
                    layer,
                    step: build.step,
                    name: super::slide_layers::layer_name(
                        show,
                        &slide_id,
                        layer,
                        &slide["items"][layer],
                    ),
                    trigger: builds.steps[index].trigger_label(),
                })
            })
            .collect::<Vec<_>>();
        rows.sort_by_key(|row| row.step);
        rows
    }

    pub(crate) fn live_build_step(&self) -> i32 {
        if self.blanked || self.slide_hidden {
            return 0;
        }
        self.layer_builds
            .as_ref()
            .and_then(LayerBuilds::next_step)
            .map_or(0, i32::from)
    }

    pub(crate) fn selected_layer_build(&self) -> LayerBuildForm {
        let slide = self.current_show().and_then(|show| {
            let ordered = show.ordered_slides();
            let slide = &ordered.get(self.slide_index?)?.id;
            show.show["slides"].get(slide)
        });
        let items = slide.and_then(|slide| slide["items"].as_array());
        let item = items.and_then(|items| items.get(self.item_position().0));
        let build = item.and_then(LayerBuild::read);
        let after_previous = build.as_ref().is_some_and(|build| {
            build.step > 0
                && items.is_some_and(|items| {
                    items
                        .iter()
                        .filter(|item| !freeshow_data::item_hidden(item))
                        .filter_map(LayerBuild::read)
                        .any(|other| other.step == build.step && other.after_previous)
                })
        });
        LayerBuildForm {
            allowed: item.is_some_and(LayerBuild::supported),
            preview_available: slide.is_some_and(|slide| {
                slide["settings"]["actionOnly"].as_bool() != Some(true)
                    && items.is_some_and(|items| {
                        items.iter().any(|item| {
                            !freeshow_data::item_hidden(item) && LayerBuild::read(item).is_some()
                        })
                    })
            }),
            step: build.as_ref().map_or(0, |build| i32::from(build.step)),
            after_previous,
            enter: build.as_ref().map(|build| &build.enter).into(),
            exit: build.as_ref().and_then(|build| build.exit.as_ref()).into(),
        }
    }

    pub(super) fn set_layer_build_field(&mut self, key: &str, value: &str) -> Result<bool, String> {
        use serde_json::json;

        let number = |max| {
            value
                .parse::<u32>()
                .ok()
                .filter(|number| *number <= max)
                .ok_or_else(|| format!("Enter a whole number from 0 to {max}"))
        };
        let edit = match key {
            "build-after-previous" if matches!(value, "true" | "false") => {
                return self.set_build_trigger(value == "true");
            }
            "build-step" => ("", "step", json!(number(u32::from(u16::MAX))?)),
            "build-enter" | "build-exit" if value == "none" || Effect::parse(value).is_some() => {
                (&key["build-".len()..], "effect", json!(value))
            }
            "build-enter-duration" => ("enter", "durationMs", json!(number(10_000)?)),
            "build-exit-duration" => ("exit", "durationMs", json!(number(10_000)?)),
            "build-enter-delay" => ("enter", "delayMs", json!(number(60_000)?)),
            "build-exit-delay" => ("exit", "delayMs", json!(number(60_000)?)),
            _ => return Err("Choose a valid animation setting".into()),
        };
        if self.tab != "shows" {
            return Ok(false);
        }
        Ok(
            self.edit_selected_slide("Edit layer animation", |show, slide, index| {
                if show.layer_locked(slide, index) {
                    return false;
                }
                let Some(items) = show.show["slides"][slide]["items"].as_array_mut() else {
                    return false;
                };
                let (phase, field, value) = edit;
                let inherited_trigger =
                    value
                        .as_u64()
                        .filter(|_| field == "step")
                        .and_then(|number| {
                            let mut peers = items
                                .iter()
                                .enumerate()
                                .filter(|(peer, _)| *peer != index)
                                .filter_map(|(_, item)| LayerBuild::read(item))
                                .filter(|build| u64::from(build.step) == number)
                                .peekable();
                            if number == 0 {
                                Some(false)
                            } else if peers.peek().is_some() {
                                Some(peers.any(|build| build.after_previous))
                            } else {
                                None
                            }
                        });
                let Some(item) = items.get_mut(index) else {
                    return false;
                };
                if !LayerBuild::supported(item) {
                    return false;
                }
                if value == "none" {
                    if phase == "enter" {
                        item.as_object_mut().unwrap().remove(BUILD_KEY);
                    } else if let Some(build) =
                        item.get_mut(BUILD_KEY).and_then(Value::as_object_mut)
                    {
                        build.remove("exit");
                    }
                    return true;
                }
                let has_build = LayerBuild::read(item).is_some();
                if !has_build {
                    if key != "build-enter" {
                        return false;
                    }
                    item[BUILD_KEY] = json!({"step":0,"enter":{"effect":value,"durationMs":400}});
                } else if phase.is_empty() {
                    item[BUILD_KEY][field] = value;
                    if let Some(after_previous) = inherited_trigger {
                        write_trigger(item, after_previous);
                    }
                } else if Phase::read(&item[BUILD_KEY][phase]).is_some() {
                    item[BUILD_KEY][phase][field] = value;
                } else if field == "effect" {
                    item[BUILD_KEY][phase] = json!({"effect":value,"durationMs":400});
                } else {
                    return false;
                }
                true
            }),
        )
    }

    fn set_build_trigger(&mut self, after_previous: bool) -> Result<bool, String> {
        if self.tab != "shows" {
            return Ok(false);
        }
        Ok(
            self.edit_selected_slide("Set animation step trigger", |show, slide, index| {
                if show.layer_locked(slide, index) {
                    return false;
                }
                let Some(items) = show.show["slides"][slide]["items"].as_array() else {
                    return false;
                };
                let Some(build) = items
                    .get(index)
                    .and_then(LayerBuild::read)
                    .filter(|build| build.step > 0)
                else {
                    return false;
                };
                let changed = items
                    .iter()
                    .enumerate()
                    .filter_map(|(index, item)| {
                        let peer = LayerBuild::read(item)?;
                        (peer.step == build.step && peer.after_previous != after_previous)
                            .then_some(index)
                    })
                    .collect::<Vec<_>>();
                if changed.is_empty()
                    || changed.iter().any(|&index| show.layer_locked(slide, index))
                {
                    return false;
                }
                for index in changed {
                    write_trigger(
                        &mut show.show["slides"][slide]["items"][index],
                        after_previous,
                    );
                }
                true
            }),
        )
    }
}

fn write_trigger(item: &mut Value, after_previous: bool) {
    if after_previous {
        item[BUILD_KEY]["afterPrevious"] = true.into();
    } else if let Some(build) = item[BUILD_KEY].as_object_mut() {
        build.remove("afterPrevious");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parallel_take_manual_step_and_automatic_exit_share_a_seekable_clock() {
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        let slide = json!({"items":[
            {BUILD_KEY: {"step":0,"enter":{"effect":"fade","durationMs":100}}},
            {BUILD_KEY: {"step":0,"enter":{"effect":"right","delayMs":50,"durationMs":100}}},
            {BUILD_KEY: {"step":3,"enter":{"effect":"up","durationMs":100},
                "exit":{"effect":"fade","delayMs":200,"durationMs":50}}},
            {"clickReveal":true, BUILD_KEY: {"step":7,"enter":{"effect":"fade","durationMs":50}}}
        ]});
        let mut build = LayerBuilds::on_take(&slide, start).unwrap();
        assert!(build.waiting_for_next());
        assert!(build.visual(0, at(50)).opacity > 0.49);
        assert_eq!(build.visual(1, at(50)).opacity, 0.0);
        assert!(build.visual(1, at(75)).x > 0.0);
        assert!(build.tick(at(100), 0));
        assert!(build.tick(at(150), 0));
        assert!(!build.tick(at(175), 0));
        assert_eq!(build.visual(2, at(200)).opacity, 0.0);
        assert!(build.next(at(200)));
        assert!(!build.waiting_for_next());
        assert!(!build.next(at(201)));
        assert!(build.visual(2, at(250)).y < 0.0);
        assert_eq!(build.visual(2, at(499)).opacity, 1.0);
        assert!(build.visual(2, at(525)).opacity < 0.51);
        assert!(build.tick(at(550), 80));
        assert_eq!(build.visual(2, at(550)).opacity, 0.0);
        // Rewind affects the manual step only and leaves the on-Take layers in place.
        assert!(build.previous(at(560)));
        assert_eq!(build.visual(0, at(570)).opacity, 1.0);
        assert_eq!(build.visual(2, at(570)).opacity, 0.0);
        assert!(build.tick(at(640), 80));
        assert!(!build.tick(at(641), 80));
        assert!(build.next(at(700)));
        assert_eq!(build.visual(2, at(800)).opacity, 1.0);
    }

    #[test]
    fn automatic_steps_follow_the_longest_entrance_and_rewind_with_their_click() {
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        let slide = json!({"items":[
            {BUILD_KEY: {"step":0,"enter":{"effect":"fade","durationMs":50}}},
            {BUILD_KEY: {"step":1,"afterPrevious":true,"enter":{"effect":"fade","durationMs":50}}},
            {BUILD_KEY: {"step":3,"enter":{"effect":"fade","durationMs":75}}},
            {BUILD_KEY: {"step":3,"enter":{"effect":"fade","delayMs":50,"durationMs":100},
                "exit":{"effect":"fade","delayMs":500,"durationMs":0}}},
            {BUILD_KEY: {"step":6,"afterPrevious":true,"enter":{"effect":"scale","durationMs":50}}},
            {BUILD_KEY: {"step":6,"enter":{"effect":"fade","durationMs":50}}},
            {BUILD_KEY: {"step":8,"afterPrevious":true,"enter":{"effect":"fade","durationMs":0}}}
        ]});
        let mut builds = LayerBuilds::on_take(&slide, start).unwrap();
        assert_eq!(builds.next_step(), Some(3));
        assert_eq!(builds.visual(1, at(49)).opacity, 0.0);
        assert_eq!(builds.visual(1, at(100)).opacity, 1.0);
        assert_eq!(builds.visual(4, at(199)).opacity, 0.0);
        assert!(builds.next(at(200)));
        assert_eq!(builds.next_step(), None);
        assert_eq!(builds.visual(4, at(349)).opacity, 0.0);
        assert!(builds.visual(4, at(375)).opacity > 0.0);
        assert_eq!(
            builds.visual(4, at(375)).opacity,
            builds.visual(5, at(375)).opacity
        );
        assert_eq!(
            builds.automatic_entrance_remaining(at(375)),
            Duration::from_millis(25)
        );
        assert_eq!(builds.visual(6, at(399)).opacity, 0.0);
        assert_eq!(builds.visual(6, at(400)).opacity, 1.0);
        assert!(builds.previous(at(410)));
        assert_eq!(builds.next_step(), Some(3));
        assert_eq!(builds.visual(1, at(410)).opacity, 1.0);
        assert_eq!(builds.visual(6, at(850)).opacity, 0.0);
        builds.restart(at(900));
        assert_eq!(builds.visual(1, at(949)).opacity, 0.0);
        assert_eq!(builds.visual(1, at(1000)).opacity, 1.0);
    }

    #[test]
    fn a_timer_cannot_consume_a_manual_build() {
        use super::super::Session;
        use freeshow_data::ShowFile;

        let mut show = ShowFile::blank("build-timer", "Timer");
        show.add_slide().unwrap();
        show.show["layouts"]["main"]["slides"][0]["nextTimer"] = json!(1);
        show.show["layouts"]["main"]["slides"][0]["nextTimerTarget"] = json!("next");
        show.show["slides"]["s1"]["items"][0][BUILD_KEY] =
            json!({"step":1,"enter":{"effect":"fade","durationMs":0}});
        let mut session = Session::empty();
        session.shows.push(show);
        session.show_index = Some(0);
        session.select_slide(0);
        let due = session.pending_cue_completion.as_ref().unwrap().due;
        assert_eq!(session.next_timer_seconds(), None);
        assert!(!session.run_due_cue_completion(due));
        assert_eq!(session.slide_index, Some(0));
        assert!(session.advance_show());
        assert_eq!(session.slide_index, Some(0));
        assert_eq!(session.next_timer_seconds(), Some(1.0));
        assert!(session.run_due_cue_completion(due));
        assert_eq!(session.slide_index, Some(1));
        assert!(session.layer_builds.is_none());
    }

    #[test]
    fn timed_cue_waits_for_automatic_entrances() {
        let mut show = freeshow_data::ShowFile::blank("build-auto", "Timer");
        show.add_slide().unwrap();
        show.show["layouts"]["main"]["slides"][0]["nextTimer"] = json!(1);
        show.show["layouts"]["main"]["slides"][0]["nextTimerTarget"] = json!("next");
        show.show["slides"]["s1"]["items"][0][BUILD_KEY] =
            json!({"step":2,"afterPrevious":true,"enter":{"effect":"fade","durationMs":2000}});
        let mut session = Session::empty();
        session.shows.push(show);
        session.show_index = Some(0);
        session.select_slide(0);
        let due = session.pending_cue_completion.as_ref().unwrap().due;
        assert_eq!(session.live_build_step(), 0);
        assert!(!session.run_due_cue_completion(due));
        assert_eq!(session.slide_index, Some(0));
        assert!(session.run_due_cue_completion(due + Duration::from_secs(2)));
        assert_eq!(session.slide_index, Some(1));
    }

    #[test]
    fn authoring_a_build_persists_phase_settings_and_undo() {
        let root =
            std::env::temp_dir().join(format!("freeshow-build-author-{}", freeshow_data::new_id()));
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        assert!(session.inspector_set("build-step", "5").is_err());
        session.inspector_set("build-enter", "scale").unwrap();
        session.inspector_set("build-step", "5").unwrap();
        session
            .inspector_set("build-enter-duration", "650")
            .unwrap();
        session.inspector_set("build-exit", "right").unwrap();
        session.inspector_set("build-exit-delay", "1200").unwrap();
        let stored = session.current_show().unwrap();
        let reread = freeshow_data::ShowFile::load(stored.path.as_ref().unwrap()).unwrap();
        assert_eq!(
            reread.show["slides"]["s1"]["items"][0][BUILD_KEY],
            json!({
                "step":5,"enter":{"effect":"scale","durationMs":650},
                "exit":{"effect":"right","durationMs":400,"delayMs":1200}
            })
        );
        assert!(
            session
                .inspector_set("build-enter-duration", "11000")
                .is_err()
        );
        session.inspector_set("build-enter", "none").unwrap();
        assert_eq!(session.selected_layer_build().enter.effect, "none");
        assert!(session.undo());
        assert_eq!(session.selected_layer_build().exit.delay_ms, 1200);
        assert!(session.set_layer_flags(true, true));
        assert!(session.inspector_set("build-exit", "none").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn changing_a_step_trigger_edits_the_whole_group_in_one_undo() {
        let root = tempfile::tempdir().unwrap();
        let mut session = Session::open(root.path().to_path_buf());
        assert!(session.create_show());
        assert!(session.edit_selected_slide("Group two animations", |show, slide, _| {
            show.show["slides"][slide]["items"] = json!([
                {"type":"text", BUILD_KEY:{"step":2,"enter":{"effect":"fade","durationMs":150}}},
                {"type":"shape", BUILD_KEY:{"step":2,"enter":{"effect":"scale","durationMs":300}}}
            ]);
            true
        }));
        assert!(!session.selected_layer_build().after_previous);
        session
            .inspector_set("build-after-previous", "true")
            .unwrap();
        let stored = session.current_show().unwrap();
        let stored = freeshow_data::ShowFile::load(stored.path.as_deref().unwrap()).unwrap();
        assert_eq!(
            stored.show["slides"]["s1"]["items"][0][BUILD_KEY]["afterPrevious"],
            true
        );
        assert_eq!(
            stored.show["slides"]["s1"]["items"][1][BUILD_KEY]["afterPrevious"],
            true
        );
        assert!(session.selected_layer_build().after_previous);
        assert!(session.undo());
        assert!(!session.selected_layer_build().after_previous);
        assert!(!session.current_show().unwrap().show["slides"]["s1"]["items"][1][BUILD_KEY]["afterPrevious"].is_boolean());
    }

    #[test]
    fn build_order_uses_playback_steps_and_selects_layers_off_air() {
        let root = tempfile::tempdir().unwrap();
        let mut session = Session::open(root.path().to_path_buf());
        session.output_width = 160;
        session.output_height = 90;
        assert!(session.create_show());
        assert!(session.save_lyrics("Already live"));
        session.select_slide(0);
        let live = crate::live::audience_frame(&session);
        assert!(session.create_show());
        assert!(session.edit_selected_slide("Set up a build order", |show, slide, _| {
            show.show["slides"][slide]["items"] = json!([
                {"type":"shape", "name":"Final", BUILD_KEY:{"step":8,"enter":{"effect":"fade","durationMs":100}}},
                {"type":"media", "src":"Pictures/Logo.png", BUILD_KEY:{"step":3,"enter":{"effect":"scale","durationMs":200}}},
                {"type":"text", "lines":[{"text":[{"value":"Announcement"}]}], BUILD_KEY:{"step":3,"afterPrevious":true,"enter":{"effect":"fade","durationMs":300}}},
                {"name":"Hidden", "style":"display:none;", BUILD_KEY:{"step":1,"enter":{"effect":"fade","durationMs":100}}},
                {"name":"Welcome", BUILD_KEY:{"step":0,"enter":{"effect":"fade","durationMs":100}}},
                {"name":"Legacy", "clickReveal":true, BUILD_KEY:{"step":2,"enter":{"effect":"fade","durationMs":100}}}
            ]);
            true
        }));
        let rows = session.build_order();
        let actual = rows
            .iter()
            .map(|row| (row.layer, row.step, row.name.as_str(), row.trigger))
            .collect::<Vec<_>>();
        assert_eq!(
            actual,
            [
                (4, 0, "Welcome", "On Take"),
                (2, 3, "Announcement", "After Previous"),
                (1, 3, "Logo.png", "After Previous"),
                (0, 8, "Final", "On Click"),
            ]
        );

        assert!(session.select_item(rows[3].layer));
        assert_eq!(session.selected_layer_build().step, 8);
        session.inspector_set("build-step", "2").unwrap();
        assert_eq!(session.build_order()[1].name, "Final");
        assert!(session.undo());
        assert_eq!(session.build_order()[3].name, "Final");
        assert_eq!(crate::live::audience_frame(&session), live);
    }
}
