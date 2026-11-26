// SPDX-License-Identifier: GPL-3.0-only

use freeshow_core::{AppSettings, DisplayIdentity, WindowPlacement};
use slint::ComponentHandle;
use slint::winit_030::{WinitWindowAccessor, winit::monitor::MonitorHandle};
use std::{cell::RefCell, path::PathBuf, rc::Rc};

use crate::{ControlWindow, OutputWindow, Session, StageWindow};

pub struct Displays {
    pub handles: Vec<MonitorHandle>,
    pub identities: Vec<DisplayIdentity>,
    pub operator: Option<usize>,
}

struct AudienceDisplay<M = MonitorHandle> {
    active: Option<(M, DisplayIdentity)>,
    interrupted: Option<DisplayIdentity>,
}

impl AudienceDisplay {
    pub fn place(
        &mut self,
        window: &slint::Window,
        settings: &AppSettings,
        displays: &Displays,
    ) -> Result<MonitorHandle, &'static str> {
        let index = self.target(settings, &displays.identities, displays.operator)?;
        let target = &displays.identities[index];
        let mut placement = output_placement(settings, target);
        if !explicit_assignment(settings)
            && settings
                .active_profile()
                .and_then(|profile| profile.output.as_ref())
                .is_none()
            && displays.operator == Some(index)
        {
            placement.x = placement.x.saturating_add(80);
            placement.y = placement.y.saturating_add(80);
        }
        place(window, &placement);
        let handle = displays.handles[index].clone();
        self.active = Some((handle.clone(), target.clone()));
        Ok(handle)
    }

    fn active_monitor(&self) -> Option<MonitorHandle> {
        self.active.as_ref().map(|(handle, _)| handle.clone())
    }
}

impl<M> Default for AudienceDisplay<M> {
    fn default() -> Self {
        Self {
            active: None,
            interrupted: None,
        }
    }
}

const DISCONNECTED: &str =
    "Audience display disconnected. Reconnect it or choose a connected screen.";
const RECONNECTED: &str = "Audience display reconnected. Click Output to reopen it.";

impl<M: PartialEq> AudienceDisplay<M> {
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn reopened(&mut self) {
        self.interrupted = None;
    }

    fn target(
        &self,
        settings: &AppSettings,
        displays: &[DisplayIdentity],
        operator: Option<usize>,
    ) -> Result<usize, &'static str> {
        self.interrupted.as_ref().map_or_else(
            || target_index(settings, displays, operator),
            |display| display.locate(displays).ok_or(DISCONNECTED),
        )
    }

    fn watch(&mut self, handles: &[M], visible: bool) -> bool {
        if !visible
            || self
                .active
                .as_ref()
                .is_none_or(|(handle, _)| handles.contains(handle))
        {
            return false;
        }
        self.interrupted = self.active.take().map(|(_, identity)| identity);
        true
    }

    fn warning(
        &self,
        settings: &AppSettings,
        displays: &[DisplayIdentity],
        operator: Option<usize>,
    ) -> Option<&'static str> {
        match self.target(settings, displays, operator) {
            Err(error) => Some(error),
            Ok(_) if self.interrupted.is_some() => Some(RECONNECTED),
            Ok(_) => None,
        }
    }
}

impl Displays {
    pub fn read(window: &slint::Window) -> Option<Self> {
        window.with_winit_window(|window| {
            let handles: Vec<_> = window.available_monitors().collect();
            let operator = window
                .current_monitor()
                .or_else(|| window.primary_monitor())
                .and_then(|monitor| handles.iter().position(|handle| *handle == monitor));
            let mut identities: Vec<_> = handles
                .iter()
                .map(|monitor| {
                    let position = monitor.position();
                    let size = monitor.size();
                    DisplayIdentity {
                        name: monitor.name().unwrap_or_default(),
                        bounds: WindowPlacement::new(
                            position.x,
                            position.y,
                            size.width,
                            size.height,
                        ),
                        match_by_name: false,
                    }
                })
                .collect();
            let mut names = std::collections::HashMap::new();
            for display in &identities {
                *names.entry(display.name.clone()).or_insert(0) += 1;
            }
            for display in &mut identities {
                display.match_by_name =
                    !display.name.is_empty() && names.get(&display.name) == Some(&1);
            }
            Self {
                handles,
                identities,
                operator,
            }
        })
    }

