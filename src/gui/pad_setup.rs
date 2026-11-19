use std::{
    io,
    path::{Path, PathBuf},
};

use sdl2::{
    event::{Event, WindowEvent},
    joystick::HatState,
    keyboard::Keycode,
    mouse::MouseButton,
    pixels::Color,
    rect::{Point, Rect},
    render::Canvas,
    video::Window,
};

use crate::frontend::{add_pad, Bindings, ACTIONS, MASKS};

use super::{bounds, box_color, i18n::Language, text, ACCENT, INK, MUTED, PANEL};

const WIDTH: u32 = 820;
const HEIGHT: u32 = 536;
const ROWS: [&str; 16] = [
    "UP",
    "DOWN",
    "LEFT",
    "RIGHT",
    "BUTTON 1",
    "BUTTON 2",
    "BUTTON 3",
    "START",
    "COIN",
    "BUTTON 4",
    "BUTTON 5",
    "BUTTON 6",
    "HORIZONTAL AXIS",
    "VERTICAL AXIS",
    "D-PAD HAT",
    "DEAD ZONE",
];
const SAVE: [i32; 4] = [662, 469, 126, 36];
const BACK: [i32; 4] = [524, 469, 126, 36];
const RESET: [i32; 4] = [32, 469, 172, 36];

pub(super) fn profile_path() -> PathBuf {
    let root = if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
    };
    root.unwrap_or_else(|| PathBuf::from(".build-local"))
        .join("kawaks-rs")
        .join("joystick.cfg")
}

fn player_rect(player: usize) -> Rect {
    Rect::new(32 + player as i32 * 74, 62, 66, 28)
}

fn row_rect(row: usize) -> Rect {
    Rect::new(
        32 + (row / 8) as i32 * 382,
        116 + (row % 8) as i32 * 39,
        374,
        34,
    )
}

fn slot(bindings: &mut Bindings, row: usize) -> Option<&mut Option<u32>> {
    match row {
        i @ 0..=11 => Some(&mut bindings.buttons[i]),
        12 => Some(&mut bindings.axis_x),
        13 => Some(&mut bindings.axis_y),
        14 => Some(&mut bindings.hat),
        _ => None,
    }
}

fn bind_button(bindings: &mut Bindings, row: usize, number: u32, lang: Language) -> String {
    let old = bindings.buttons[row].filter(|&old| {
        old != number
            && !bindings
                .buttons
                .iter()
                .enumerate()
                .any(|(index, value)| index != row && *value == Some(old))
    });
    let mut replacement = old;
    let mut first_conflict = None;
    let mut conflicts = 0;
    for (index, value) in bindings.buttons.iter_mut().enumerate() {
        if index != row && *value == Some(number) {
            first_conflict.get_or_insert(index);
            conflicts += 1;
            *value = replacement.take();
        }
    }
    bindings.buttons[row] = Some(number);
    if lang == Language::Chinese {
        return match (first_conflict, old, conflicts) {
            (Some(other), Some(old), 1) => format!(
                "{} 现用 {number}，{} 现用 {old}。",
                lang.tr(ROWS[row]),
                lang.tr(ROWS[other])
            ),
            (Some(other), None, 1) => format!(
                "{} 现用 {number}，{} 的绑定已清除。",
                lang.tr(ROWS[row]),
                lang.tr(ROWS[other])
            ),
            (Some(_), _, _) => format!("{} 现用 {number}，重复绑定已处理。", lang.tr(ROWS[row])),
            (None, _, _) => String::new(),
        };
    }
    match (first_conflict, old, conflicts) {
        (Some(other), Some(old), 1) => {
            format!(
                "{} now uses {number}. {} now uses {old}.",
                ROWS[row], ROWS[other]
            )
        }
        (Some(other), None, 1) => {
            format!(
                "{} now uses {number}. {} was cleared.",
                ROWS[row], ROWS[other]
            )
        }
        (Some(_), _, _) => {
            format!(
                "{} now uses {number}. Duplicate bindings resolved.",
                ROWS[row]
            )
        }
        (None, _, _) => String::new(),
    }
}

