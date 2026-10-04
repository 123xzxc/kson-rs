use std::collections::HashMap;

use egui::Stroke;
use crate::gilrs_compat::{ev::Code, Axis, Button, GamepadId};
use uuid::Uuid;
use winit::keyboard::PhysicalKey;

use crate::{
    button_codes::UscButton,
    config::{GameConfig, Keybinds},
    input_state::InputState,
};

#[derive(Debug, PartialEq, Default)]
enum ActiveBinding {
    #[default]
    None,
    Button(Button),
    Axis(Axis, HashMap<Code, f32>),
}

pub struct BindingUi {
    controller: GamepadId,
    currently_binding: ActiveBinding,
    input_state: InputState,
    uuid: Uuid,
}

impl BindingUi {
    pub fn new(controller: GamepadId, input_state: InputState) -> Self {
        let uuid = {
            let lock_gilrs = input_state.lock_gilrs();
            let gilrs = lock_gilrs
                .as_ref()
                .expect("Controllers not supported on this platform");
            let uuid = gilrs.gamepad(controller).uuid();
            uuid::Uuid::from_bytes(uuid)
        };

        Self {
            controller,
            currently_binding: ActiveBinding::None,
            input_state,
            uuid,
        }
    }

    pub fn run_checks(&mut self, settings: &mut GameConfig) {
        let lock_gilrs = self.input_state.lock_gilrs();
        let gilrs = lock_gilrs
            .as_ref()
            .expect("Controllers not supported on platform");
        let gamepad = gilrs.gamepad(self.controller);
        let state = gamepad.state();
        let bindings = settings.controller_binds.entry(self.uuid).or_default();

        self.currently_binding = match std::mem::take(&mut self.currently_binding) {
            ActiveBinding::None => ActiveBinding::None,
            ActiveBinding::Button(button) => {
                if let Some((code, _)) = state.buttons().find(|x| x.1.is_pressed()) {
                    bindings.buttons.insert(button, code);
                    ActiveBinding::None
                } else {
                    ActiveBinding::Button(button)
                }
            }
            ActiveBinding::Axis(axis, mut states) => state
                .axes()
                .find_map(|(code, axis_data)| {
                    let value = *states.entry(code).or_insert_with(|| axis_data.value());
                    let new_value = axis_data.value();
                    if (value - new_value).abs() > 0.1 {
                        bindings.axis.insert(axis, code);
                        Some(ActiveBinding::None)
                    } else {
                        None
                    }
                })
                .unwrap_or(ActiveBinding::Axis(axis, states)),
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, settings: &mut GameConfig) {
        let bindable_buttons: Vec<UscButton> = (0..8u8).map(UscButton::from).collect();
        let bindings = settings.controller_binds.entry(self.uuid).or_default();

        ui.label("Buttons:");
        ui.end_row();
        let stroke = Stroke::new(2.0, egui::Color32::GREEN);
        for btn in bindable_buttons {
            let mut button = egui::Button::new(btn.as_str());
            let gilrs_btn = Button::from(btn);
            let binding = ActiveBinding::Button(gilrs_btn);
            let active = self.currently_binding == binding;

            if active {
                button = button.stroke(stroke);
            }

            if ui.add(button).clicked() {
                self.currently_binding = if active { ActiveBinding::None } else { binding };
            }

            if let Some(bound) = bindings.buttons.get(&gilrs_btn) {
                ui.label(format!(": {bound}"));
            }

            ui.end_row();
        }

        ui.label("Lasers:");
        ui.end_row();
        let mut left_button = egui::Button::new("Left");
        let mut right_button = egui::Button::new("Right");

        let (left_active, right_active) = match self.currently_binding {
            ActiveBinding::Axis(Axis::RightStickX, _) => {
                right_button = right_button.stroke(stroke);
                (false, true)
            }
            ActiveBinding::Axis(Axis::LeftStickX, _) => {
                left_button = left_button.stroke(stroke);
                (true, false)
            }
            _ => (false, false),
        };

        let axes_states = || {
            let lock_gilrs = self.input_state.lock_gilrs();
            let gilrs = lock_gilrs
                .as_ref()
                .expect("Controllers not supported on platform");
            gilrs
                .gamepad(self.controller)
                .state()
                .axes()
                .map(|(code, state)| (code, state.value()))
                .collect()
        };

        if ui.add(left_button).clicked() {
            let left_axis = ActiveBinding::Axis(Axis::LeftStickX, axes_states());
            self.currently_binding = if left_active {
                ActiveBinding::None
            } else {
                left_axis
            };
        }
        if let Some(bound) = bindings.axis.get(&Axis::LeftStickX) {
            ui.label(format!(": {bound}"));
        }

        ui.end_row();
        if ui.add(right_button).clicked() {
            let right_axis = ActiveBinding::Axis(Axis::RightStickX, axes_states());
            self.currently_binding = if right_active {
                ActiveBinding::None
            } else {
                right_axis
            };
        }
        if let Some(bound) = bindings.axis.get(&Axis::RightStickX) {
            ui.label(format!(": {bound}"));
        }
        ui.end_row();
        ui.separator();
        ui.end_row();
        //Clear button
        match self.currently_binding {
            ActiveBinding::None => {
                if ui.button("Clear All").clicked() {
                    bindings.axis.clear();
                    bindings.buttons.clear();
                }
            }
            ActiveBinding::Button(button) => {
                if ui
                    .button(format!("Clear {}", UscButton::from(button).as_str()))
                    .clicked()
                {
                    bindings.buttons.remove(&button);
                    self.currently_binding = ActiveBinding::None;
                }
            }
            ActiveBinding::Axis(axis, _) => {
                if ui
                    .button(format!(
                        "Clear {}",
                        match axis {
                            Axis::LeftStickX => "Left",
                            Axis::RightStickX => "Right",
                            _ => "",
                        }
                    ))
                    .clicked()
                {
                    bindings.axis.remove(&axis);
                    self.currently_binding = ActiveBinding::None;
                }
            }
        }
    }
}

pub struct KeyboardBindingUi {
    currently_binding: (UscButton, usize),
    key_names: HashMap<PhysicalKey, String>,
}

/// Controller binding UI for iOS.
///
/// `gilrs` is a stub on iPadOS, so the widgets cannot read the pad state the
/// desktop `BindingUi` polls. Instead the player taps a key and then presses a
/// physical button (or flicks a stick): the Objective-C `GCController`
/// handlers call back into the gamepad module, which records the physical
/// index here, and `run_checks` turns that into a binding for the selected
/// controller.
#[cfg(target_os = "ios")]
pub struct IosBindingUi {
    controller: u32,
    currently_binding: Option<UscButton>,
}

#[cfg(target_os = "ios")]
impl IosBindingUi {
    pub fn new(controller: u32) -> Self {
        Self {
            controller,
            currently_binding: None,
        }
    }