    pub fn label(&self, index: usize) -> String {
        let name = &self.identities[index].name;
        if name.trim().is_empty() {
            format!("Screen {}", index + 1)
        } else {
            format!("{} · {name}", index + 1)
        }
    }
}

fn contains_at_least(window: &WindowPlacement, bounds: &WindowPlacement) -> bool {
    // Keep enough of the title area on a physical display to recover the window.
    let x = i64::from(window.x);
    let y = i64::from(window.y);
    let left = i64::from(bounds.x);
    let top = i64::from(bounds.y);
    let right = left + i64::from(bounds.width);
    let bottom = top + i64::from(bounds.height);
    (left..right).contains(&x)
        && (top..bottom).contains(&y)
        && right - x >= i64::from(window.width.min(32))
        && bottom - y >= i64::from(window.height.min(24))
}

pub fn profile_on_displays(placement: &WindowPlacement, displays: &[DisplayIdentity]) -> bool {
    displays
        .iter()
        .any(|display| contains_at_least(placement, &display.bounds))
}

pub fn assigned_index(settings: &AppSettings, displays: &[DisplayIdentity]) -> Option<usize> {
    if let Some(saved) = &settings.output_display {
        return saved.locate(displays);
    }
    settings
        .output_screen
        .filter(|&index| index < displays.len())
}

pub fn explicit_assignment(settings: &AppSettings) -> bool {
    settings.output_display.is_some() || settings.output_screen.is_some()
}

pub fn target_index(
    settings: &AppSettings,
    displays: &[DisplayIdentity],
    operator: Option<usize>,
) -> Result<usize, &'static str> {
    if displays.is_empty() {
        return Err("No displays reported. Check the display connection.");
    }
    if explicit_assignment(settings) {
        return assigned_index(settings, displays).ok_or(
            "Assigned audience display unavailable. Reconnect it or choose a connected screen.",
        );
    }
    if let Some(place) = settings
        .active_profile()
        .and_then(|profile| profile.output.as_ref())
    {
        return displays.iter().position(|display| contains_at_least(place, &display.bounds))
            .ok_or("The saved audience window is off screen. Choose a connected screen or update the window layout.");
    }
    Ok(freeshow_core::output_monitor_index(None, displays.len())
        .or(operator)
        .unwrap_or(0))
}

pub fn output_placement(settings: &AppSettings, target: &DisplayIdentity) -> WindowPlacement {
    let profile = settings
        .active_profile()
        .and_then(|profile| profile.output.as_ref());
    if let Some(place) = profile.filter(|place| contains_at_least(place, &target.bounds)) {
        return place.clone();
    }
    let (width, height) = profile.map_or((0, 0), |place| {
        (
            place.width.min(target.bounds.width),
            place.height.min(target.bounds.height),
        )
    });
    // Zero size tells the caller to keep the existing window size.
    WindowPlacement {
        x: target.bounds.x,
        y: target.bounds.y,
        width,
        height,
    }
}

pub fn place(window: &slint::Window, placement: &WindowPlacement) {
    window.set_position(slint::PhysicalPosition::new(placement.x, placement.y));
    if placement.width > 0 && placement.height > 0 {
        window.set_size(slint::PhysicalSize::new(placement.width, placement.height));
    }
}

pub fn fallback(window: &slint::Window, displays: &Displays, x: i32, y: i32) {
    let operator = displays
        .operator
        .and_then(|index| displays.identities.get(index));
    let anchor = operator
        .map(|display| (display.bounds.x, display.bounds.y))
        .unwrap_or_default();
    window.set_position(slint::PhysicalPosition::new(
        anchor.0.saturating_add(x),
        anchor.1.saturating_add(y),
    ));
}

pub struct DisplaySetup {
    control: slint::Weak<ControlWindow>,
    output: slint::Weak<OutputWindow>,
    stage: slint::Weak<StageWindow>,
    session: Rc<RefCell<Session>>,
    settings: Rc<RefCell<AppSettings>>,
    settings_path: PathBuf,
    audience: RefCell<AudienceDisplay>,
    picker: RefCell<Vec<DisplayIdentity>>,
}

