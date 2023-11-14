use core_graphics;

// TODO(dustin): use only the things i need

use self::core_graphics::display::*;
use self::core_graphics::event::*;
use self::core_graphics::event_source::*;

use crate::macos::keycodes::*;
use crate::{Key, KeyboardControllable, MouseButton, MouseControllable};
use objc::runtime::Class;

struct MyCGEvent;

#[allow(improper_ctypes)]
#[allow(non_snake_case)]
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn CGEventPost(tapLocation: CGEventTapLocation, event: *mut MyCGEvent);
    // not present in servo/core-graphics
    fn CGEventCreateScrollWheelEvent(
        source: &CGEventSourceRef,
        units: ScrollUnit,
        wheelCount: u32,
        wheel1: i32,
        ...
    ) -> *mut MyCGEvent;
    fn CGEventSourceKeyState(stateID: i32, key: u16) -> bool;
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NSPoint {
    x: f64,
    y: f64,
}

// not present in servo/core-graphics
#[allow(dead_code)]
#[derive(Debug)]
enum ScrollUnit {
    Pixel = 0,
    Line = 1,
}
// hack

/// The main struct for handling the event emitting
pub struct Enigo {
    event_source: Option<CGEventSource>,
    double_click_interval: u32,
    last_click_time: Option<std::time::Instant>,
    multiple_click: i64,
    flags: CGEventFlags,
}

impl Enigo {
    ///
    pub fn reset_flag(&mut self) {
        self.flags = CGEventFlags::CGEventFlagNull;
    }

    ///
    pub fn add_flag(&mut self, key: &Key) {
        let flag = match key {
            &Key::CapsLock => CGEventFlags::CGEventFlagAlphaShift,
            &Key::Shift => CGEventFlags::CGEventFlagShift,
            &Key::Control => CGEventFlags::CGEventFlagControl,
            &Key::Alt => CGEventFlags::CGEventFlagAlternate,
            &Key::Meta => CGEventFlags::CGEventFlagCommand,
            &Key::NumLock => CGEventFlags::CGEventFlagNumericPad,
            _ => CGEventFlags::CGEventFlagNull,
        };
        self.flags |= flag;
    }

    fn post(&self, event: CGEvent) {
        event.set_flags(self.flags);
        event.post(CGEventTapLocation::HID);
    }
}

impl Default for Enigo {
    fn default() -> Self {
        let mut double_click_interval = 500;
        if let Some(ns_event) = Class::get("NSEvent") {
            let tm: f64 = unsafe { msg_send![ns_event, doubleClickInterval] };
            if tm > 0. {
                double_click_interval = (tm * 1000.) as u32;
                //log::info!("double click interval: {}ms", double_click_interval);
            }
        }
        Self {
            // TODO(dustin): return error rather than panic here
            event_source: if let Ok(src) =
                CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
            {
                Some(src)
            } else {
                None
            },
            double_click_interval,
            multiple_click: 1,
            last_click_time: None,
            flags: CGEventFlags::CGEventFlagNull,
        }
    }
}

impl MouseControllable for Enigo {
    fn mouse_move_to(&mut self, x: i32, y: i32) {
        let pressed = Self::pressed_buttons();

        let event_type = if pressed & 1 > 0 {
            CGEventType::LeftMouseDragged
        } else if pressed & 2 > 0 {
            CGEventType::RightMouseDragged
        } else {
            CGEventType::MouseMoved
        };

        let dest = CGPoint::new(x as f64, y as f64);
        if let Some(src) = self.event_source.as_ref() {
            if let Ok(event) =
                CGEvent::new_mouse_event(src.clone(), event_type, dest, CGMouseButton::Left)
            {
                self.post(event);
            }
        }
    }

    fn mouse_move_relative(&mut self, x: i32, y: i32) {
        let (display_width, display_height) = Self::main_display_size();
        let (current_x, y_inv) = Self::mouse_location_raw_coords();
        let current_y = (display_height as i32) - y_inv;
        let new_x = current_x + x;
        let new_y = current_y + y;

        if new_x < 0
            || new_x as usize > display_width
            || new_y < 0
            || new_y as usize > display_height
        {
            return;
        }

        self.mouse_move_to(new_x, new_y);
    }

