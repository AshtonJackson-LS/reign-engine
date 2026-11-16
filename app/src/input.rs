//! Input actions: named buttons a project binds to keys, mouse buttons and
//! gamepad buttons, in `assets/input.ron`.
//!
//! The simulation never sees a key or a pad, only which actions are held
//! each tick (an [`InputFrame`]), so replays and rollback work the same
//! whatever the bindings are. There are up to [`MAX_ACTIONS`] actions; the
//! first eight default to the movement and face buttons that
//! [`Button`](reign::core::Button) names, so `ctx.held(Button::Jump)` and
//! `ctx.action("Jump")` mean the same thing until a project renames them.
//!
//! The file is an ordinary asset: it hot-reloads in the editor and is
//! packed into exported games.

pub mod dialog;

use std::sync::{Arc, Mutex, RwLock};

use reign::app::gilrs::Button as Pad;
use reign::app::winit::event::MouseButton;
use reign::app::winit::keyboard::KeyCode;
use reign::app::{PadInput, Pads};
use reign::core::{Button, InputFrame};
use serde::{Deserialize, Serialize};

/// How many actions a project can have: one per bit the simulation's
/// input carries.
pub const MAX_ACTIONS: usize = Button::ALL.len();
/// The bindings file, at the root of the project's assets.
pub const INPUT_FILE: &str = "input.ron";

/// A project's actions, in slot order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct InputMap {
    pub actions: Vec<Action>,
}

/// One action and what triggers it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Action {
    pub name: String,
    /// Keyboard keys, by their names on a US layout: `"Space"`, `"KeyA"` or
    /// just `"A"`, `"ArrowLeft"`, `"ShiftLeft"`, `"Digit1"` or `"1"`, `"F5"`.
    pub keys: Vec<String>,
    /// Mouse buttons: `"Left"`, `"Right"`, `"Middle"`, `"Back"`,
    /// `"Forward"`.
    pub mouse: Vec<String>,
    /// Gamepad buttons: `"South"` (A on Xbox, Cross on PlayStation),
    /// `"East"`, `"West"`, `"North"`, `"LeftTrigger"`, `"RightTrigger"`,
    /// `"LeftTrigger2"`, `"RightTrigger2"`, `"Start"`, `"Select"`,
    /// `"DPadUp"` and so on, and `"LeftStickLeft"`/`Right`/`Up`/`Down`.
    pub pad: Vec<String>,
}

impl Default for InputMap {
    fn default() -> Self {
        let action = |name: &str, keys: &[&str], pad: &[&str]| Action {
            name: name.to_owned(),
            keys: keys.iter().map(|k| k.to_string()).collect(),
            mouse: Vec::new(),
            pad: pad.iter().map(|p| p.to_string()).collect(),
        };
        InputMap {
            actions: vec![
                action("Left", &["ArrowLeft", "KeyA"], &["DPadLeft", "LeftStickLeft"]),
                action("Right", &["ArrowRight", "KeyD"], &["DPadRight", "LeftStickRight"]),
                action("Up", &["ArrowUp", "KeyW"], &["DPadUp", "LeftStickUp"]),
                action("Down", &["ArrowDown", "KeyS"], &["DPadDown", "LeftStickDown"]),
                action("Jump", &["Space"], &["South"]),
                action("Attack", &["KeyJ", "KeyZ"], &["West"]),
                action("Dash", &["KeyK", "KeyX", "ShiftLeft"], &["East"]),
                action("Parry", &["KeyL", "KeyC"], &["North"]),
                action("Interact", &["KeyE"], &["RightTrigger"]),
                action("Pause", &["Escape", "KeyP"], &["Start"]),
            ],
        }
    }
}

impl Action {
    /// Every binding, as the editor shows it: `"Space"`, `"Mouse Left"`,
    /// `"Pad South"`.
    pub fn labels(&self) -> Vec<String> {
        let keys = self.keys.iter().cloned();
        let mouse = self.mouse.iter().map(|m| format!("Mouse {m}"));
        let pad = self.pad.iter().map(|p| format!("Pad {p}"));
        keys.chain(mouse).chain(pad).collect()
    }
}