impl DisplaySetup {
    pub fn wire(
        control: &ControlWindow,
        output: &OutputWindow,
        stage: &StageWindow,
        session: Rc<RefCell<Session>>,
        settings: Rc<RefCell<AppSettings>>,
        settings_path: PathBuf,
    ) -> Rc<Self> {
        let setup = Rc::new(Self {
            control: control.as_weak(),
            output: output.as_weak(),
            stage: stage.as_weak(),
            session,
            settings,
            settings_path,
            audience: RefCell::default(),
            picker: RefCell::default(),
        });
        let open = Rc::clone(&setup);
        control.on_open_output(move || open.toggle_audience());
        let open = Rc::clone(&setup);
        control.on_open_stage(move || open.toggle_stage());
        let pick = Rc::clone(&setup);
        control.on_pick_screen(move |index| pick.pick_screen(index));
        setup
    }

    fn save(&self) {
        if let Err(error) = self.settings.borrow().save(&self.settings_path) {
            tracing::error!(%error, "could not save display settings");
        }
    }

    /// Resolve against the operator window so the picker works while Output is closed.
    fn inventory(&self) -> Option<Displays> {
        Displays::read(self.control.upgrade()?.window())
    }

    fn put_audience_on_target(&self, output: &OutputWindow, displays: &Displays) -> bool {
        let moved =
            self.audience
                .borrow_mut()
                .place(output.window(), &self.settings.borrow(), displays);
        let Ok(monitor) = moved else {
            if output.window().is_visible() {
                let _ = output.hide();
            }
            return false;
        };
        if let Err(error) = output.show() {
            tracing::error!(%error, "could not show audience output");
            return false;
        }
        self.audience.borrow_mut().reopened();
        let (on_top, kiosk) = self.session.borrow().output_window_flags();
        crate::apply_window_behavior_on(output.window(), on_top, kiosk, Some(monitor));
        true
    }

    pub fn apply_audience_flags(&self) {
        if let Some(output) = self
            .output
            .upgrade()
            .filter(|output| output.window().is_visible())
        {
            let (on_top, kiosk) = self.session.borrow().output_window_flags();
            crate::apply_window_behavior_on(
                output.window(),
                on_top,
                kiosk,
                self.audience.borrow().active_monitor(),
            );
        }
    }

    pub fn startup(&self) -> bool {
        let Some(displays) = self.inventory() else {
            self.sync();
            return false;
        };
        let legacy = {
            let settings = self.settings.borrow();
            if settings.output_display.is_none() {
                settings
                    .output_screen
                    .and_then(|index| displays.identities.get(index))
                    .cloned()
            } else {
                None
            }
        };
        if let Some(display) = legacy {
            self.settings.borrow_mut().output_display = Some(display);
            self.save();
        }
        self.apply_profile();
        if (self.settings.borrow().show_output || self.session.borrow().preferences.auto_output())
            && let Some(output) = self.output.upgrade()
        {
            self.put_audience_on_target(&output, &displays);
        }
        if self.settings.borrow().show_stage
            && let Some(stage) = self.stage.upgrade()
        {
            crate::place_stage(stage.window(), &self.settings.borrow(), &displays);
            let _ = stage.show();
        }
        self.sync();
        true
    }

    fn toggle_audience(&self) {
        let Some(output) = self.output.upgrade() else {
            return;
        };
        if output.window().is_visible() {
            let _ = output.hide();
            self.settings.borrow_mut().show_output = false;
            self.save();
        } else if let Some(displays) = self.inventory()
            && self.put_audience_on_target(&output, &displays)
        {
            self.settings.borrow_mut().show_output = true;
            self.save();
        }
        self.sync();
    }

    fn toggle_stage(&self) {
        let Some(stage) = self.stage.upgrade() else {
            return;
        };
        if stage.window().is_visible() {
            let _ = stage.hide();
        } else {
            if let Some(displays) = self.inventory() {
                crate::place_stage(stage.window(), &self.settings.borrow(), &displays);
            }
            if let Err(error) = stage.show() {
                tracing::error!(%error, "could not show stage output");
            }
        }
        self.settings.borrow_mut().show_stage = stage.window().is_visible();
        self.save();
        self.sync();
    }

