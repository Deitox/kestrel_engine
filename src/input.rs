use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use winit::event::{DeviceEvent, ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::{Key, NamedKey};

pub struct Input {
    bindings: InputBindings,
    pub mouse_delta: (f32, f32),
    pub wheel: f32,
    pub events: Vec<InputEvent>,
    space_pressed: bool,
    b_pressed: bool,
    mesh_toggle_pressed: bool,
    forward_held: bool,
    backward_held: bool,
    left_held: bool,
    right_held: bool,
    ascend_held: bool,
    descend_held: bool,
    boost_held: bool,
    ctrl_held: bool,
    roll_left_held: bool,
    roll_right_held: bool,
    frustum_lock_toggle: bool,
    cursor_pos: Option<(f32, f32)>,
    left_pressed: bool,
    left_clicked: bool,
    right_pressed: bool,
}

impl Input {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_config(path: impl AsRef<Path>) -> Self {
        let bindings = InputBindings::load_or_default(path);
        Self::with_bindings(bindings)
    }

    fn with_bindings(bindings: InputBindings) -> Self {
        Self {
            bindings,
            mouse_delta: (0.0, 0.0),
            wheel: 0.0,
            events: Vec::new(),
            space_pressed: false,
            b_pressed: false,
            mesh_toggle_pressed: false,
            forward_held: false,
            backward_held: false,
            left_held: false,
            right_held: false,
            ascend_held: false,
            descend_held: false,
            boost_held: false,
            ctrl_held: false,
            roll_left_held: false,
            roll_right_held: false,
            frustum_lock_toggle: false,
            cursor_pos: None,
            left_pressed: false,
            left_clicked: false,
            right_pressed: false,
        }
    }

    pub fn push(&mut self, ev: InputEvent) {
        match &ev {
            InputEvent::Key { key, pressed } => {
                self.apply_key_binding(key, *pressed);
            }
            InputEvent::MouseMove { dx, dy } => {
                self.mouse_delta.0 += *dx;
                self.mouse_delta.1 += *dy;
            }
            InputEvent::Wheel { delta } => {
                self.wheel += *delta;
            }
            InputEvent::MouseButton { button, pressed } => match button {
                MouseButton::Left => {
                    if *pressed {
                        self.left_clicked = true;
                        self.left_pressed = true;
                    } else {
                        self.left_pressed = false;
                    }
                }
                MouseButton::Right => {
                    self.right_pressed = *pressed;
                }
                _ => {}
            },
            InputEvent::CursorPos { x, y } => {
                self.cursor_pos = Some((*x, *y));
            }
            InputEvent::Other => {}
        }
        self.events.push(ev);
    }

    pub fn clear_frame(&mut self) {
        self.events.clear();
        self.mouse_delta = (0.0, 0.0);
        self.wheel = 0.0;
        self.left_clicked = false;
        self.mesh_toggle_pressed = false;
        self.frustum_lock_toggle = false;
    }

    pub fn consume_wheel_delta(&mut self) -> Option<f32> {
        if self.wheel.abs() > 0.0 {
            let d = self.wheel;
            self.wheel = 0.0;
            Some(d)
        } else {
            None
        }
    }

    pub fn take_space_pressed(&mut self) -> bool {
        let v = self.space_pressed;
        self.space_pressed = false;
        v
    }

    pub fn take_b_pressed(&mut self) -> bool {
        let v = self.b_pressed;
        self.b_pressed = false;
        v
    }

    pub fn take_mesh_toggle(&mut self) -> bool {
        let v = self.mesh_toggle_pressed;
        self.mesh_toggle_pressed = false;
        v
    }

    pub fn take_left_click(&mut self) -> bool {
        let was = self.left_clicked;
        self.left_clicked = false;
        was
    }

    pub fn right_held(&self) -> bool {
        self.right_pressed
    }
    pub fn left_held(&self) -> bool {
        self.left_pressed
    }
    pub fn cursor_position(&self) -> Option<(f32, f32)> {
        self.cursor_pos
    }
    pub fn freefly_forward(&self) -> bool {
        self.forward_held
    }
    pub fn freefly_backward(&self) -> bool {
        self.backward_held
    }
    pub fn freefly_left(&self) -> bool {
        self.left_held
    }
    pub fn freefly_right(&self) -> bool {
        self.right_held
    }
    pub fn freefly_ascend(&self) -> bool {
        self.ascend_held
    }
    pub fn freefly_descend(&self) -> bool {
        self.descend_held
    }
    pub fn freefly_boost(&self) -> bool {
        self.boost_held
    }
    pub fn shift_held(&self) -> bool {
        self.boost_held
    }
    pub fn ctrl_held(&self) -> bool {
        self.ctrl_held
    }
    pub fn freefly_roll_left(&self) -> bool {
        self.roll_left_held
    }
    pub fn freefly_roll_right(&self) -> bool {
        self.roll_right_held
    }
    pub fn take_frustum_lock_toggle(&mut self) -> bool {
        let pressed = self.frustum_lock_toggle;
        self.frustum_lock_toggle = false;
        pressed
    }

    fn apply_key_binding(&mut self, key: &Key, pressed: bool) {
        if let Some(binding_key) = InputKeyBinding::from_event_key(key) {
            let actions: Vec<_> = self.bindings.actions_for_key(&binding_key).collect();
            for action in actions {
                self.update_action_state(action, pressed);
            }
        }
    }

    fn update_action_state(&mut self, action: InputAction, pressed: bool) {
        match action {
            InputAction::SpawnBurstSmall => {
                if pressed {
                    self.space_pressed = true;
                }
            }
            InputAction::SpawnBurstLarge => {
                if pressed {
                    self.b_pressed = true;
                }
            }
            InputAction::MeshToggle => {
                if pressed {
                    self.mesh_toggle_pressed = true;
                }
            }
            InputAction::FrustumLockToggle => {
                if pressed {
                    self.frustum_lock_toggle = true;
                }
            }
            InputAction::FreeflyForward => self.forward_held = pressed,
            InputAction::FreeflyBackward => self.backward_held = pressed,
            InputAction::FreeflyLeft => self.left_held = pressed,
            InputAction::FreeflyRight => self.right_held = pressed,
            InputAction::FreeflyAscend => self.ascend_held = pressed,
            InputAction::FreeflyDescend => self.descend_held = pressed,
            InputAction::FreeflyRollLeft => self.roll_left_held = pressed,
            InputAction::FreeflyRollRight => self.roll_right_held = pressed,
            InputAction::FreeflyBoost => self.boost_held = pressed,
            InputAction::ModifierCtrl => self.ctrl_held = pressed,
        }
    }
}

impl Default for Input {
    fn default() -> Self {
        Self::with_bindings(InputBindings::default())
    }
}

#[derive(Debug, Clone)]
struct InputBindings {
    key_to_actions: HashMap<InputKeyBinding, Vec<InputAction>>,
}

impl InputBindings {
    fn load_or_default(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        match fs::read_to_string(path) {
            Ok(contents) => match serde_json::from_str::<InputConfigFile>(&contents) {
                Ok(config) => Self::from_config(config, &path.display().to_string()),
                Err(err) => {
                    eprintln!(
                        "[input] Failed to parse {}: {err}. Falling back to default bindings.",
                        path.display()
                    );
                    Self::default()
                }
            },
            Err(err) => {
                eprintln!(
                    "[input] Failed to read {}: {err}. Falling back to default bindings.",
                    path.display()
                );
                Self::default()
            }
        }
    }

    fn from_config(config: InputConfigFile, origin: &str) -> Self {
        let overrides = config.into_overrides(origin);
        Self::with_overrides(overrides)
    }

    fn with_overrides(overrides: HashMap<InputAction, Vec<InputKeyBinding>>) -> Self {
        let mut action_map = Self::default_action_map();
        for (action, keys) in overrides {
            if keys.is_empty() {
                continue;
            }
            action_map.insert(action, keys);
        }
        Self::from_action_map(action_map)
    }

    fn default_action_map() -> HashMap<InputAction, Vec<InputKeyBinding>> {
        use InputAction::*;
        let mut map = HashMap::new();
        map.insert(SpawnBurstSmall, vec![InputKeyBinding::named(NamedKeyCode::Space)]);
        map.insert(SpawnBurstLarge, vec![InputKeyBinding::character("b")]);
        map.insert(MeshToggle, vec![InputKeyBinding::character("m")]);
        map.insert(FrustumLockToggle, vec![InputKeyBinding::character("l")]);
        map.insert(FreeflyForward, vec![InputKeyBinding::character("w")]);
        map.insert(FreeflyBackward, vec![InputKeyBinding::character("s")]);
        map.insert(FreeflyLeft, vec![InputKeyBinding::character("a")]);
        map.insert(FreeflyRight, vec![InputKeyBinding::character("d")]);
        map.insert(FreeflyAscend, vec![InputKeyBinding::character("e")]);
        map.insert(FreeflyDescend, vec![InputKeyBinding::character("q")]);
        map.insert(FreeflyRollLeft, vec![InputKeyBinding::character("z")]);
        map.insert(FreeflyRollRight, vec![InputKeyBinding::character("c")]);
        map.insert(FreeflyBoost, vec![InputKeyBinding::named(NamedKeyCode::Shift)]);
        map.insert(ModifierCtrl, vec![InputKeyBinding::named(NamedKeyCode::Control)]);
        map
    }

    fn from_action_map(action_map: HashMap<InputAction, Vec<InputKeyBinding>>) -> Self {
        let mut key_to_actions: HashMap<InputKeyBinding, Vec<InputAction>> = HashMap::new();
        for (action, keys) in action_map {
            for key in keys {
                key_to_actions.entry(key).or_default().push(action);
            }
        }
        Self { key_to_actions }
    }

    fn actions_for_key(&self, key: &InputKeyBinding) -> impl Iterator<Item = InputAction> + '_ {
        self.key_to_actions.get(key).into_iter().flatten().copied()
    }
}