/// The project's map as its file says, for showing it: the defaults when
/// there's no file, or why the file can't be read.
pub fn read(file: &std::path::Path) -> Result<InputMap, String> {
    match std::fs::read_to_string(file) {
        Ok(text) => InputMap::parse(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(InputMap::default()),
        Err(e) => Err(e.to_string()),
    }
}

impl InputMap {
    pub fn parse(text: &str) -> Result<InputMap, String> {
        let map: InputMap = ron::from_str(text).map_err(|e| e.to_string())?;
        if map.actions.len() > MAX_ACTIONS {
            return Err(format!("{} actions; the most is {MAX_ACTIONS}", map.actions.len()));
        }
        Ok(map)
    }

    /// The file's text, with a comment saying how to edit it.
    pub fn to_text(&self) -> String {
        let pretty = ron::ser::PrettyConfig::new().depth_limit(3).struct_names(false);
        let body = ron::ser::to_string_pretty(self, pretty).expect("input maps serialize");
        format!(
            "// Input actions: scripts read them with ctx.action(\"Jump\"),\n\
             // ctx.action_pressed(\"Jump\") and so on. Up to {MAX_ACTIONS}, in order;\n\
             // the first eight are also Button::Left, Right, Up, Down, Jump,\n\
             // Attack, Dash and Parry. Edit here or in Scripts > Input Actions.\n\
             {body}\n"
        )
    }

    /// The slot an action is in.
    pub fn index(&self, name: &str) -> Option<usize> {
        self.actions.iter().position(|a| a.name == name)
    }

    /// Names in this map that aren't keys, mouse buttons or pad buttons it
    /// knows.
    pub fn unknown_bindings(&self) -> Vec<String> {
        let mut out = Vec::new();
        for a in &self.actions {
            out.extend(a.keys.iter().filter(|k| key_code(k).is_none()).cloned());
            out.extend(a.mouse.iter().filter(|m| mouse_button(m).is_none()).cloned());
            out.extend(a.pad.iter().filter(|p| pad_input(p).is_none()).cloned());
        }
        out
    }
}

/// The map in use: the project's `input.ron`, reloaded when it changes,
/// or the defaults.
pub fn current() -> Arc<InputMap> {
    type Cache = Option<(Option<u32>, Arc<InputMap>)>;
    static CACHE: RwLock<Cache> = RwLock::new(None);
    let db = crate::DB.get();
    let guid = db.and_then(|db| db.guid(INPUT_FILE));
    let version = db.zip(guid).map(|(db, g)| db.version(g));
    if let Some((v, map)) = CACHE.read().expect("input cache").as_ref()
        && *v == version
    {
        return map.clone();
    }
    let map = match db.zip(guid) {
        Some((db, guid)) => {
            match db.bytes(guid).map(|b| String::from_utf8_lossy(&b).into_owned()) {
                Ok(text) => InputMap::parse(&text).unwrap_or_else(|e| {
                    log::error!("{INPUT_FILE}: {e}; using the default actions");
                    InputMap::default()
                }),
                Err(e) => {
                    log::error!("{INPUT_FILE}: {e}");
                    InputMap::default()
                }
            }
        }
        None => InputMap::default(),
    };
    let unknown = map.unknown_bindings();
    if !unknown.is_empty() {
        log::warn!("{INPUT_FILE}: unknown keys or buttons: {}", unknown.join(", "));
    }
    let map = Arc::new(map);
    *CACHE.write().expect("input cache") = Some((version, map.clone()));
    map
}

/// The simulation button for an action, by name. Warns once about names
/// that aren't in the map.
pub fn button(name: &str) -> Option<Button> {
    let found = current().index(name).map(|i| Button::ALL[i]);
    if found.is_none() {
        static WARNED: Mutex<Vec<String>> = Mutex::new(Vec::new());
        let mut warned = WARNED.lock().expect("warned list");
        if !warned.iter().any(|w| w == name) {
            log::warn!("no input action called {name:?}: add it in {INPUT_FILE}");
            warned.push(name.to_owned());
        }
    }
    found
}

/// The input frame for this tick: which actions the held keys, mouse
/// buttons and pads trigger.
pub fn frame(keys: &[KeyCode], mouse: &[MouseButton], pads: Option<&Pads>) -> InputFrame {
    let map = current();
    let direction = pads.map_or((0, 0), Pads::direction);
    let mut frame = InputFrame::default();
    for (action, button) in map.actions.iter().zip(Button::ALL) {
        let key = action.keys.iter().filter_map(|k| key_code(k)).any(|k| keys.contains(&k));
        let click = action.mouse.iter().filter_map(|m| mouse_button(m)).any(|m| mouse.contains(&m));
        let pad = pads.is_some_and(|pads| {
            action.pad.iter().filter_map(|p| pad_input(p)).any(|input| match input {
                PadInput::Button(b) => pads.any_pressed(b),
                PadInput::Stick(dx, dy) => {
                    (dx != 0 && direction.0 == dx) || (dy != 0 && direction.1 == dy)
                }
            })
        });
        if key || click || pad {
            frame.set(button, true);
        }
    }
    frame
}

/// The pad buttons and stick directions a binding can name, by their
/// names in `input.ron`. A stick direction also counts the d-pad.
const PAD_NAMES: [(&str, PadInput); 21] = [
    ("South", PadInput::Button(Pad::South)),
    ("East", PadInput::Button(Pad::East)),
    ("West", PadInput::Button(Pad::West)),
    ("North", PadInput::Button(Pad::North)),
    ("LeftTrigger", PadInput::Button(Pad::LeftTrigger)),
    ("LeftTrigger2", PadInput::Button(Pad::LeftTrigger2)),
    ("RightTrigger", PadInput::Button(Pad::RightTrigger)),
    ("RightTrigger2", PadInput::Button(Pad::RightTrigger2)),
    ("Select", PadInput::Button(Pad::Select)),
    ("Start", PadInput::Button(Pad::Start)),
    ("Mode", PadInput::Button(Pad::Mode)),
    ("LeftThumb", PadInput::Button(Pad::LeftThumb)),
    ("RightThumb", PadInput::Button(Pad::RightThumb)),
    ("DPadUp", PadInput::Button(Pad::DPadUp)),
    ("DPadDown", PadInput::Button(Pad::DPadDown)),
    ("DPadLeft", PadInput::Button(Pad::DPadLeft)),
    ("DPadRight", PadInput::Button(Pad::DPadRight)),
    ("LeftStickLeft", PadInput::Stick(-1, 0)),
    ("LeftStickRight", PadInput::Stick(1, 0)),
    ("LeftStickUp", PadInput::Stick(0, -1)),
    ("LeftStickDown", PadInput::Stick(0, 1)),
];

/// Every pad name a binding can use, for menus.
pub fn pad_names() -> impl Iterator<Item = &'static str> {
    PAD_NAMES.iter().map(|(name, _)| *name)
}

