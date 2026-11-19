use std::{
    io,
    path::Path,
    thread,
    time::{Duration, Instant},
};

use kawaks_rs::{
    ArcadeMachine, FrameInput, AUDIO_SAMPLES_PER_FRAME, AUDIO_SAMPLE_RATE, FRAME_HEIGHT,
    FRAME_WIDTH,
};
use sdl2::{
    audio::AudioSpecDesired,
    event::{Event, WindowEvent},
    joystick::{HatState, Joystick},
    keyboard::Scancode,
    pixels::PixelFormatEnum,
};

use crate::netplay::Peer;

const ACTIONS: [&str; 12] = [
    "up", "down", "left", "right", "button1", "button2", "button3", "start", "coin", "button4",
    "button5", "button6",
];
const MASKS: [u16; 12] = [1, 2, 4, 8, 16, 32, 64, 0x1000, 0x2000, 0x80, 0x100, 0x200];

#[derive(Clone, Copy)]
struct Bindings {
    keys: [Option<Scancode>; 12],
    buttons: [Option<u32>; 12],
    axis_x: Option<u32>,
    axis_y: Option<u32>,
    hat: Option<u32>,
    deadzone: i16,
}

impl Bindings {
    fn default_players() -> [Self; 3] {
        use Scancode::*;
        let buttons = [
            None,
            None,
            None,
            None,
            Some(0),
            Some(1),
            Some(2),
            Some(7),
            Some(6),
            Some(3),
            Some(4),
            Some(5),
        ];
        let pad = Self {
            keys: [None; 12],
            buttons,
            axis_x: Some(0),
            axis_y: Some(1),
            hat: Some(0),
            deadzone: 10000,
        };
        [
            Self {
                keys: [Up, Down, Left, Right, Z, X, C, Num1, Num5, V, B, N].map(Some),
                ..pad
            },
            Self {
                keys: [W, S, A, D, F, G, H, Num2, Num6, R, T, Y].map(Some),
                ..pad
            },
            pad,
        ]
    }

    fn load(path: Option<&Path>) -> io::Result<[Self; 3]> {
        let mut players = Self::default_players();
        let Some(path) = path else { return Ok(players) };
        for (line, value) in std::fs::read_to_string(path)?.lines().enumerate() {
            let value = value.split('#').next().unwrap().trim();
            if value.is_empty() {
                continue;
            }
            let err = || {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{}:{} invalid input binding", path.display(), line + 1),
                )
            };
            let (key, value) = value.split_once('=').ok_or_else(err)?;
            let value = value.trim();
            let mut path = key.trim().split('.');
            let player = path
                .next()
                .and_then(|p| p.strip_prefix('p'))
                .and_then(|n| n.parse::<usize>().ok())
                .filter(|n| (1..=3).contains(n))
                .ok_or_else(err)?;
            let bindings = &mut players[player - 1];
            let source = path.next().ok_or_else(err)?;
            let action = path.next().ok_or_else(err)?;
            if path.next().is_some() {
                return Err(err());
            }

            let index = || {
                ACTIONS
                    .iter()
                    .position(|&name| name == action)
                    .ok_or_else(err)
            };
            let number = || {
                if value == "none" {
                    Ok(None)
                } else {
                    value.parse::<u32>().map(Some).map_err(|_| err())
                }
            };
            match (source, action) {
                ("key", _) => {
                    bindings.keys[index()?] = if value == "none" {
                        None
                    } else {
                        Some(Scancode::from_name(value).ok_or_else(err)?)
                    }
                }
                ("pad", "axis_x") => bindings.axis_x = number()?,
                ("pad", "axis_y") => bindings.axis_y = number()?,
                ("pad", "hat") => bindings.hat = number()?,
                ("pad", "deadzone") => {
                    bindings.deadzone = value
                        .parse::<i16>()
                        .ok()
                        .filter(|v| *v >= 0)
                        .ok_or_else(err)?
                }
                ("pad", _) => bindings.buttons[index()?] = number()?,
                _ => return Err(err()),
            }
        }
        Ok(players)
    }

    fn sample(self, keys: &sdl2::keyboard::KeyboardState<'_>, pad: Option<&Joystick>) -> u16 {
        let mut buttons = 0;
        for (index, mask) in MASKS.into_iter().enumerate() {
            if self.keys[index].is_some_and(|key| keys.is_scancode_pressed(key))
                || pad
                    .zip(self.buttons[index])
                    .is_some_and(|(pad, button)| pad.button(button).unwrap_or(false))
            {
                buttons |= mask;
            }
        }
        if let Some(pad) = pad {
            let read = |axis: Option<u32>| axis.and_then(|axis| pad.axis(axis).ok()).unwrap_or(0);
            let x = read(self.axis_x);
            let y = read(self.axis_y);
            if x < -self.deadzone {
                buttons |= 4;
            }
            if x > self.deadzone {
                buttons |= 8;
            }
            if y < -self.deadzone {
                buttons |= 1;
            }
            if y > self.deadzone {
                buttons |= 2;
            }
            buttons |= match self
                .hat
                .and_then(|hat| pad.hat(hat).ok())
                .unwrap_or(HatState::Centered)
            {
                HatState::Centered => 0,
                HatState::Up => 1,
                HatState::Down => 2,
                HatState::Left => 4,
                HatState::Right => 8,
                HatState::RightUp => 9,
                HatState::RightDown => 10,
                HatState::LeftUp => 5,
                HatState::LeftDown => 6,
            };
        }
        if buttons & 3 == 3 {
            buttons &= !3;
        }
        if buttons & 12 == 12 {
            buttons &= !12;
        }
        buttons
    }
}