impl Default for InputBindings {
    fn default() -> Self {
        Self::from_action_map(Self::default_action_map())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum InputKeyBinding {
    Character(String),
    Named(NamedKeyCode),
}

impl InputKeyBinding {
    fn character(ch: &str) -> Self {
        Self::Character(ch.to_lowercase())
    }

    fn named(named: NamedKeyCode) -> Self {
        Self::Named(named)
    }

    fn from_event_key(key: &Key) -> Option<Self> {
        match key {
            Key::Character(ch) => {
                let s = ch.to_string();
                if s.is_empty() {
                    None
                } else {
                    Some(Self::Character(s.to_lowercase()))
                }
            }
            Key::Named(named) => NamedKeyCode::from_named_key(named).map(Self::Named),
            _ => None,
        }
    }

    fn from_config_value(raw: &str) -> Result<Self, ()> {
        let normalized = raw.trim().to_lowercase();
        if normalized.is_empty() {
            return Err(());
        }
        if let Some(named) = NamedKeyCode::from_str(&normalized) {
            return Ok(Self::Named(named));
        }
        if normalized.chars().count() == 1 {
            return Ok(Self::Character(normalized));
        }
        Err(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum NamedKeyCode {
    Space,
    Shift,
    Control,
}

impl NamedKeyCode {
    fn from_named_key(key: &NamedKey) -> Option<Self> {
        match key {
            NamedKey::Space => Some(Self::Space),
            NamedKey::Shift => Some(Self::Shift),
            NamedKey::Control => Some(Self::Control),
            _ => None,
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "space" => Some(Self::Space),
            "shift" | "left_shift" | "right_shift" => Some(Self::Shift),
            "ctrl" | "control" | "left_ctrl" | "right_ctrl" => Some(Self::Control),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum InputAction {
    SpawnBurstSmall,
    SpawnBurstLarge,
    MeshToggle,
    FrustumLockToggle,
    FreeflyForward,
    FreeflyBackward,
    FreeflyLeft,
    FreeflyRight,
    FreeflyAscend,
    FreeflyDescend,
    FreeflyRollLeft,
    FreeflyRollRight,
    FreeflyBoost,
    ModifierCtrl,
}

impl InputAction {
    fn from_str(value: &str) -> Option<Self> {
        match value {
            "spawn_burst_small" => Some(Self::SpawnBurstSmall),
            "spawn_burst_large" => Some(Self::SpawnBurstLarge),
            "mesh_toggle" => Some(Self::MeshToggle),
            "frustum_lock_toggle" => Some(Self::FrustumLockToggle),
            "freefly_forward" => Some(Self::FreeflyForward),
            "freefly_backward" => Some(Self::FreeflyBackward),
            "freefly_left" => Some(Self::FreeflyLeft),
            "freefly_right" => Some(Self::FreeflyRight),
            "freefly_ascend" => Some(Self::FreeflyAscend),
            "freefly_descend" => Some(Self::FreeflyDescend),
            "freefly_roll_left" => Some(Self::FreeflyRollLeft),
            "freefly_roll_right" => Some(Self::FreeflyRollRight),
            "freefly_boost" => Some(Self::FreeflyBoost),
            "modifier_ctrl" => Some(Self::ModifierCtrl),
            _ => None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct InputConfigFile {
    #[serde(default)]
    bindings: HashMap<String, Vec<String>>,
}

impl InputConfigFile {
    fn into_overrides(self, origin: &str) -> HashMap<InputAction, Vec<InputKeyBinding>> {
        let mut overrides = HashMap::new();
        for (action_name, keys) in self.bindings {
            let action_key = action_name.trim().to_lowercase();
            match InputAction::from_str(&action_key) {
                Some(action) => {
                    let mut parsed = Vec::new();
                    for key in keys {
                        match InputKeyBinding::from_config_value(&key) {
                            Ok(binding) => parsed.push(binding),
                            Err(_) => eprintln!(
                                "[input] {origin}: unknown key '{key}' for action '{action_name}', ignoring."
                            ),
                        }
                    }
                    if parsed.is_empty() {
                        eprintln!(
                            "[input] {origin}: action '{action_name}' has no valid keys, keeping defaults."
                        );
                        continue;
                    }
                    overrides.insert(action, parsed);
                }
                None => eprintln!("[input] {origin}: unknown action '{action_name}', ignoring."),
            }
        }
        overrides
    }
}

pub enum InputEvent {
    Key { key: Key, pressed: bool },
    MouseMove { dx: f32, dy: f32 },
    Wheel { delta: f32 },
    MouseButton { button: MouseButton, pressed: bool },
    CursorPos { x: f32, y: f32 },
    Other,
}

impl InputEvent {
    pub fn from_window_event(ev: &WindowEvent) -> Self {
        match ev {
            WindowEvent::MouseWheel { delta, .. } => {
                let d = match delta {
                    MouseScrollDelta::LineDelta(_, y) => *y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32,
                };
                InputEvent::Wheel { delta: d as f32 }
            }
            WindowEvent::CursorMoved { position, .. } => {
                InputEvent::CursorPos { x: position.x as f32, y: position.y as f32 }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                InputEvent::MouseButton { button: *button, pressed: *state == ElementState::Pressed }
            }
            WindowEvent::KeyboardInput { event, .. } => InputEvent::Key {
                key: event.logical_key.clone(),
                pressed: event.state == ElementState::Pressed,
            },
            _ => InputEvent::Other,
        }
    }

    pub fn from_device_event(ev: &DeviceEvent) -> Self {
        match ev {
            DeviceEvent::MouseMotion { delta: (dx, dy) } => {
                InputEvent::MouseMove { dx: *dx as f32, dy: *dy as f32 }
            }
            _ => InputEvent::Other,
        }
    }
}