    fn pick_screen(&self, index: i32) {
        let Some(displays) = self.inventory() else {
            return;
        };
        let chosen = if let Some(index) = freeshow_core::screen_from_combo(index) {
            let mapped = self
                .picker
                .borrow()
                .get(index)
                .and_then(|display| display.locate(&displays.identities));
            let Some(index) = mapped else {
                self.sync();
                return;
            };
            Some(index)
        } else {
            None
        };
        {
            let mut settings = self.settings.borrow_mut();
            settings.output_screen = chosen;
            settings.output_display = chosen.map(|index| displays.identities[index].clone());
        }
        self.audience.borrow_mut().reset();
        if let Some(output) = self
            .output
            .upgrade()
            .filter(|output| output.window().is_visible())
        {
            self.put_audience_on_target(&output, &displays);
        }
        self.save();
        self.sync();
    }

    pub fn apply_profile(&self) {
        let Some(displays) = self.inventory() else {
            return;
        };
        let Some((control, stage)) = self.control.upgrade().zip(self.stage.upgrade()) else {
            return;
        };
        crate::apply_window_profile(
            control.window(),
            stage.window(),
            &self.settings.borrow(),
            &displays,
        );
        if let Some(output) = self
            .output
            .upgrade()
            .filter(|output| output.window().is_visible())
        {
            self.put_audience_on_target(&output, &displays);
        }
        self.sync();
    }