pub(super) fn run(
    sdl: &sdl2::Sdl,
    events: &mut sdl2::EventPump,
    current: Option<&Path>,
    lang: Language,
) -> io::Result<Option<PathBuf>> {
    let video = sdl.video().map_err(io::Error::other)?;
    let joysticks = sdl.joystick().map_err(io::Error::other)?;
    let mut pads = std::array::from_fn(|_| None);
    for index in 0..joysticks.num_joysticks().map_err(io::Error::other)? {
        add_pad(&joysticks, &mut pads, index);
    }
    let mut config = Bindings::load(current)?;
    let mut canvas = video
        .window(lang.tr("Kawaks Rust - Joystick Setup"), WIDTH, HEIGHT)
        .position_centered()
        .resizable()
        .build()
        .map_err(io::Error::other)?
        .into_canvas()
        .build()
        .map_err(io::Error::other)?;
    canvas
        .set_logical_size(WIDTH, HEIGHT)
        .map_err(io::Error::other)?;
    let window = canvas.window().id();
    let mut player = 0;
    let mut capture = None;
    let mut status = String::new();

    loop {
        let event = events.wait_event_timeout(25);
        for event in event.into_iter().chain(events.poll_iter()) {
            let id = pads[player].as_ref().map(|pad| pad.instance_id());
            let mut assigned = None;
            match event {
                Event::Quit { .. } => return Ok(None),
                Event::Window {
                    window_id,
                    win_event: WindowEvent::Close,
                    ..
                } if window_id == window => return Ok(None),
                Event::KeyDown {
                    window_id,
                    keycode: Some(Keycode::Escape),
                    ..
                } if window_id == window => {
                    if capture.take().is_none() {
                        return Ok(None);
                    }
                    status.clear();
                }
                Event::JoyDeviceAdded { which, .. } => add_pad(&joysticks, &mut pads, which),
                Event::JoyDeviceRemoved { which, .. } => {
                    for pad in &mut pads {
                        if pad.as_ref().is_some_and(|pad| pad.instance_id() == which) {
                            *pad = None;
                        }
                    }
                    if id == Some(which) {
                        capture = None;
                    }
                }
                Event::JoyButtonDown {
                    which, button_idx, ..
                } if id == Some(which) => {
                    if let Some(row @ 0..=11) = capture {
                        assigned = Some((row, button_idx));
                    }
                }
                Event::JoyAxisMotion {
                    which,
                    axis_idx,
                    value,
                    ..
                } if id == Some(which) && value.unsigned_abs() >= 17000 => {
                    if let Some(row @ (12 | 13)) = capture {
                        assigned = Some((row, axis_idx));
                    }
                }
                Event::JoyHatMotion {
                    which,
                    hat_idx,
                    state,
                    ..
                } if id == Some(which) && state != HatState::Centered => {
                    if capture == Some(14) {
                        assigned = Some((14, hat_idx));
                    }
                }
                Event::MouseButtonDown {
                    window_id,
                    mouse_btn: MouseButton::Left,
                    x,
                    y,
                    ..
                } if window_id == window => {
                    let point = Point::new(x, y);
                    if bounds(SAVE).contains_point(point) {
                        let path = profile_path();
                        match Bindings::save(&config, &path) {
                            Ok(()) => return Ok(Some(path)),
                            Err(error) => {
                                status = format!("{}: {error}", lang.tr("Could not save"))
                            }
                        }
                    } else if bounds(BACK).contains_point(point) {
                        return Ok(None);
                    } else if bounds(RESET).contains_point(point) {
                        let defaults = Bindings::default_players()[player];
                        config[player].buttons = defaults.buttons;
                        config[player].axis_x = defaults.axis_x;
                        config[player].axis_y = defaults.axis_y;
                        config[player].hat = defaults.hat;
                        config[player].deadzone = defaults.deadzone;
                        capture = None;
                        status.clear();
                    } else if let Some(next) =
                        (0..3).find(|&i| player_rect(i).contains_point(point))
                    {
                        player = next;
                        capture = None;
                        status.clear();
                    } else if let Some(row) =
                        (0..ROWS.len()).find(|&i| row_rect(i).contains_point(point))
                    {
                        status.clear();
                        capture = None;
                        let rect = row_rect(row);
                        if row == 15 {
                            let direction = if x >= rect.right() - 37 {
                                1
                            } else if x >= rect.right() - 75 {
                                -1
                            } else {
                                0
                            };
                            config[player].deadzone =
                                (i32::from(config[player].deadzone) + 1000 * direction)
                                    .clamp(0, 32767) as i16;
                        } else if x >= rect.right() - 37 {
                            *slot(&mut config[player], row).unwrap() = None;
                        } else if id.is_some() {
                            capture = Some(row);
                        } else {
                            status = "Connect a joystick for this player.".to_owned();
                        }
                    }
                }
                _ => {}
            }
            if let Some((row, number)) = assigned {
                let number = u32::from(number);
                status = if row < ACTIONS.len() {
                    bind_button(&mut config[player], row, number, lang)
                } else {
                    *slot(&mut config[player], row).unwrap() = Some(number);
                    String::new()
                };
                capture = None;
            }
        }
        let pad = pads[player].as_ref();
        let hint = match capture {
            Some(12 | 13) => "Move the axis, or press Esc to cancel.",
            Some(14) => "Move the D-pad, or press Esc to cancel.",
            Some(_) => "Press the joystick button, or Esc to cancel.",
            None if status.is_empty() => {
                "Click a row to bind. X clears it. Active inputs light up."
            }
            None => &status,
        };
        draw(
            &mut canvas,
            player,
            config[player],
            pad,
            capture,
            hint,
            lang,
        )
        .map_err(io::Error::other)?;
    }
}