    fn mouse_down(&mut self, button: MouseButton) -> crate::ResultType {
        let now = std::time::Instant::now();
        if let Some(t) = self.last_click_time {
            if t.elapsed().as_millis() as u32 <= self.double_click_interval {
                self.multiple_click += 1;
            } else {
                self.multiple_click = 1;
            }
        }
        self.last_click_time = Some(now);
        let (current_x, current_y) = Self::mouse_location();
        let (button, event_type) = match button {
            MouseButton::Left => (CGMouseButton::Left, CGEventType::LeftMouseDown),
            MouseButton::Middle => (CGMouseButton::Center, CGEventType::OtherMouseDown),
            MouseButton::Right => (CGMouseButton::Right, CGEventType::RightMouseDown),
            _ => unimplemented!(),
        };
        let dest = CGPoint::new(current_x as f64, current_y as f64);
        if let Some(src) = self.event_source.as_ref() {
            if let Ok(event) = CGEvent::new_mouse_event(src.clone(), event_type, dest, button) {
                if self.multiple_click > 1 {
                    event.set_integer_value_field(
                        EventField::MOUSE_EVENT_CLICK_STATE,
                        self.multiple_click,
                    );
                }
                self.post(event);
            }
        }
        Ok(())
    }

    fn mouse_up(&mut self, button: MouseButton) {
        let (current_x, current_y) = Self::mouse_location();
        let (button, event_type) = match button {
            MouseButton::Left => (CGMouseButton::Left, CGEventType::LeftMouseUp),
            MouseButton::Middle => (CGMouseButton::Center, CGEventType::OtherMouseUp),
            MouseButton::Right => (CGMouseButton::Right, CGEventType::RightMouseUp),
            _ => unimplemented!(),
        };
        let dest = CGPoint::new(current_x as f64, current_y as f64);
        if let Some(src) = self.event_source.as_ref() {
            if let Ok(event) = CGEvent::new_mouse_event(src.clone(), event_type, dest, button) {
                if self.multiple_click > 1 {
                    event.set_integer_value_field(
                        EventField::MOUSE_EVENT_CLICK_STATE,
                        self.multiple_click,
                    );
                }
                self.post(event);
            }
        }
    }

    fn mouse_click(&mut self, button: MouseButton) {
        self.mouse_down(button).ok();
        self.mouse_up(button);
    }

    fn mouse_scroll_x(&mut self, length: i32) {
        let mut scroll_direction = -1; // 1 left -1 right;
        let mut length = length;

        if length < 0 {
            length *= -1;
            scroll_direction *= -1;
        }

        if let Some(src) = self.event_source.as_ref() {
            for _ in 0..length {
                unsafe {
                    let mouse_ev = CGEventCreateScrollWheelEvent(
                        &src,
                        ScrollUnit::Line,
                        2, // CGWheelCount 1 = y 2 = xy 3 = xyz
                        0,
                        scroll_direction,
                    );

                    CGEventPost(CGEventTapLocation::HID, mouse_ev);
                    CFRelease(mouse_ev as *const std::ffi::c_void);
                }
            }
        }
    }

    fn mouse_scroll_y(&mut self, length: i32) {
        let mut scroll_direction = -1; // 1 left -1 right;
        let mut length = length;

        if length < 0 {
            length *= -1;
            scroll_direction *= -1;
        }

        if let Some(src) = self.event_source.as_ref() {
            for _ in 0..length {
                unsafe {
                    let mouse_ev = CGEventCreateScrollWheelEvent(
                        &src,
                        ScrollUnit::Line,
                        1, // CGWheelCount 1 = y 2 = xy 3 = xyz
                        scroll_direction,
                    );

                    CGEventPost(CGEventTapLocation::HID, mouse_ev);
                    CFRelease(mouse_ev as *const std::ffi::c_void);
                }
            }
        }
    }
}

// https://stackoverflow.
// com/questions/1918841/how-to-convert-ascii-character-to-cgkeycode

impl KeyboardControllable for Enigo {
    fn key_sequence(&mut self, sequence: &str) {
        // NOTE(dustin): This is a fix for issue https://github.com/enigo-rs/enigo/issues/68
        // TODO(dustin): This could be improved by aggregating 20 bytes worth of graphemes at a time
        // but i am unsure what would happen for grapheme clusters greater than 20 bytes ...
        use unicode_segmentation::UnicodeSegmentation;
        let clusters = UnicodeSegmentation::graphemes(sequence, true).collect::<Vec<&str>>();
        for cluster in clusters {
            if let Some(src) = self.event_source.as_ref() {
                if let Ok(event) = CGEvent::new_keyboard_event(src.clone(), 0, true) {
                    event.set_string(cluster);
                    self.post(event);
                }
            }
        }
    }