pub fn run(
    machine: &mut dyn ArcadeMachine,
    mut peer: Option<&mut Peer>,
    config: Option<&Path>,
    frames: Option<u64>,
    mute: bool,
    game: &str,
) -> io::Result<()> {
    let bindings = Bindings::load(config)?;
    let sdl = sdl2::init().map_err(sdl_error)?;
    let video = sdl.video().map_err(sdl_error)?;
    let joystick = sdl.joystick().map_err(sdl_error)?;
    let mut events = sdl.event_pump().map_err(sdl_error)?;
    let window = video
        .window(
            &format!("Kawaks Rust · {game}"),
            FRAME_WIDTH as u32 * 2,
            FRAME_HEIGHT as u32 * 2,
        )
        .position_centered()
        .resizable()
        .build()
        .map_err(sdl_error)?;
    let mut canvas = window.into_canvas().build().map_err(sdl_error)?;
    canvas
        .set_logical_size(FRAME_WIDTH as u32, FRAME_HEIGHT as u32)
        .map_err(sdl_error)?;
    let textures = canvas.texture_creator();
    let mut texture = textures
        .create_texture_streaming(
            PixelFormatEnum::RGB24,
            FRAME_WIDTH as u32,
            FRAME_HEIGHT as u32,
        )
        .map_err(sdl_error)?;

    let audio = if mute {
        None
    } else {
        let device = sdl
            .audio()
            .map_err(sdl_error)?
            .open_queue::<i16, _>(
                None,
                &AudioSpecDesired {
                    freq: Some(AUDIO_SAMPLE_RATE as i32),
                    channels: Some(2),
                    samples: Some(512),
                },
            )
            .map_err(sdl_error)?;
        device.resume();
        Some(device)
    };
    let mut pads: [Option<Joystick>; 3] = std::array::from_fn(|_| None);
    for index in 0..joystick.num_joysticks().map_err(sdl_error)? {
        add_pad(&joystick, &mut pads, index);
    }

    let interval = Duration::from_nanos(
        1_000_000_000 * AUDIO_SAMPLES_PER_FRAME as u64 / u64::from(AUDIO_SAMPLE_RATE),
    );
    let mut next = Instant::now();
    let mut report = machine.report();
    let mut focused = true;
    let mut pcm = Vec::with_capacity(AUDIO_SAMPLES_PER_FRAME * 2);
    'window: loop {
        for event in events.poll_iter() {
            match event {
                Event::Quit { .. }
                | Event::KeyDown {
                    scancode: Some(Scancode::Escape),
                    ..
                } => break 'window,
                Event::JoyDeviceAdded { which, .. } => add_pad(&joystick, &mut pads, which),
                Event::JoyDeviceRemoved { which, .. } => {
                    for pad in &mut pads {
                        if pad.as_ref().is_some_and(|pad| pad.instance_id() == which) {
                            *pad = None;
                        }
                    }
                }
                Event::Window {
                    win_event: WindowEvent::FocusGained,
                    ..
                } => focused = true,
                Event::Window {
                    win_event: WindowEvent::FocusLost,
                    ..
                } => focused = false,
                _ => {}
            }
        }

        let advance = frames.is_none_or(|limit| report.frame < limit);
        if advance {
            let keys = events.keyboard_state();
            let mut input = FrameInput::default();
            if focused {
                for index in 0..3 {
                    input.players[index] = bindings[index].sample(&keys, pads[index].as_ref());
                }
            }
            if let Some(peer) = &mut peer {
                input = peer.exchange(input.players[0], report)?;
            }
            report = machine.step_frame(input)?;
            if let Some(device) = &audio {
                if device.size() < 8 * pcm.capacity() as u32 {
                    pcm.clear();
                    pcm.extend(machine.frame_audio().iter().flatten().copied());
                    device.queue_audio(&pcm).map_err(sdl_error)?;
                }
            }
        }

        texture
            .update(None, &machine.render_rgb(), FRAME_WIDTH * 3)
            .map_err(sdl_error)?;
        canvas.clear();
        canvas.copy(&texture, None, None).map_err(sdl_error)?;
        canvas.present();
        if frames.is_some_and(|limit| report.frame >= limit) {
            break;
        }
        next += interval;
        let now = Instant::now();
        if next > now {
            thread::sleep(next - now);
        } else if now - next > interval * 3 {
            next = now;
        }
    }
    Ok(())
}

