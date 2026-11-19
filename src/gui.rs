use std::{
    io,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
};

use font8x8::{UnicodeFonts, BASIC_FONTS};
use kawaks_rs::ArcadeMachine;
use sdl2::{
    event::Event,
    keyboard::{Keycode, Mod},
    mouse::MouseButton,
    pixels::Color,
    rect::{Point, Rect},
    render::Canvas,
    video::Window,
};

use crate::{frontend, load_machine, netplay::Peer};

const WIDTH: u32 = 820;
const HEIGHT: u32 = 508;
const INK: Color = Color::RGB(236, 244, 250);
const MUTED: Color = Color::RGB(167, 184, 202);
const ACCENT: Color = Color::RGB(146, 225, 240);
const PANEL: Color = Color::RGB(31, 44, 59);
const GAMES: [(&str, &str, &str); 6] = [
    ("dino", "CPS1", "dino"),
    ("wof", "CPS1", "wof"),
    ("wofa", "CPS1", "wof"),
    ("punisher", "CPS1", "punisher"),
    ("ssf2", "CPS2", "ssf2"),
    ("kof97", "NEO GEO", "kof97"),
];
const ROM: [i32; 4] = [32, 162, 756, 41];
const SAVE: [i32; 4] = [32, 210, 756, 41];
const BIOS: [i32; 4] = [32, 258, 756, 41];
const INPUT: [i32; 4] = [32, 306, 756, 41];
const AUDIO: [i32; 4] = [647, 366, 141, 32];
const ADDRESS: [i32; 4] = [300, 366, 333, 32];
const START: [i32; 4] = [606, 432, 182, 44];
const CLEAR_COLUMN: i32 = 742;
const HOST_ADDRESS: &str = "0.0.0.0:22651";
const JOIN_ADDRESS: &str = "127.0.0.1:22651";

