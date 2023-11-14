use super::*;
#[cfg(target_os = "macos")]
use dispatch::Queue;
use enigo::{Enigo, Key, KeyboardControllable, MouseButton, MouseControllable};
use hbb_common::{config::COMPRESS_LEVEL, protobuf::ProtobufEnumOrUnknown};
use std::{
    convert::TryFrom,
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

#[derive(Default)]
struct StateCursor {
    hcursor: u64,
    cursor_data: Arc<Message>,
    cached_cursor_data: HashMap<u64, Arc<Message>>,
}

impl super::service::Reset for StateCursor {
    fn reset(&mut self) {
        *self = Default::default();
        crate::platform::reset_input_cache();
    }
}

#[derive(Default)]
struct StatePos {
    cursor_pos: (i32, i32),
}

impl super::service::Reset for StatePos {
    fn reset(&mut self) {
        self.cursor_pos = (0, 0);
    }
}

#[derive(Default)]
struct Input {
    conn: i32,
    time: i64,
}

const KEY_CHAR_START: i32 = 9999;

#[derive(Clone, Default)]
pub struct MouseCursorSub {
    inner: ConnInner,
    cached: HashMap<u64, Arc<Message>>,
}

impl From<ConnInner> for MouseCursorSub {
    fn from(inner: ConnInner) -> Self {
        Self {
            inner,
            cached: HashMap::new(),
        }
    }
}

impl Subscriber for MouseCursorSub {
    #[inline]
    fn id(&self) -> i32 {
        self.inner.id()
    }

    #[inline]
    fn send(&mut self, msg: Arc<Message>) {
        if let Some(message::Union::cursor_data(cd)) = &msg.union {
            if let Some(msg) = self.cached.get(&cd.id) {
                self.inner.send(msg.clone());
            } else {
                self.inner.send(msg.clone());
                let mut tmp = Message::new();
                // only send id out, require client side cache also
                tmp.set_cursor_id(cd.id);
                self.cached.insert(cd.id, Arc::new(tmp));
            }
        } else {
            self.inner.send(msg);
        }
    }
}

pub const NAME_CURSOR: &'static str = "mouse_cursor";
pub const NAME_POS: &'static str = "mouse_pos";
pub type MouseCursorService = ServiceTmpl<MouseCursorSub>;

pub fn new_cursor() -> MouseCursorService {
    let sp = MouseCursorService::new(NAME_CURSOR, true);
    sp.repeat::<StateCursor, _>(33, run_cursor);
    sp
}

pub fn new_pos() -> GenericService {
    let sp = GenericService::new(NAME_POS, false);
    sp.repeat::<StatePos, _>(33, run_pos);
    sp
}

fn run_pos(sp: GenericService, state: &mut StatePos) -> ResultType<()> {
    if let Some((x, y)) = crate::platform::get_cursor_pos() {
        if state.cursor_pos.0 != x || state.cursor_pos.1 != y {
            state.cursor_pos = (x, y);
            let mut msg_out = Message::new();
            msg_out.set_cursor_position(CursorPosition {
                x,
                y,
                ..Default::default()
            });
            let exclude = {
                let now = crate::common::get_time();
                let lock = LATEST_INPUT.lock().unwrap();
                if now - lock.time < 300 {
                    lock.conn
                } else {
                    0
                }
            };
            sp.send_without(msg_out, exclude);
        }
    }

    sp.snapshot(|sps| {
        let mut msg_out = Message::new();
        msg_out.set_cursor_position(CursorPosition {
            x: state.cursor_pos.0,
            y: state.cursor_pos.1,
            ..Default::default()
        });
        sps.send(msg_out);
        Ok(())
    })?;
    Ok(())
}

fn run_cursor(sp: MouseCursorService, state: &mut StateCursor) -> ResultType<()> {
    if let Some(hcursor) = crate::platform::get_cursor()? {
        if hcursor != state.hcursor {
            let msg;
            if let Some(cached) = state.cached_cursor_data.get(&hcursor) {
                //super::log::trace!("Cursor data cached, hcursor: {}", hcursor);
                msg = cached.clone();
            } else {
                let mut data = crate::platform::get_cursor_data(hcursor)?;
                data.colors = hbb_common::compress::compress(&data.colors[..], COMPRESS_LEVEL);
                let mut tmp = Message::new();
                tmp.set_cursor_data(data);
                msg = Arc::new(tmp);
                state.cached_cursor_data.insert(hcursor, msg.clone());
                //super::log::trace!("Cursor data updated, hcursor: {}", hcursor);
            }
            state.hcursor = hcursor;
            sp.send_shared(msg.clone());
            state.cursor_data = msg;
        }
    }
    sp.snapshot(|sps| {
        sps.send_shared(state.cursor_data.clone());
        Ok(())
    })?;
    Ok(())
}

lazy_static::lazy_static! {
    static ref ENIGO: Arc<Mutex<Enigo>> = Arc::new(Mutex::new(Enigo::new()));
    static ref KEYS_DOWN: Arc<Mutex<HashMap<i32, Instant>>> = Default::default();
    static ref LATEST_INPUT: Arc<Mutex<Input>> = Default::default();
}
static EXITING: AtomicBool = AtomicBool::new(false);


#[cfg(windows)]
pub fn mouse_move_relative(x: i32, y: i32) {
    crate::platform::windows::try_change_desktop();
    let mut en = ENIGO.lock().unwrap();
    en.mouse_move_relative(x, y);
}

#[cfg(not(target_os = "macos"))]
fn modifier_sleep() {
    // sleep for a while, this is only for keying in rdp in peer so far
    #[cfg(windows)]
    std::thread::sleep(std::time::Duration::from_nanos(1));
}

#[cfg(not(target_os = "macos"))]
#[inline]
fn get_modifier_state(key: Key, en: &mut Enigo) -> bool {
    let x = en.get_key_state(key.clone());
    match key {
        Key::Shift => x || en.get_key_state(Key::RightShift),
        Key::Control => x || en.get_key_state(Key::RightControl),
        Key::Alt => x || en.get_key_state(Key::RightAlt),
        Key::Meta => x || en.get_key_state(Key::RWin),
        _ => x,
    }
}

pub fn handle_mouse(evt: &MouseEvent, conn: i32) {
    #[cfg(target_os = "macos")]
    if !*IS_SERVER {
        // having GUI, run main GUI thread, otherwise crash
        let evt = evt.clone();
        QUEUE.exec_async(move || handle_mouse_(&evt, conn));
        return;
    }
    handle_mouse_(evt, conn);
}


pub fn fix_key_down_timeout_at_exit() {
    if EXITING.load(Ordering::SeqCst) {
        return;
    }
    EXITING.store(true, Ordering::SeqCst);
    fix_key_down_timeout(true);
    //log::info!("fix_key_down_timeout_at_exit");
}

fn fix_key_down_timeout(force: bool) {
    if KEYS_DOWN.lock().unwrap().is_empty() {
        return;
    }
    let cloned = (*KEYS_DOWN.lock().unwrap()).clone();
    //log::debug!("{} keys in key down timeout map", cloned.len());
    for (key, value) in cloned.into_iter() {
        if force || value.elapsed().as_millis() >= 3_000 {
            KEYS_DOWN.lock().unwrap().remove(&key);
            let key = if key < KEY_CHAR_START {
                if let Some(key) = KEY_MAP.get(&key) {
                    Some(*key)
                } else {
                    None
                }
            } else {
                Some(Key::Layout(((key - KEY_CHAR_START) as u8) as _))
            };
            if let Some(key) = key {
                let func = move || {
                    let mut en = ENIGO.lock().unwrap();
                    if en.get_key_state(key) {
                        en.key_up(key);
                        //log::debug!("Fixed {:?} timeout", key);
                    }
                };
                #[cfg(target_os = "macos")]
                QUEUE.exec_async(func);
                #[cfg(not(target_os = "macos"))]
                func();
            }
        }
    }
}

// e.g. current state of ctrl is down, but ctrl not in modifier, we should change ctrl to up, to make modifier state sync between remote and local
#[inline]
fn fix_modifier(
    modifiers: &[ProtobufEnumOrUnknown<ControlKey>],
    key0: ControlKey,
    key1: Key,
    en: &mut Enigo,
) {
    if en.get_key_state(key1) && !modifiers.contains(&ProtobufEnumOrUnknown::new(key0)) {
        en.key_up(key1);
        //log::debug!("Fixed {:?}", key1);
    }
}

fn fix_modifiers(modifiers: &[ProtobufEnumOrUnknown<ControlKey>], en: &mut Enigo, ck: i32) {
    if ck != ControlKey::Shift.value() {
        fix_modifier(modifiers, ControlKey::Shift, Key::Shift, en);
    }
    if ck != ControlKey::RShift.value() {
        fix_modifier(modifiers, ControlKey::Shift, Key::RightShift, en);
    }
    if ck != ControlKey::Alt.value() {
        fix_modifier(modifiers, ControlKey::Alt, Key::Alt, en);
    }
    if ck != ControlKey::RAlt.value() {
        fix_modifier(modifiers, ControlKey::Alt, Key::RightAlt, en);
    }
    if ck != ControlKey::Control.value() {
        fix_modifier(modifiers, ControlKey::Control, Key::Control, en);
    }
    if ck != ControlKey::RControl.value() {
        fix_modifier(modifiers, ControlKey::Control, Key::RightControl, en);
    }
    if ck != ControlKey::Meta.value() {
        fix_modifier(modifiers, ControlKey::Meta, Key::Meta, en);
    }
    if ck != ControlKey::RWin.value() {
        fix_modifier(modifiers, ControlKey::Meta, Key::RWin, en);
    }
}

fn handle_mouse_(evt: &MouseEvent, conn: i32) {
    if EXITING.load(Ordering::SeqCst) {
        return;
    }
    #[cfg(windows)]
    crate::platform::windows::try_change_desktop();
    let buttons = evt.mask >> 3;
    let evt_type = evt.mask & 0x7;
    if evt_type == 0 {
        let time = crate::common::get_time();
        *LATEST_INPUT.lock().unwrap() = Input { time, conn };
    }
    let mut en = ENIGO.lock().unwrap();
    #[cfg(not(target_os = "macos"))]
    let mut to_release = Vec::new();
    fix_modifiers(&evt.modifiers[..], &mut en, 0);
    if evt_type == 1 {
        #[cfg(target_os = "macos")]
        en.reset_flag();
        for ref ck in evt.modifiers.iter() {
            if let Some(key) = KEY_MAP.get(&ck.value()) {
                #[cfg(target_os = "macos")]
                en.add_flag(key);
                #[cfg(not(target_os = "macos"))]
                if key != &Key::CapsLock && key != &Key::NumLock {
                    if !get_modifier_state(key.clone(), &mut en) {
                        en.key_down(key.clone()).ok();
                        modifier_sleep();
                        to_release.push(key);
                    } else {
                        KEYS_DOWN.lock().unwrap().insert(ck.value(), Instant::now());
                    }
                }
            }
        }
    }
    match evt_type {
        0 => {
            en.mouse_move_to(evt.x, evt.y);
        }
        1 => match buttons {
            1 => {
                allow_err!(en.mouse_down(MouseButton::Left));
            }
            2 => {
                allow_err!(en.mouse_down(MouseButton::Right));
            }
            4 => {
                allow_err!(en.mouse_down(MouseButton::Middle));
            }
            _ => {}
        },
        2 => match buttons {
            1 => {
                en.mouse_up(MouseButton::Left);
            }
            2 => {
                en.mouse_up(MouseButton::Right);
            }
            4 => {
                en.mouse_up(MouseButton::Middle);
            }
            _ => {}
        },
        3 => {
            #[allow(unused_mut)]
            let mut x = evt.x;
            #[allow(unused_mut)]
            let mut y = evt.y;
            #[cfg(not(windows))]
            {
                x = -x;
                y = -y;
            }
            if x != 0 {
                en.mouse_scroll_x(x);
            }
            if y != 0 {
                en.mouse_scroll_y(y);
            }
        }
        _ => {}
    }
    #[cfg(not(target_os = "macos"))]
    for key in to_release {
        en.key_up(key.clone());
    }
}


lazy_static::lazy_static! {
    static ref KEY_MAP: HashMap<i32, Key> =
    [
        (ControlKey::Alt, Key::Alt),
        (ControlKey::Backspace, Key::Backspace),
        (ControlKey::CapsLock, Key::CapsLock),
        (ControlKey::Control, Key::Control),
        (ControlKey::Delete, Key::Delete),
        (ControlKey::DownArrow, Key::DownArrow),
        (ControlKey::End, Key::End),
        (ControlKey::Escape, Key::Escape),
        (ControlKey::F1, Key::F1),
        (ControlKey::F10, Key::F10),
        (ControlKey::F11, Key::F11),
        (ControlKey::F12, Key::F12),
        (ControlKey::F2, Key::F2),
        (ControlKey::F3, Key::F3),
        (ControlKey::F4, Key::F4),
        (ControlKey::F5, Key::F5),
        (ControlKey::F6, Key::F6),
        (ControlKey::F7, Key::F7),
        (ControlKey::F8, Key::F8),
        (ControlKey::F9, Key::F9),
        (ControlKey::Home, Key::Home),
        (ControlKey::LeftArrow, Key::LeftArrow),
        (ControlKey::Meta, Key::Meta),
        (ControlKey::Option, Key::Option),
        (ControlKey::PageDown, Key::PageDown),
        (ControlKey::PageUp, Key::PageUp),
        (ControlKey::Return, Key::Return),
        (ControlKey::RightArrow, Key::RightArrow),
        (ControlKey::Shift, Key::Shift),
        (ControlKey::Space, Key::Space),
        (ControlKey::Tab, Key::Tab),
        (ControlKey::UpArrow, Key::UpArrow),
        (ControlKey::Numpad0, Key::Numpad0),
        (ControlKey::Numpad1, Key::Numpad1),
        (ControlKey::Numpad2, Key::Numpad2),
        (ControlKey::Numpad3, Key::Numpad3),
        (ControlKey::Numpad4, Key::Numpad4),
        (ControlKey::Numpad5, Key::Numpad5),
        (ControlKey::Numpad6, Key::Numpad6),
        (ControlKey::Numpad7, Key::Numpad7),
        (ControlKey::Numpad8, Key::Numpad8),
        (ControlKey::Numpad9, Key::Numpad9),
        (ControlKey::Cancel, Key::Cancel),
        (ControlKey::Clear, Key::Clear),
        (ControlKey::Menu, Key::Alt),
        (ControlKey::Pause, Key::Pause),
        (ControlKey::Kana, Key::Kana),
        (ControlKey::Hangul, Key::Hangul),
        (ControlKey::Junja, Key::Junja),
        (ControlKey::Final, Key::Final),
        (ControlKey::Hanja, Key::Hanja),
        (ControlKey::Kanji, Key::Kanji),
        (ControlKey::Convert, Key::Convert),
        (ControlKey::Select, Key::Select),
        (ControlKey::Print, Key::Print),
        (ControlKey::Execute, Key::Execute),
        (ControlKey::Snapshot, Key::Snapshot),
        (ControlKey::Insert, Key::Insert),
        (ControlKey::Help, Key::Help),
        (ControlKey::Sleep, Key::Sleep),
        (ControlKey::Separator, Key::Separator),
        (ControlKey::Scroll, Key::Scroll),
        (ControlKey::NumLock, Key::NumLock),
        (ControlKey::RWin, Key::RWin),
        (ControlKey::Apps, Key::Apps),
        (ControlKey::Multiply, Key::Multiply),
        (ControlKey::Add, Key::Add),
        (ControlKey::Subtract, Key::Subtract),
        (ControlKey::Decimal, Key::Decimal),
        (ControlKey::Divide, Key::Divide),
        (ControlKey::Equals, Key::Equals),
        (ControlKey::NumpadEnter, Key::NumpadEnter),
        (ControlKey::RAlt, Key::RightAlt),
        (ControlKey::RWin, Key::RWin),
        (ControlKey::RControl, Key::RightControl),
        (ControlKey::RShift, Key::RightShift),
    ].iter().map(|(a, b)| (a.value(), b.clone())).collect();
    static ref NUMPAD_KEY_MAP: HashMap<i32, bool> =
    [
        (ControlKey::Home, true),
        (ControlKey::UpArrow, true),
        (ControlKey::PageUp, true),
        (ControlKey::LeftArrow, true),
        (ControlKey::RightArrow, true),
        (ControlKey::End, true),
        (ControlKey::DownArrow, true),
        (ControlKey::PageDown, true),
        (ControlKey::Insert, true),
        (ControlKey::Delete, true),
    ].iter().map(|(a, b)| (a.value(), b.clone())).collect();
}

pub fn handle_key(evt: &KeyEvent) {
    #[cfg(target_os = "macos")]
    if !*IS_SERVER {
        // having GUI, run main GUI thread, otherwise crash
        let evt = evt.clone();
        QUEUE.exec_async(move || handle_key_(&evt));
        return;
    }
    handle_key_(evt);
}

fn handle_key_(evt: &KeyEvent) {
    if EXITING.load(Ordering::SeqCst) {
        return;
    }
    #[cfg(windows)]
    crate::platform::windows::try_change_desktop();
    let mut en = ENIGO.lock().unwrap();
    // disable numlock if press home etc when numlock is on,
    // because we will get numpad value (7,8,9 etc) if not
    #[cfg(windows)]
    let mut disable_numlock = false;
    #[cfg(target_os = "macos")]
    en.reset_flag();
    #[cfg(not(target_os = "macos"))]
    let mut to_release = Vec::new();
    #[cfg(not(target_os = "macos"))]
    let mut has_cap = false;
    #[cfg(windows)]
    let mut has_numlock = false;
    if evt.down {
        let ck = if let Some(key_event::Union::control_key(ck)) = evt.union {
            ck.value()
        } else {
            -1
        };
        fix_modifiers(&evt.modifiers[..], &mut en, ck);
        for ref ck in evt.modifiers.iter() {
            if let Some(key) = KEY_MAP.get(&ck.value()) {
                #[cfg(target_os = "macos")]
                en.add_flag(key);
                #[cfg(not(target_os = "macos"))]
                {
                    if key == &Key::CapsLock {
                        has_cap = true;
                    } else if key == &Key::NumLock {
                        #[cfg(windows)]
                        {
                            has_numlock = true;
                        }
                    } else {
                        if !get_modifier_state(key.clone(), &mut en) {
                            en.key_down(key.clone()).ok();
                            modifier_sleep();
                            to_release.push(key);
                        } else {
                            KEYS_DOWN.lock().unwrap().insert(ck.value(), Instant::now());
                        }
                    }
                }
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    if crate::common::valid_for_capslock(evt) {
        if has_cap != en.get_key_state(Key::CapsLock) {
            en.key_down(Key::CapsLock).ok();
            en.key_up(Key::CapsLock);
        }
    }
    #[cfg(windows)]
    if crate::common::valid_for_numlock(evt) {
        if has_numlock != en.get_key_state(Key::NumLock) {
            en.key_down(Key::NumLock).ok();
            en.key_up(Key::NumLock);
        }
    }
    match evt.union {
        Some(key_event::Union::control_key(ck)) => {
            if let Some(key) = KEY_MAP.get(&ck.value()) {
                #[cfg(windows)]
                if let Some(_) = NUMPAD_KEY_MAP.get(&ck.value()) {
                    disable_numlock = en.get_key_state(Key::NumLock);
                    if disable_numlock {
                        en.key_down(Key::NumLock).ok();
                        en.key_up(Key::NumLock);
                    }
                }
                if evt.down {
                    allow_err!(en.key_down(key.clone()));
                    KEYS_DOWN.lock().unwrap().insert(ck.value(), Instant::now());
                } else {
                    en.key_up(key.clone());
                    KEYS_DOWN.lock().unwrap().remove(&ck.value());
                }
            } else if ck.value() == ControlKey::CtrlAltDel.value() {
                // have to spawn new thread because send_sas is tokio_main, the caller can not be tokio_main.
                std::thread::spawn(|| {
                    allow_err!(send_sas());
                });
            } else if ck.value() == ControlKey::LockScreen.value() {
                crate::platform::lock_screen();
                super::video_service::switch_to_primary();
            }
        }
        Some(key_event::Union::chr(chr)) => {
            if evt.down {
                allow_err!(en.key_down(Key::Layout(chr as u8 as _)));
                KEYS_DOWN
                    .lock()
                    .unwrap()
                    .insert(chr as i32 + KEY_CHAR_START, Instant::now());
            } else {
                en.key_up(Key::Layout(chr as u8 as _));
                KEYS_DOWN
                    .lock()
                    .unwrap()
                    .remove(&(chr as i32 + KEY_CHAR_START));
            }
        }
        Some(key_event::Union::unicode(chr)) => {
            if let Ok(chr) = char::try_from(chr) {
                en.key_sequence(&chr.to_string());
            }
        }
        Some(key_event::Union::seq(ref seq)) => {
            en.key_sequence(&seq);
        }
        _ => {}
    }
    #[cfg(not(target_os = "macos"))]
    for key in to_release {
        en.key_up(key.clone());
    }
    #[cfg(windows)]
    if disable_numlock {
        en.key_down(Key::NumLock).ok();
        en.key_up(Key::NumLock);
    }
}

#[tokio::main(flavor = "current_thread")]
async fn send_sas() -> ResultType<()> {
    let mut stream = crate::ipc::connect(1000, crate::common::POSTFIX_SERVICE).await?;
    timeout(1000, stream.send(&crate::ipc::Data::SAS)).await??;
    Ok(())
}