    pub fn sync(&self) {
        let Some(control) = self.control.upgrade() else {
            return;
        };
        let Some(displays) = Displays::read(control.window()) else {
            control.set_display_warning("Display information is unavailable.".into());
            return;
        };
        let Some((output, stage)) = self.output.upgrade().zip(self.stage.upgrade()) else {
            return;
        };
        let settings = self.settings.borrow();
        let mut audience = self.audience.borrow_mut();
        let was_visible = output.window().is_visible();
        audience.watch(&displays.handles, was_visible);
        if was_visible
            && audience.interrupted.is_some()
            && let Err(error) = output.hide()
        {
            tracing::warn!(%error, "could not hide the disconnected audience window");
        }
        let assigned = assigned_index(&settings, &displays.identities);
        let missing = explicit_assignment(&settings) && assigned.is_none();
        let expected_size = displays.identities.len() + 1 + usize::from(missing);
        let mut inventory = self.picker.borrow_mut();
        let topology_changed = *inventory != displays.identities;
        let rows = control.get_screen_choices();
        use slint::Model;
        if topology_changed || rows.row_count() != expected_size {
            let mut choices = vec!["Auto".to_string()];
            choices.extend((0..displays.identities.len()).map(|index| displays.label(index)));
            if missing {
                choices.push("Assigned display unavailable".into());
            }
            control.set_screen_choices(crate::strings(&choices));
            *inventory = displays.identities.clone();
        }
        let picker_index = if missing {
            displays.identities.len() as i32 + 1
        } else {
            freeshow_core::screen_combo_index(assigned)
        };
        if control.get_screen_index() != picker_index {
            control.set_screen_index(picker_index);
        }
        control.set_screen_label(
            crate::screen_label(assigned, &self.session.borrow().preferences.outputs).into(),
        );
        let stage_visible = stage.window().is_visible();
        let output_visible = output.window().is_visible();
        control.set_output_open(output_visible);
        control.set_stage_open(stage_visible);
        if topology_changed
            && stage_visible
            && !displays.identities.is_empty()
            && let Some(place) = crate::capture_placement(stage.window())
            && !profile_on_displays(&place, &displays.identities)
        {
            fallback(stage.window(), &displays, 120, 140);
        }
        let mut warnings = Vec::new();
        if let Some(warning) = audience.warning(&settings, &displays.identities, displays.operator)
        {
            warnings.push(warning);
        }
        let on_display = |window: &slint::Window| {
            let monitor = window
                .with_winit_window(|window| window.current_monitor())
                .flatten()?;
            displays
                .handles
                .iter()
                .position(|handle| *handle == monitor)
        };
        let actual_audience = output_visible
            .then(|| on_display(output.window()))
            .flatten();
        if let Some(actual) = actual_audience
            && audience
                .active_monitor()
                .is_some_and(|assigned| displays.handles[actual] != assigned)
        {
            warnings.push("The audience window is on a different screen. Verify its location before presenting.");
        }
        if settings
            .active_profile()
            .and_then(|profile| profile.stage.as_ref())
            .is_some_and(|place| !profile_on_displays(place, &displays.identities))
        {
            warnings
                .push("Saved stage location is off screen. Stage opens on the operator display.");
        }
        control.set_display_warning(warnings.join(" ").into());
        let audience_text = if output_visible {
            actual_audience.map_or_else(
                || "Audience window open · location unreported".to_string(),
                |index| format!("Audience on {}", displays.label(index)),
            )
        } else {
            "Audience window closed".into()
        };
        let count = displays.identities.len();
        let stage_text = if stage_visible {
            on_display(stage.window()).map_or_else(
                || "Stage window open".to_string(),
                |index| format!("Stage on {}", displays.label(index)),
            )
        } else {
            "Stage window closed".into()
        };
        control.set_display_summary(
            format!(
                "{count} connected {} · {audience_text} · {stage_text}",
                if count == 1 { "display" } else { "displays" },
            )
            .into(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use freeshow_core::WindowProfile;

    fn display(name: &str, x: i32) -> DisplayIdentity {
        DisplayIdentity {
            name: name.into(),
            bounds: WindowPlacement::new(x, 0, 1920, 1080),
            match_by_name: true,
        }
    }

    #[test]
    fn absent_assignments_and_offscreen_profiles_require_a_choice() {
        let operator = display("Operator", 0);
        let projector = display("Projector", -1920);
        let mut settings = AppSettings::default();
        assert_eq!(
            target_index(&settings, &[operator.clone(), projector.clone()], Some(0)),
            Ok(1)
        );
        settings.output_screen = Some(1);
        settings.output_display = Some(projector.clone());
        assert_eq!(
            target_index(&settings, &[projector.clone(), operator.clone()], Some(1)),
            Ok(0)
        );
        assert!(target_index(&settings, std::slice::from_ref(&operator), Some(0)).is_err());

        settings.output_screen = None;
        settings.output_display = None;
        let placement = WindowPlacement::new(-1200, 45, 800, 600);
        settings.save_window_profile(
            "Room",
            WindowProfile {
                output: Some(placement.clone()),
                ..WindowProfile::default()
            },
        );
        assert_eq!(
            target_index(&settings, &[operator.clone(), projector.clone()], Some(0)),
            Ok(1)
        );
        assert_eq!(output_placement(&settings, &projector), placement);
        assert!(target_index(&settings, std::slice::from_ref(&operator), Some(0)).is_err());
        settings.output_screen = Some(0);
        settings.output_display = Some(operator.clone());
        assert_eq!(
            target_index(&settings, std::slice::from_ref(&operator), Some(0)),
            Ok(0)
        );
        assert_eq!(
            output_placement(&settings, &operator),
            WindowPlacement::new(0, 0, 800, 600)
        );
    }

    #[test]
    fn loss_and_reconnection_preserve_the_old_target_until_an_operator_reopens() {
        let operator = display("Operator", 0);
        let projector = display("Projector", -1920);
        let settings = AppSettings::default();
        let mut audience = AudienceDisplay {
            active: Some((8, projector.clone())),
            ..Default::default()
        };
        assert!(!audience.watch(&[8, 2], true));
        assert!(audience.watch(&[2], true));
        assert!(audience.active.is_none());
        assert_eq!(
            audience.target(&settings, std::slice::from_ref(&operator), Some(0)),
            Err(DISCONNECTED)
        );
        let reconnected = [display("Projector", 1920), operator.clone()];
        assert!(!audience.watch(&[9, 2], false));
        assert_eq!(
            audience.warning(&settings, &reconnected, Some(1)),
            Some(RECONNECTED)
        );
        assert_eq!(audience.target(&settings, &reconnected, Some(1)), Ok(0));
        assert!(audience.active.is_none());
        audience.reopened();
        assert_eq!(audience.warning(&settings, &reconnected, Some(1)), None);
        audience.reset();
        assert_eq!(audience.target(&settings, &[operator], Some(0)), Ok(0));
    }
}