/// The name `input.ron` uses for something a pad did, if a binding can
/// name it.
pub fn pad_name(input: PadInput) -> Option<&'static str> {
    PAD_NAMES.iter().find(|(_, i)| *i == input).map(|(name, _)| *name)
}

/// The mouse buttons a binding can name.
pub const MOUSE_NAMES: [&str; 5] = ["Left", "Right", "Middle", "Back", "Forward"];

/// A mouse button by name.
pub fn mouse_button(name: &str) -> Option<MouseButton> {
    Some(match name {
        "Left" => MouseButton::Left,
        "Right" => MouseButton::Right,
        "Middle" => MouseButton::Middle,
        "Back" => MouseButton::Back,
        "Forward" => MouseButton::Forward,
        _ => return None,
    })
}

/// The name a key goes by in `input.ron`, if a binding can name it.
pub fn key_name(key: KeyCode) -> Option<String> {
    let name = format!("{key:?}");
    (key_code(&name) == Some(key)).then_some(name)
}

/// A pad button or stick direction by name, with the Xbox and PlayStation
/// names as well.
pub fn pad_input(name: &str) -> Option<PadInput> {
    let button = match name {
        "South" | "A" | "Cross" => Pad::South,
        "East" | "B" | "Circle" => Pad::East,
        "West" | "X" | "Square" => Pad::West,
        "North" | "Y" | "Triangle" => Pad::North,
        "LeftTrigger" | "LB" | "L1" => Pad::LeftTrigger,
        "LeftTrigger2" | "LT" | "L2" => Pad::LeftTrigger2,
        "RightTrigger" | "RB" | "R1" => Pad::RightTrigger,
        "RightTrigger2" | "RT" | "R2" => Pad::RightTrigger2,
        "Select" | "Back" => Pad::Select,
        "Start" => Pad::Start,
        "Mode" => Pad::Mode,
        "LeftThumb" | "L3" => Pad::LeftThumb,
        "RightThumb" | "R3" => Pad::RightThumb,
        "DPadUp" => Pad::DPadUp,
        "DPadDown" => Pad::DPadDown,
        "DPadLeft" => Pad::DPadLeft,
        "DPadRight" => Pad::DPadRight,
        "LeftStickLeft" => return Some(PadInput::Stick(-1, 0)),
        "LeftStickRight" => return Some(PadInput::Stick(1, 0)),
        "LeftStickUp" => return Some(PadInput::Stick(0, -1)),
        "LeftStickDown" => return Some(PadInput::Stick(0, 1)),
        _ => return None,
    };
    Some(PadInput::Button(button))
}