    /// Folds a physical button press reported by Objective-C into the binding
    /// being recorded. Returns `true` when it was consumed.
    pub fn capture_button(&mut self, raw_index: i32) -> bool {
        let Some(button) = self.currently_binding.take() else {
            return false;
        };
        crate::platform::gamepad::bind_button(button, raw_index);
        log::info!("Bound {} to gamepad button {}", button.as_str(), raw_index);
        true
    }

    /// Folds a stick deflection into the binding being recorded.
    pub fn capture_axis(&mut self, axis_index: i32) -> bool {
        let Some(button) = self.currently_binding.take() else {
            return false;
        };
        crate::platform::gamepad::bind_axis(button, axis_index);
        log::info!("Bound {} to gamepad axis {}", button.as_str(), axis_index);
        true
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, settings: &mut GameConfig) {
        use crate::platform::gamepad::{binding_for, BindingKind};

        let _ = self.controller;
        let _ = settings;
        let stroke = Stroke::new(2.0, egui::Color32::GREEN);

        ui.label("Tap a key, then press the pad button or flick the stick.");
        ui.end_row();

        egui::Grid::new("ios_controller_binds")
            .striped(true)
            .show(ui, |ui| {
                for binding in UscButton::iter() {
                    let active = self.currently_binding == Some(binding);
                    let mut button = egui::Button::new(binding.as_str());
                    if active {
                        button = button.stroke(stroke);
                    }
                    if ui.add(button).clicked() {
                        self.currently_binding = if active { None } else { Some(binding) };
                    }

                    let label = match binding_for(binding).map(|b| (b.kind, b.index)) {
                        Some((BindingKind::Button, index)) => format!("button {index}"),
                        Some((BindingKind::Axis, index)) => format!("axis {index}"),
                        None => "-".to_owned(),
                    };
                    ui.label(label);
                    ui.end_row();
                }
            });

        if ui.button("Clear All").clicked() {
            crate::platform::gamepad::clear_bindings();
            self.currently_binding = None;
        }
    }
}

impl KeyboardBindingUi {
    pub fn new() -> Self {
        Self {
            currently_binding: (UscButton::Other(Button::Unknown), 0),
            key_names: HashMap::new(),
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, settings: &mut GameConfig) {
        let stroke = Stroke::new(2.0, egui::Color32::GREEN);

        egui::Grid::new("keybinds_ui").striped(true).show(ui, |ui| {
            for binding in UscButton::iter() {
                ui.label(format!("{binding}:"));

                for (i, b) in settings.keybinds.iter().enumerate() {
                    let active =
                        binding == self.currently_binding.0 && i == self.currently_binding.1;
                    let old_bind = b.get(binding).unwrap();

                    let key_name =
                        self.key_names
                            .entry(old_bind)
                            .or_insert_with(|| match old_bind {
                                winit::keyboard::PhysicalKey::Code(key_code) => {
                                    format!("{key_code:?}")
                                        .trim_start_matches("Key")
                                        .trim_start_matches("Digit")
                                        .to_string()
                                }
                                winit::keyboard::PhysicalKey::Unidentified(_) => "Unk".to_owned(),
                            });

                    let mut button = egui::Button::new(key_name.as_str());
                    if active {
                        button = button.stroke(stroke)
                    }
                    if ui.add(button).clicked() {
                        self.currently_binding = (binding, i)
                    }
                }
                ui.end_row();
            }
        });
        ui.end_row();
        if ui.button("Add key set").clicked() {
            settings.keybinds.push(Keybinds::default());
        }

        if settings.keybinds.len() > 1 && ui.button("Remove last key set").clicked() {
            _ = settings.keybinds.pop();
        }
    }

    pub fn key_pressed(&mut self, key: PhysicalKey, settings: &mut GameConfig) {
        settings.keybinds[self.currently_binding.1].set(self.currently_binding.0, key);
        self.currently_binding = (UscButton::Other(Button::Unknown), 0);
    }
}