fn draw(
    canvas: &mut Canvas<Window>,
    player: usize,
    mut bindings: Bindings,
    pad: Option<&sdl2::joystick::Joystick>,
    capture: Option<usize>,
    hint: &str,
    lang: Language,
) -> Result<(), String> {
    canvas.set_draw_color(Color::RGB(14, 23, 34));
    canvas.clear();
    text(canvas, 32, 26, lang.tr("JOYSTICK SETUP"), 2, INK, 25)?;
    for i in 0..3 {
        let rect = player_rect(i);
        box_color(
            canvas,
            rect,
            if i == player {
                Color::RGB(46, 83, 105)
            } else {
                PANEL
            },
        )?;
        text(canvas, rect.x() + 24, 72, &format!("P{}", i + 1), 1, INK, 3)?;
    }
    let device = pad.map_or_else(
        || lang.tr("No joystick detected").to_owned(),
        |pad| pad.name(),
    );
    text(canvas, 269, 72, &device, 1, ACCENT, 62)?;
    let live = bindings.sample_pad(pad);
    for (row, label) in ROWS.into_iter().enumerate() {
        let rect = row_rect(row);
        let active = MASKS.get(row).is_some_and(|mask| live & mask != 0);
        box_color(
            canvas,
            rect,
            if capture == Some(row) || active {
                Color::RGB(46, 83, 105)
            } else {
                PANEL
            },
        )?;
        let value = if row == 15 {
            format!("{}", bindings.deadzone)
        } else {
            slot(&mut bindings, row)
                .unwrap()
                .map_or_else(|| lang.tr("None").to_owned(), |binding| binding.to_string())
        };
        text(
            canvas,
            rect.x() + 9,
            rect.y() + 12,
            lang.tr(label),
            1,
            if active { ACCENT } else { INK },
            18,
        )?;
        text(
            canvas,
            rect.x() + 183,
            rect.y() + 12,
            if capture == Some(row) {
                lang.tr("Waiting...")
            } else {
                &value
            },
            1,
            MUTED,
            18,
        )?;
        if row == 15 {
            text(canvas, rect.right() - 63, rect.y() + 12, "-", 1, ACCENT, 1)?;
            text(canvas, rect.right() - 25, rect.y() + 12, "+", 1, ACCENT, 1)?;
        } else {
            text(canvas, rect.right() - 25, rect.y() + 12, "X", 1, ACCENT, 1)?;
        }
    }
    text(canvas, 32, 440, lang.tr(hint), 1, MUTED, 93)?;
    for (rect, label) in [
        (bounds(RESET), "DEFAULTS"),
        (bounds(BACK), "CANCEL"),
        (bounds(SAVE), "SAVE & APPLY"),
    ] {
        box_color(
            canvas,
            rect,
            if rect == bounds(SAVE) {
                Color::RGB(46, 83, 105)
            } else {
                PANEL
            },
        )?;
        text(
            canvas,
            rect.x() + 14,
            rect.y() + 14,
            lang.tr(label),
            1,
            INK,
            14,
        )?;
    }
    canvas.present();
    Ok(())
}

const _: () = assert!(ACTIONS.len() == 12);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reassigning_button_three_to_button_one_swaps_the_physical_buttons() {
        let mut bindings = Bindings::default_players()[0];
        let status = bind_button(&mut bindings, 4, 2, Language::English);
        assert_eq!(bindings.buttons[4], Some(2));
        assert_eq!(bindings.buttons[6], Some(0));
        assert_eq!(bindings.buttons[5], Some(1));
        assert!(status.contains("BUTTON 3 now uses 0"));
    }

    #[test]
    fn reassigning_to_an_unbound_action_clears_all_other_bindings_for_that_button() {
        let mut bindings = Bindings::default_players()[0];
        bindings.buttons[4] = Some(2);
        assert_eq!(bindings.buttons[6], Some(2));
        bind_button(&mut bindings, 0, 2, Language::English);
        assert_eq!(bindings.buttons[0], Some(2));
        assert_eq!(bindings.buttons[4], None);
        assert_eq!(bindings.buttons[6], None);
        assert_eq!(bindings.axis_x, Some(0));
    }
}