    fn key_click(&mut self, key: Key) {
        let keycode = self.key_to_keycode(key);
        if keycode == 0 {
            return;
        }

        if let Some(src) = self.event_source.as_ref() {
            if let Ok(event) = CGEvent::new_keyboard_event(src.clone(), keycode, true) {
                self.post(event);
            }

            if let Ok(event) = CGEvent::new_keyboard_event(src.clone(), keycode, false) {
                self.post(event);
            }
        }
    }

    fn key_down(&mut self, key: Key) -> crate::ResultType {
        if let Some(src) = self.event_source.as_ref() {
            if let Ok(event) =
                CGEvent::new_keyboard_event(src.clone(), self.key_to_keycode(key), true)
            {
                self.post(event);
            }
        }
        Ok(())
    }

    fn key_up(&mut self, key: Key) {
        if let Some(src) = self.event_source.as_ref() {
            if let Ok(event) =
                CGEvent::new_keyboard_event(src.clone(), self.key_to_keycode(key), false)
            {
                self.post(event);
            }
        }
    }

    fn get_key_state(&mut self, key: Key) -> bool {
        let keycode = self.key_to_keycode(key);
        unsafe { CGEventSourceKeyState(1, keycode) }
    }
}

impl Enigo {
    fn pressed_buttons() -> usize {
        if let Some(ns_event) = Class::get("NSEvent") {
            unsafe { msg_send![ns_event, pressedMouseButtons] }
        } else {
            0
        }
    }

    /// Fetches the `(width, height)` in pixels of the main display
    pub fn main_display_size() -> (usize, usize) {
        let display_id = unsafe { CGMainDisplayID() };
        let width = unsafe { CGDisplayPixelsWide(display_id) };
        let height = unsafe { CGDisplayPixelsHigh(display_id) };
        (width, height)
    }

    /// Returns the current mouse location in Cocoa coordinates which have Y
    /// inverted from the Carbon coordinates used in the rest of the API.
    /// This function exists so that mouse_move_relative only has to fetch
    /// the screen size once.
    fn mouse_location_raw_coords() -> (i32, i32) {
        if let Some(ns_event) = Class::get("NSEvent") {
            let pt: NSPoint = unsafe { msg_send![ns_event, mouseLocation] };
            (pt.x as i32, pt.y as i32)
        } else {
            (0, 0)
        }
    }

    /// The mouse coordinates in points, only works on the main display
    pub fn mouse_location() -> (i32, i32) {
        let (x, y_inv) = Self::mouse_location_raw_coords();
        let (_, display_height) = Self::main_display_size();
        (x, (display_height as i32) - y_inv)
    }

