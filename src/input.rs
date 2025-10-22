use winit::event::{DeviceEvent, ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::{Key, NamedKey};

#[derive(Default)]
pub struct Input {
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

    pub fn push(&mut self, ev: InputEvent) {
        match &ev {
            InputEvent::Key { key, pressed } => {
                if *pressed {
                    match key {
                        Key::Named(NamedKey::Space) => self.space_pressed = true,
                        Key::Character(ch) if ch.eq_ignore_ascii_case("b") => self.b_pressed = true,
                        Key::Character(ch) if ch.eq_ignore_ascii_case("m") => self.mesh_toggle_pressed = true,
                        Key::Character(ch) if ch.eq_ignore_ascii_case("l") => self.frustum_lock_toggle = true,
                        _ => {}
                    }
                }
                let is_down = *pressed;
                match key {
                    Key::Character(ch) if ch.eq_ignore_ascii_case("w") => self.forward_held = is_down,
                    Key::Character(ch) if ch.eq_ignore_ascii_case("s") => self.backward_held = is_down,
                    Key::Character(ch) if ch.eq_ignore_ascii_case("a") => self.left_held = is_down,
                    Key::Character(ch) if ch.eq_ignore_ascii_case("d") => self.right_held = is_down,
                    Key::Character(ch) if ch.eq_ignore_ascii_case("e") => self.ascend_held = is_down,
                    Key::Character(ch) if ch.eq_ignore_ascii_case("q") => self.descend_held = is_down,
                    Key::Character(ch) if ch.eq_ignore_ascii_case("z") => self.roll_left_held = is_down,
                    Key::Character(ch) if ch.eq_ignore_ascii_case("c") => self.roll_right_held = is_down,
                    Key::Named(NamedKey::Shift) => self.boost_held = is_down,
                    _ => {}
                }
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