fn add_pad(joystick: &sdl2::JoystickSubsystem, players: &mut [Option<Joystick>; 3], index: u32) {
    let Ok(device) = joystick.open(index) else {
        return;
    };
    if players
        .iter()
        .flatten()
        .any(|open| open.instance_id() == device.instance_id())
    {
        return;
    }
    if let Some(player) = players.iter().position(Option::is_none) {
        eprintln!(
            "Player {} joystick: {} ({} axes, {} buttons, {} hats)",
            player + 1,
            device.name(),
            device.num_axes(),
            device.num_buttons(),
            device.num_hats()
        );
        players[player] = Some(device);
    }
}

fn sdl_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdl2::sys;

    #[test]
    fn virtual_arcade_stick_reaches_the_logical_input_map() {
        let sdl = sdl2::init().unwrap();
        sdl2::hint::set("SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS", "1");
        let joystick = sdl.joystick().unwrap();
        let events = sdl.event_pump().unwrap();
        // The SDL subsystem remains alive, and the joystick handle stays open
        // for the entire sequence of virtual input updates.
        let index = unsafe {
            sys::SDL_JoystickAttachVirtual(
                sys::SDL_JoystickType::SDL_JOYSTICK_TYPE_ARCADE_STICK,
                2,
                8,
                1,
            )
        };
        assert!(index >= 0, "{}", sdl2::get_error());
        let pad = joystick.open(index as u32).unwrap();
        let raw = unsafe { sys::SDL_JoystickFromInstanceID(pad.instance_id() as i32) };
        assert!(!raw.is_null());
        unsafe {
            assert_eq!(sys::SDL_JoystickSetVirtualAxis(raw, 0, 24000), 0);
            assert_eq!(sys::SDL_JoystickSetVirtualAxis(raw, 1, 3000), 0);
            assert_eq!(
                sys::SDL_JoystickSetVirtualHat(raw, 0, HatState::Up as u8),
                0
            );
            assert_eq!(sys::SDL_JoystickSetVirtualButton(raw, 1, 1), 0);
            assert_eq!(sys::SDL_JoystickSetVirtualButton(raw, 5, 1), 0);
            assert_eq!(sys::SDL_JoystickSetVirtualButton(raw, 6, 1), 0);
        }
        joystick.update();
        let mut map = Bindings::default_players()[0];
        map.buttons[4] = Some(1);
        map.buttons[5] = None;
        assert_eq!(
            map.sample(&events.keyboard_state(), Some(&pad)),
            0x2000 | 0x200 | 16 | 8 | 1
        );
        drop(pad);
        assert_eq!(unsafe { sys::SDL_JoystickDetachVirtual(index) }, 0);
    }
}