    fn key_to_keycode(&mut self, key: Key) -> CGKeyCode {
        #[allow(deprecated)]
        // I mean duh, we still need to support deprecated keys until they're removed
        match key {
            Key::Alt => kVK_Option,
            Key::Backspace => kVK_Delete,
            Key::CapsLock => kVK_CapsLock,
            Key::Control => kVK_Control,
            Key::Delete => kVK_ForwardDelete,
            Key::DownArrow => kVK_DownArrow,
            Key::End => kVK_End,
            Key::Escape => kVK_Escape,
            Key::F1 => kVK_F1,
            Key::F10 => kVK_F10,
            Key::F11 => kVK_F11,
            Key::F12 => kVK_F12,
            Key::F2 => kVK_F2,
            Key::F3 => kVK_F3,
            Key::F4 => kVK_F4,
            Key::F5 => kVK_F5,
            Key::F6 => kVK_F6,
            Key::F7 => kVK_F7,
            Key::F8 => kVK_F8,
            Key::F9 => kVK_F9,
            Key::Home => kVK_Home,
            Key::LeftArrow => kVK_LeftArrow,
            Key::Option => kVK_Option,
            Key::PageDown => kVK_PageDown,
            Key::PageUp => kVK_PageUp,
            Key::Return => kVK_Return,
            Key::RightArrow => kVK_RightArrow,
            Key::Shift => kVK_Shift,
            Key::Space => kVK_Space,
            Key::Tab => kVK_Tab,
            Key::UpArrow => kVK_UpArrow,
            Key::Numpad0 => kVK_ANSI_Keypad0,
            Key::Numpad1 => kVK_ANSI_Keypad1,
            Key::Numpad2 => kVK_ANSI_Keypad2,
            Key::Numpad3 => kVK_ANSI_Keypad3,
            Key::Numpad4 => kVK_ANSI_Keypad4,
            Key::Numpad5 => kVK_ANSI_Keypad5,
            Key::Numpad6 => kVK_ANSI_Keypad6,
            Key::Numpad7 => kVK_ANSI_Keypad7,
            Key::Numpad8 => kVK_ANSI_Keypad8,
            Key::Numpad9 => kVK_ANSI_Keypad9,
            Key::Mute => kVK_Mute,
            Key::VolumeDown => kVK_VolumeUp,
            Key::VolumeUp => kVK_VolumeDown,
            Key::Help => kVK_Help,
            Key::Snapshot => kVK_F13,
            Key::Clear => kVK_ANSI_KeypadClear,
            Key::Decimal => kVK_ANSI_KeypadDecimal,
            Key::Multiply => kVK_ANSI_KeypadMultiply,
            Key::Add => kVK_ANSI_KeypadPlus,
            Key::Divide => kVK_ANSI_KeypadDivide,
            Key::NumpadEnter => kVK_ANSI_KeypadEnter,
            Key::Subtract => kVK_ANSI_KeypadMinus,
            Key::Equals => kVK_ANSI_KeypadEquals,
            Key::NumLock => kVK_ANSI_KeypadClear,
            Key::RWin => kVK_RIGHT_COMMAND,
            Key::RightShift => kVK_RightShift,
            Key::RightControl => kVK_RightControl,
            Key::RightAlt => kVK_RightOption,

            Key::Raw(raw_keycode) => raw_keycode,
            Key::Layout(c) => self.map_key_board(c),

            Key::Super | Key::Command | Key::Windows | Key::Meta => kVK_Command,
            _ => 0,
        }
    }

    #[inline]
    fn map_key_board(&self, ch: char) -> CGKeyCode {
        match ch {
            'a' => kVK_ANSI_A,
            'b' => kVK_ANSI_B,
            'c' => kVK_ANSI_C,
            'd' => kVK_ANSI_D,
            'e' => kVK_ANSI_E,
            'f' => kVK_ANSI_F,
            'g' => kVK_ANSI_G,
            'h' => kVK_ANSI_H,
            'i' => kVK_ANSI_I,
            'j' => kVK_ANSI_J,
            'k' => kVK_ANSI_K,
            'l' => kVK_ANSI_L,
            'm' => kVK_ANSI_M,
            'n' => kVK_ANSI_N,
            'o' => kVK_ANSI_O,
            'p' => kVK_ANSI_P,
            'q' => kVK_ANSI_Q,
            'r' => kVK_ANSI_R,
            's' => kVK_ANSI_S,
            't' => kVK_ANSI_T,
            'u' => kVK_ANSI_U,
            'v' => kVK_ANSI_V,
            'w' => kVK_ANSI_W,
            'x' => kVK_ANSI_X,
            'y' => kVK_ANSI_Y,
            'z' => kVK_ANSI_Z,
            '0' => kVK_ANSI_0,
            '1' => kVK_ANSI_1,
            '2' => kVK_ANSI_2,
            '3' => kVK_ANSI_3,
            '4' => kVK_ANSI_4,
            '5' => kVK_ANSI_5,
            '6' => kVK_ANSI_6,
            '7' => kVK_ANSI_7,
            '8' => kVK_ANSI_8,
            '9' => kVK_ANSI_9,
            '-' => kVK_ANSI_Minus,
            '=' => kVK_ANSI_Equal,
            '[' => kVK_ANSI_LeftBracket,
            ']' => kVK_ANSI_RightBracket,
            '\\' => kVK_ANSI_Backslash,
            ';' => kVK_ANSI_Semicolon,
            '\'' => kVK_ANSI_Quote,
            ',' => kVK_ANSI_Comma,
            '.' => kVK_ANSI_Period,
            '/' => kVK_ANSI_Slash,
            '`' => kVK_ANSI_Grave,
            _ => 0,
        }
    }
}

unsafe impl Send for Enigo {}