fn bounds([x, y, w, h]: [i32; 4]) -> Rect {
    Rect::new(x, y, w as u32, h as u32)
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Mode {
    #[default]
    Local,
    Host,
    Join,
}

impl Mode {
    const OPTIONS: [(&str, Self); 3] = [
        ("Local", Self::Local),
        ("Host LAN", Self::Host),
        ("Join LAN", Self::Join),
    ];

    fn rect(index: usize) -> Rect {
        Rect::new(32 + index as i32 * 88, 366, 82, 32)
    }
}

struct Settings {
    game: usize,
    rom: PathBuf,
    save: Option<PathBuf>,
    bios: Option<PathBuf>,
    input: Option<PathBuf>,
    mute: bool,
    mode: Mode,
    address: String,
}

impl Settings {
    fn new() -> Self {
        let mut settings = Self {
            game: 0,
            rom: PathBuf::new(),
            save: None,
            bios: None,
            input: None,
            mute: false,
            mode: Mode::Local,
            address: String::new(),
        };
        settings.select_game(0);
        settings
    }

    fn select_game(&mut self, index: usize) {
        let (game, platform, archive) = GAMES[index];
        let relative = PathBuf::from("roms")
            .join(platform.to_ascii_lowercase().replace(' ', ""))
            .join(format!("{archive}.zip"));
        self.rom = default_rom(&relative);
        self.game = index;
        self.save = None;
        if game != "kof97" {
            self.bios = None;
        }
    }

    fn default_save(&self) -> PathBuf {
        let name = format!("{}.ks0", GAMES[self.game].0);
        self.rom
            .ancestors()
            .nth(3)
            .map(|root| root.join("saves"))
            .filter(|saves| saves.is_dir())
            .unwrap_or_else(|| PathBuf::from("."))
            .join(name)
    }

    fn load(&self) -> io::Result<(Launch, [u32; 4])> {
        let game = GAMES[self.game].0;
        let (machine, crc) =
            load_machine(game, self.rom.clone(), self.bios.clone(), self.save.clone())?;
        Ok((
            Launch {
                machine,
                peer: None,
            },
            crc,
        ))
    }
}

fn default_rom(relative: &Path) -> PathBuf {
    let cwd = std::env::current_dir().ok();
    let exe = std::env::current_exe().ok();
    cwd.as_deref()
        .into_iter()
        .chain(exe.as_deref().and_then(Path::parent))
        .flat_map(|base| base.ancestors().take(6))
        .map(|base| base.join(relative))
        .find(|path| path.is_file())
        .unwrap_or_else(|| relative.to_owned())
}

struct Launch {
    machine: Box<dyn ArcadeMachine>,
    peer: Option<Peer>,
}

struct Pending {
    launch: Option<Launch>,
    stop: Arc<AtomicBool>,
    result: mpsc::Receiver<io::Result<Peer>>,
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub fn run() -> io::Result<()> {
    let mut settings = Settings::new();
    let mut status = String::new();
    while let Some(mut launch) = choose(&mut settings, &mut status)? {
        status = match frontend::run(
            launch.machine.as_mut(),
            launch.peer.as_mut(),
            settings.input.as_deref(),
            None,
            settings.mute,
            GAMES[settings.game].0,
        ) {
            Ok(()) => String::new(),
            Err(error)
                if launch.peer.is_some()
                    && matches!(
                        error.kind(),
                        io::ErrorKind::UnexpectedEof
                            | io::ErrorKind::ConnectionAborted
                            | io::ErrorKind::ConnectionReset
                            | io::ErrorKind::BrokenPipe
                    ) =>
            {
                "The other player disconnected.".to_owned()
            }
            Err(error) => error.to_string(),
        };
    }
    Ok(())
}

fn choose(settings: &mut Settings, status: &mut String) -> io::Result<Option<Launch>> {
    let sdl = sdl2::init().map_err(io::Error::other)?;
    let video = sdl.video().map_err(io::Error::other)?;
    let mut events = sdl.event_pump().map_err(io::Error::other)?;
    let mut canvas = video
        .window("Kawaks Rust", WIDTH, HEIGHT)
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
    let mut pending: Option<Pending> = None;
    let mut editing = false;
    let mut address_selected = false;
    let mut dirty = true;

    loop {
        if let Some(job) = &mut pending {
            match job.result.try_recv() {
                Ok(Ok(peer)) => {
                    let mut launch = job.launch.take().expect("pending job holds its game");
                    launch.peer = Some(peer);
                    return Ok(Some(launch));
                }
                Ok(Err(error)) => {
                    *status = error.to_string();
                    pending = None;
                    dirty = true;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    *status = "LAN connection ended".to_owned();
                    pending = None;
                    dirty = true;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        for event in events
            .wait_event_timeout(22)
            .into_iter()
            .chain(events.poll_iter())
        {
            let mut start = false;
            dirty = true;
            match event {
                Event::Quit { .. } => return Ok(None),
                Event::KeyDown {
                    keycode: Some(Keycode::Escape),
                    ..
                } => {
                    if pending.take().is_some() {
                        status.clear();
                    } else if editing {
                        editing = false;
                        video.text_input().stop();
                    } else {
                        return Ok(None);
                    }
                }
                Event::KeyDown {
                    keycode: Some(Keycode::Return | Keycode::KpEnter),
                    repeat: false,
                    ..
                } => start = true,
                Event::KeyDown {
                    keycode: Some(Keycode::A),
                    keymod,
                    ..
                } if editing
                    && keymod.intersects(
                        Mod::LCTRLMOD | Mod::RCTRLMOD | Mod::LGUIMOD | Mod::RGUIMOD,
                    ) =>
                {
                    address_selected = true;
                }
                Event::KeyDown {
                    keycode: Some(Keycode::V),
                    keymod,
                    ..
                } if editing
                    && keymod.intersects(
                        Mod::LCTRLMOD | Mod::RCTRLMOD | Mod::LGUIMOD | Mod::RGUIMOD,
                    ) =>
                {
                    if let Ok(text) = video.clipboard().clipboard_text() {
                        edit_address(&mut settings.address, &mut address_selected, text.trim());
                    }
                }
                Event::KeyDown {
                    keycode: Some(Keycode::Backspace | Keycode::Delete),
                    ..
                } if editing => {
                    if std::mem::take(&mut address_selected) {
                        settings.address.clear();
                    } else {
                        settings.address.pop();
                    }
                }
                Event::TextInput { text, .. } if editing => {
                    edit_address(&mut settings.address, &mut address_selected, &text);
                }
                Event::DropFile { filename, .. } if pending.is_none() => {
                    let path = PathBuf::from(filename);
                    if path
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
                    {
                        if let Some(game) = GAMES.iter().position(|item| {
                            path.file_stem()
                                .is_some_and(|s| s.eq_ignore_ascii_case(item.0))
                        }) {
                            if settings.game != game {
                                settings.select_game(game);
                            }
                        }
                        settings.rom = path;
                    } else {
                        settings.save = Some(path);
                    }
                }
                Event::MouseButtonDown {
                    mouse_btn: MouseButton::Left,
                    x,
                    y,
                    ..
                } => {
                    let point = Point::new(x, y);
                    if bounds(START).contains_point(point) {
                        start = true;
                    } else if pending.is_none() {
                        editing =
                            settings.mode != Mode::Local && bounds(ADDRESS).contains_point(point);
                        address_selected = editing;
                        if editing {
                            video.text_input().start();
                        } else {
                            video.text_input().stop();
                        }
                        if let Some(index) =
                            (0..GAMES.len()).find(|&i| game_rect(i).contains_point(point))
                        {
                            settings.select_game(index);
                            status.clear();
                        } else if bounds(ROM).contains_point(point) {
                            if let Some(path) = open_file(
                                "Open arcade ROM",
                                &settings.rom,
                                Some((&["*.zip"], "ROM archive")),
                            ) {
                                settings.rom = path;
                            }
                        } else if bounds(SAVE).contains_point(point) {
                            let default = settings.default_save();
                            choose_optional(
                                x,
                                &mut settings.save,
                                "Open KWK4 state",
                                &default,
                                Some((&["*.ks?", "*.kwk", "*.kwk4"], "KWK4 save")),
                            );
                        } else if GAMES[settings.game].0 == "kof97"
                            && bounds(BIOS).contains_point(point)
                        {
                            choose_optional(
                                x,
                                &mut settings.bios,
                                "Open Neo Geo BIOS",
                                &settings.rom.with_file_name("neogeo.zip"),
                                Some((&["*.zip"], "BIOS archive")),
                            );
                        } else if bounds(INPUT).contains_point(point) {
                            choose_optional(
                                x,
                                &mut settings.input,
                                "Open input configuration",
                                Path::new("input.cfg"),
                                None,
                            );
                        } else if bounds(AUDIO).contains_point(point) {
                            settings.mute = !settings.mute;
                        } else if let Some(index) =
                            (0..3).find(|&i| Mode::rect(i).contains_point(point))
                        {
                            settings.mode = Mode::OPTIONS[index].1;
                            status.clear();
                            if matches!(settings.address.as_str(), "" | HOST_ADDRESS | JOIN_ADDRESS)
                            {
                                settings.address = if settings.mode == Mode::Host {
                                    HOST_ADDRESS.into()
                                } else {
                                    JOIN_ADDRESS.into()
                                };
                            }
                        }
                    }
                }
                _ => {}
            }
            if start {
                if pending.take().is_some() {
                    status.clear();
                    continue;
                }
                editing = false;
                video.text_input().stop();
                match settings.load() {
                    Ok((launch, _)) if settings.mode == Mode::Local => return Ok(Some(launch)),
                    Ok((launch, crc)) => match settings.address.parse::<SocketAddr>() {
                        Ok(address) => {
                            let stop = Arc::new(AtomicBool::new(false));
                            let worker_stop = stop.clone();
                            let state = launch.machine.report().state_crc32;
                            let mode = settings.mode;
                            let (send, result) = mpsc::channel();
                            thread::spawn(move || {
                                let outcome = if mode == Mode::Host {
                                    Peer::listen_until(address, crc, state, &worker_stop).map_err(|error| {
                                        match error.kind() {
                                            io::ErrorKind::AddrInUse | io::ErrorKind::PermissionDenied => io::Error::new(
                                                error.kind(),
                                                format!("Cannot listen on {address}. Try a different port. {error}"),
                                            ),
                                            _ => error,
                                        }
                                    })
                                } else {
                                    Peer::connect(address, crc, state)
                                };
                                let _ = send.send(outcome);
                            });
                            *status = if mode == Mode::Host {
                                "Waiting for player 2"
                            } else {
                                "Connecting to host"
                            }
                            .to_owned();
                            pending = Some(Pending {
                                launch: Some(launch),
                                stop,
                                result,
                            });
                        }
                        Err(_) => *status = "Enter a numeric IP and port".to_owned(),
                    },
                    Err(error) => *status = error.to_string(),
                }
            }
        }
        if dirty {
            draw(&mut canvas, settings, status, pending.is_some(), editing)
                .map_err(io::Error::other)?;
            dirty = false;
        }
    }
}

fn choose_optional(
    x: i32,
    value: &mut Option<PathBuf>,
    title: &str,
    default: &Path,
    filter: Option<(&[&str], &str)>,
) {
    if x >= CLEAR_COLUMN {
        *value = None;
    } else if let Some(path) = open_file(title, value.as_deref().unwrap_or(default), filter) {
        *value = Some(path);
    }
}

fn open_file(title: &str, default: &Path, filter: Option<(&[&str], &str)>) -> Option<PathBuf> {
    let absolute = std::path::absolute(default).unwrap_or_else(|_| default.to_owned());
    tinyfiledialogs::open_file_dialog(title, &absolute.to_string_lossy(), filter).map(PathBuf::from)
}

fn edit_address(address: &mut String, selected: &mut bool, text: &str) {
    let characters = text
        .chars()
        .filter(|c| c.is_ascii_hexdigit() || matches!(c, '.' | ':' | '[' | ']' | '%'));
    if std::mem::take(selected) {
        address.clear();
    }
    address.extend(characters.take(64 - address.len()));
}

fn game_rect(index: usize) -> Rect {
    Rect::new(32 + index as i32 * 127, 94, 121, 49)
}

fn draw(
    canvas: &mut Canvas<Window>,
    settings: &Settings,
    status: &str,
    pending: bool,
    editing: bool,
) -> Result<(), String> {
    canvas.set_draw_color(Color::RGB(14, 23, 34));
    canvas.clear();
    text(canvas, 32, 26, "KAWAKS RUST", 3, INK, 32)?;
    text(canvas, 32, 66, "SELECT GAME", 1, ACCENT, 20)?;
    for (index, (name, platform, _)) in GAMES.iter().enumerate() {
        let rect = game_rect(index);
        box_color(
            canvas,
            rect,
            if settings.game == index {
                Color::RGB(46, 83, 105)
            } else {
                PANEL
            },
        )?;
        text(
            canvas,
            rect.x() + 9,
            rect.y() + 9,
            &name.to_ascii_uppercase(),
            1,
            INK,
            13,
        )?;
        text(canvas, rect.x() + 9, rect.y() + 29, platform, 1, MUTED, 13)?;
    }
    let bios_enabled = GAMES[settings.game].0 == "kof97";
    let rows = [
        (ROM, "ROM", Some(settings.rom.as_path()), "Choose ZIP", true),
        (
            SAVE,
            "KWK4 STATE",
            settings.save.as_deref(),
            "Cold boot",
            true,
        ),
        (
            BIOS,
            "NEO GEO BIOS",
            settings.bios.as_deref(),
            if bios_enabled {
                "Beside ROM by default"
            } else {
                "Neo Geo games only"
            },
            bios_enabled,
        ),
        (
            INPUT,
            "KEY / PAD MAP",
            settings.input.as_deref(),
            "Default controls",
            true,
        ),
    ];
    for (region, label, path, placeholder, enabled) in rows {
        let rect = bounds(region);
        box_color(
            canvas,
            rect,
            if enabled {
                PANEL
            } else {
                Color::RGB(21, 34, 46)
            },
        )?;
        text(
            canvas,
            rect.x() + 12,
            rect.y() + 16,
            label,
            1,
            if enabled { ACCENT } else { MUTED },
            16,
        )?;
        let display = path.map(|path| path.display().to_string());
        text(
            canvas,
            174,
            rect.y() + 16,
            display.as_deref().unwrap_or(placeholder),
            1,
            INK,
            61,
        )?;
        if enabled {
            text(canvas, 699, rect.y() + 16, "OPEN", 1, ACCENT, 8)?;
            if region != ROM {
                text(canvas, 764, rect.y() + 16, "X", 1, MUTED, 2)?;
            }
        }
    }
    for (index, (name, mode)) in Mode::OPTIONS.iter().enumerate() {
        let rect = Mode::rect(index);
        box_color(
            canvas,
            rect,
            if settings.mode == *mode {
                Color::RGB(46, 83, 105)
            } else {
                PANEL
            },
        )?;
        text(canvas, rect.x() + 8, rect.y() + 12, name, 1, INK, 10)?;
    }
    if settings.mode != Mode::Local {
        box_color(
            canvas,
            bounds(ADDRESS),
            if editing {
                Color::RGB(46, 83, 105)
            } else {
                PANEL
            },
        )?;
        text(canvas, 311, 378, &settings.address, 1, INK, 39)?;
    }
    box_color(canvas, bounds(AUDIO), PANEL)?;
    text(
        canvas,
        658,
        378,
        if settings.mute {
            "AUDIO OFF"
        } else {
            "AUDIO ON"
        },
        1,
        INK,
        14,
    )?;
    box_color(canvas, bounds(START), ACCENT)?;
    text(
        canvas,
        621,
        447,
        if pending { "CANCEL" } else { "PLAY" },
        2,
        Color::RGB(14, 23, 34),
        10,
    )?;
    for (index, line) in wrap(status, 67).iter().take(3).enumerate() {
        text(canvas, 32, 435 + index as i32 * 17, line, 1, MUTED, 67)?;
    }
    canvas.present();
    Ok(())
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if line.len() + word.len() >= width && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    lines.push(line);
    lines
}

fn box_color(canvas: &mut Canvas<Window>, rect: Rect, color: Color) -> Result<(), String> {
    canvas.set_draw_color(color);
    canvas.fill_rect(rect)
}

fn text(
    canvas: &mut Canvas<Window>,
    x: i32,
    y: i32,
    value: &str,
    scale: i32,
    color: Color,
    max: usize,
) -> Result<(), String> {
    let mut pixels = Vec::new();
    let clipped = value.chars().count() > max;
    let suffix = if clipped { "..." } else { "" };
    for (index, ch) in value
        .chars()
        .take(max - suffix.len())
        .chain(suffix.chars())
        .enumerate()
    {
        if let Some(bitmap) = BASIC_FONTS.get(ch).or_else(|| BASIC_FONTS.get('?')) {
            for (row, bits) in bitmap.into_iter().enumerate() {
                for column in 0..8 {
                    if bits & (1 << column) != 0 {
                        pixels.push(Rect::new(
                            x + (index as i32 * 8 + column) * scale,
                            y + row as i32 * scale,
                            scale as u32,
                            scale as u32,
                        ));
                    }
                }
            }
        }
    }
    canvas.set_draw_color(color);
    canvas.fill_rects(&pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires local KOF97 ROM and KWK4 plus SDL's dummy video driver"]
    fn launcher_opens_kof97_save_and_draws_controls() {
        let mut settings = Settings::new();
        settings.select_game(5);
        let fixture = std::env::var_os("KAWAKS_FIXTURE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join(".."));
        settings.rom = fixture.join("roms/neogeo/kof97.zip");
        settings.save = Some(fixture.join("saves/kof97.ks0"));
        let (mut launch, _) = settings.load().unwrap();
        assert_eq!(launch.machine.report().main_pc, 0x9c50);
        launch
            .machine
            .step_frame(kawaks_rs::FrameInput::default())
            .unwrap();
        assert!(launch
            .machine
            .frame_audio()
            .iter()
            .flatten()
            .any(|&v| v != 0));

        let sdl = sdl2::init().unwrap();
        let video = sdl.video().unwrap();
        let mut canvas = video
            .window("Launcher smoke test", WIDTH, HEIGHT)
            .hidden()
            .build()
            .unwrap()
            .into_canvas()
            .software()
            .build()
            .unwrap();
        draw(&mut canvas, &settings, "", false, false).unwrap();
        let rgb = canvas
            .read_pixels(None, sdl2::pixels::PixelFormatEnum::RGB24)
            .unwrap();
        let pixel = |x, y| {
            let offset = (y * WIDTH as usize + x) * 3;
            &rgb[offset..offset + 3]
        };
        assert_eq!(pixel(32 + 127 * 5 + 3, 98), &[46, 83, 105]);
        assert_eq!(pixel(610, 435), &[146, 225, 240]);
    }
}