/// A key by name: winit's names (`"KeyA"`, `"Digit1"`, `"ArrowLeft"`), or
/// a bare letter or digit.
pub fn key_code(name: &str) -> Option<KeyCode> {
    use KeyCode::*;
    const LETTERS: [KeyCode; 26] = [
        KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN, KeyO,
        KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ,
    ];
    const DIGITS: [KeyCode; 10] =
        [Digit0, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9];
    const F: [KeyCode; 12] = [F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12];
    let single = |s: &str| {
        let mut chars = s.chars();
        let c = chars.next()?;
        chars.next().is_none().then_some(c)
    };
    if let Some(c) = name.strip_prefix("Key").and_then(single).or_else(|| single(name))
        && c.is_ascii_alphabetic()
    {
        return Some(LETTERS[(c.to_ascii_uppercase() as u8 - b'A') as usize]);
    }
    if let Some(c) = name.strip_prefix("Digit").and_then(single).or_else(|| single(name))
        && let Some(d) = c.to_digit(10)
    {
        return Some(DIGITS[d as usize]);
    }
    if let Some(n) = name.strip_prefix('F').and_then(|n| n.parse::<usize>().ok())
        && (1..=12).contains(&n)
    {
        return Some(F[n - 1]);
    }
    Some(match name {
        "Space" => Space,
        "Enter" | "Return" => Enter,
        "Escape" | "Esc" => Escape,
        "Tab" => Tab,
        "Backspace" => Backspace,
        "ArrowLeft" | "Left" => ArrowLeft,
        "ArrowRight" | "Right" => ArrowRight,
        "ArrowUp" | "Up" => ArrowUp,
        "ArrowDown" | "Down" => ArrowDown,
        "ShiftLeft" | "Shift" => ShiftLeft,
        "ShiftRight" => ShiftRight,
        "ControlLeft" | "Control" | "Ctrl" => ControlLeft,
        "ControlRight" => ControlRight,
        "AltLeft" | "Alt" => AltLeft,
        "AltRight" => AltRight,
        "Comma" => Comma,
        "Period" => Period,
        "Slash" => Slash,
        "Semicolon" => Semicolon,
        "Quote" => Quote,
        "BracketLeft" => BracketLeft,
        "BracketRight" => BracketRight,
        "Minus" => Minus,
        "Equal" => Equal,
        "Backquote" => Backquote,
        "Backslash" => Backslash,
        "IntlBackslash" => IntlBackslash,
        "CapsLock" => CapsLock,
        "Insert" => Insert,
        "Delete" => Delete,
        "Home" => Home,
        "End" => End,
        "PageUp" => PageUp,
        "PageDown" => PageDown,
        "SuperLeft" => SuperLeft,
        "SuperRight" => SuperRight,
        "ContextMenu" => ContextMenu,
        "Numpad0" => Numpad0,
        "Numpad1" => Numpad1,
        "Numpad2" => Numpad2,
        "Numpad3" => Numpad3,
        "Numpad4" => Numpad4,
        "Numpad5" => Numpad5,
        "Numpad6" => Numpad6,
        "Numpad7" => Numpad7,
        "Numpad8" => Numpad8,
        "Numpad9" => Numpad9,
        "NumpadAdd" => NumpadAdd,
        "NumpadSubtract" => NumpadSubtract,
        "NumpadMultiply" => NumpadMultiply,
        "NumpadDivide" => NumpadDivide,
        "NumpadDecimal" => NumpadDecimal,
        "NumpadEnter" => NumpadEnter,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_map_round_trips_and_knows_every_binding() {
        let map = InputMap::default();
        assert_eq!(map.actions.len(), MAX_ACTIONS);
        assert!(map.unknown_bindings().is_empty(), "{:?}", map.unknown_bindings());
        assert_eq!(InputMap::parse(&map.to_text()).unwrap(), map);
        // The first eight line up with the engine's buttons.
        let names = ["Left", "Right", "Up", "Down", "Jump", "Attack", "Dash", "Parry"];
        for (i, name) in names.iter().enumerate() {
            assert_eq!(map.index(name), Some(i));
        }
    }

    #[test]
    fn keys_by_name() {
        assert_eq!(key_code("KeyA"), Some(KeyCode::KeyA));
        assert_eq!(key_code("q"), Some(KeyCode::KeyQ));
        assert_eq!(key_code("7"), Some(KeyCode::Digit7));
        assert_eq!(key_code("Digit0"), Some(KeyCode::Digit0));
        assert_eq!(key_code("F12"), Some(KeyCode::F12));
        assert_eq!(key_code("F13"), None);
        assert_eq!(key_code("Space"), Some(KeyCode::Space));
        assert_eq!(key_code("Nope"), None);
    }

    #[test]
    fn frames_follow_the_bindings() {
        let f = frame(&[KeyCode::KeyD, KeyCode::Space], &[], None);
        assert!(f.held(Button::Right) && f.held(Button::Jump));
        assert!(!f.held(Button::Left));
        assert_eq!(frame(&[KeyCode::KeyQ], &[], None), InputFrame::default());
    }

    #[test]
    fn heard_inputs_have_names_the_map_reads_back() {
        for key in [KeyCode::KeyA, KeyCode::Digit3, KeyCode::ShiftRight, KeyCode::Numpad4] {
            let name = key_name(key).unwrap();
            assert_eq!(key_code(&name), Some(key), "{name}");
        }
        assert_eq!(key_name(KeyCode::KeyA).as_deref(), Some("KeyA"));
        assert_eq!(key_name(KeyCode::F24), None, "not a key the map knows");
        for name in pad_names() {
            let input = pad_input(name).unwrap();
            assert_eq!(pad_name(input), Some(name));
        }
        assert_eq!(pad_name(PadInput::Button(Pad::Unknown)), None);
        for name in MOUSE_NAMES {
            assert!(mouse_button(name).is_some(), "{name}");
        }
    }

    #[test]
    fn mouse_bindings_load_and_old_files_still_do() {
        let mut map = InputMap::default();
        map.actions[5].mouse = vec!["Left".into()];
        assert_eq!(InputMap::parse(&map.to_text()).unwrap().actions[5].mouse, ["Left"]);
        let old = r#"(actions: [(name: "Jump", keys: ["Space"], pad: ["South"])])"#;
        assert!(InputMap::parse(old).unwrap().actions[0].mouse.is_empty());
        map.actions[5].mouse.push("Thumb".into());
        assert_eq!(map.unknown_bindings(), ["Thumb"]);
    }

    #[test]
    fn labels_name_the_device() {
        let jump = &InputMap::default().actions[4];
        assert_eq!(jump.labels(), ["Space", "Pad South"]);
        let click = Action { mouse: vec!["Left".into()], ..Action::default() };
        assert_eq!(click.labels(), ["Mouse Left"]);
    }

    #[test]
    fn reading_a_missing_file_gives_the_defaults() {
        let file = std::env::temp_dir().join("reign-input-not-here.ron");
        assert_eq!(read(&file).unwrap(), InputMap::default());
    }

    #[test]
    fn too_many_actions_is_an_error() {
        let mut map = InputMap::default();
        map.actions.push(Action { name: "Eleventh".into(), ..Action::default() });
        assert!(InputMap::parse(&map.to_text()).is_err());
    }
}
