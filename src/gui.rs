mod catalog;
mod i18n;
mod pad_setup;
mod stage_picker;

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

use i18n::Language;

use crate::{frontend, load_machine, netplay::Peer};

const WIDTH: u32 = 820;
const HEIGHT: u32 = 616;
const LANGUAGE: [i32; 4] = [674, 22, 114, 30];
const INK: Color = Color::RGB(236, 244, 250);
const MUTED: Color = Color::RGB(167, 184, 202);
const ACCENT: Color = Color::RGB(146, 225, 240);
const PANEL: Color = Color::RGB(31, 44, 59);
const PAGE_SIZE: usize = 12;
const SEARCH: [i32; 4] = [226, 60, 319, 25];
const PREVIOUS: [i32; 4] = [696, 60, 42, 25];
const NEXT: [i32; 4] = [746, 60, 42, 25];
const ROM: [i32; 4] = [32, 270, 756, 41];
const SAVE: [i32; 4] = [32, 318, 756, 41];
const BIOS: [i32; 4] = [32, 366, 756, 41];
const INPUT: [i32; 4] = [32, 414, 756, 41];
const AUDIO: [i32; 4] = [647, 474, 141, 32];
const ADDRESS: [i32; 4] = [300, 474, 333, 32];
const START: [i32; 4] = [606, 540, 182, 44];
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
        Rect::new(32 + index as i32 * 88, 474, 82, 32)
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Edit {
    #[default]
    None,
    Search,
    Address,
}

struct Settings {
    catalog: Vec<catalog::Entry>,
    language: Language,
    search: String,
    page: usize,
    game: usize,
    rom: PathBuf,
    save: Option<PathBuf>,
    stage: Option<u8>,
    bios: Option<PathBuf>,
    input: Option<PathBuf>,
    mute: bool,
    mode: Mode,
    address: String,
}

impl Settings {
    fn new() -> Self {
        let mut settings = Self {
            catalog: catalog::discover(),
            language: Language::load(),
            search: String::new(),
            page: 0,
            game: 0,
            rom: PathBuf::new(),
            save: None,
            stage: None,
            bios: None,
            input: Some(pad_setup::profile_path()).filter(|path| path.is_file()),
            mute: false,
            mode: Mode::Local,
            address: String::new(),
        };
        settings.select_game(0);
        settings
    }

    fn entry(&self) -> &catalog::Entry {
        &self.catalog[self.game]
    }

    fn filtered(&self) -> Vec<usize> {
        let query = self.search.trim().to_ascii_lowercase();
        self.catalog
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| entry.matches(&query).then_some(index))
            .collect()
    }

    fn change_page(&mut self, forward: bool) {
        let last = self.filtered().len().div_ceil(PAGE_SIZE).saturating_sub(1);
        self.page = if forward {
            (self.page + 1).min(last)
        } else {
            self.page.saturating_sub(1)
        };
    }

    fn select_game(&mut self, index: usize) {
        if index == self.game && !self.rom.as_os_str().is_empty() {
            return;
        }
        self.game = index;
        self.rom = self.entry().rom.clone();
        self.save = None;
        self.stage = None;
        if self.entry().driver != Some("kof97") {
            self.bios = None;
        }
    }

    fn add_search_text(&mut self, value: &str) {
        let room = 40usize.saturating_sub(self.search.chars().count());
        self.search
            .extend(value.chars().filter(|c| !c.is_control()).take(room));
        self.page = 0;
    }

    fn default_save(&self) -> PathBuf {
        let name = format!("{}.ks0", self.entry().id);
        self.rom
            .ancestors()
            .nth(3)
            .map(|root| root.join("saves"))
            .filter(|saves| saves.is_dir())
            .unwrap_or_else(|| PathBuf::from("."))
            .join(name)
    }

    fn load(&self) -> io::Result<(Launch, [u32; 4])> {
        let game = self.entry().driver.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "{}: {}",
                    self.entry().title(self.language),
                    self.language.tr("No compatible driver for this ROM.")
                ),
            )
        })?;
        let (machine, crc) = load_machine(
            game,
            self.rom.clone(),
            self.bios.clone(),
            self.save.clone(),
            self.stage,
        )?;
        Ok((
            Launch {
                machine,
                game,
                peer: None,
            },
            crc,
        ))
    }
}

struct Launch {
    machine: Box<dyn ArcadeMachine>,
    game: &'static str,
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
            settings.language.title(launch.game),
            settings.language.tr("Paused (Enter to resume)"),
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
    let mut editing = Edit::None;
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
        let mut configure_pad = false;
        let mut pick_stage = false;
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
                    } else if editing != Edit::None {
                        editing = Edit::None;
                        video.text_input().stop();
                    } else {
                        return Ok(None);
                    }
                }
                Event::KeyDown {
                    keycode: Some(Keycode::Return | Keycode::KpEnter),
                    repeat: false,
                    ..
                } if editing == Edit::Search => {
                    editing = Edit::None;
                    video.text_input().stop();
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
                } if editing == Edit::Address
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
                } if editing == Edit::Address
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
                } if editing == Edit::Address => {
                    if std::mem::take(&mut address_selected) {
                        settings.address.clear();
                    } else {
                        settings.address.pop();
                    }
                }
                Event::KeyDown {
                    keycode: Some(Keycode::V),
                    keymod,
                    ..
                } if editing == Edit::Search
                    && keymod.intersects(
                        Mod::LCTRLMOD | Mod::RCTRLMOD | Mod::LGUIMOD | Mod::RGUIMOD,
                    ) =>
                {
                    if let Ok(text) = video.clipboard().clipboard_text() {
                        settings.add_search_text(text.trim());
                    }
                }
                Event::KeyDown {
                    keycode: Some(Keycode::Backspace | Keycode::Delete),
                    ..
                } if editing == Edit::Search => {
                    settings.search.pop();
                    settings.page = 0;
                }
                Event::TextInput { text, .. } if editing == Edit::Search => {
                    settings.add_search_text(&text)
                }
                Event::TextInput { text, .. } if editing == Edit::Address => {
                    edit_address(&mut settings.address, &mut address_selected, &text);
                }
                Event::DropFile { filename, .. } if pending.is_none() => {
                    let path = PathBuf::from(filename);
                    if path
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
                    {
                        if let Some(game) = settings.catalog.iter().position(|entry| {
                            entry.rom == path
                                || path
                                    .file_stem()
                                    .is_some_and(|stem| stem.eq_ignore_ascii_case(&entry.id))
                        }) {
                            settings.select_game(game);
                            settings.search.clear();
                            settings.page = game / PAGE_SIZE;
                        }
                        settings.rom = path;
                    } else {
                        settings.save = Some(path);
                        settings.stage = None;
                    }
                }
                Event::MouseWheel { y, .. } if y != 0 && pending.is_none() => {
                    settings.change_page(y < 0)
                }
                Event::MouseButtonDown {
                    mouse_btn: MouseButton::Left,
                    x,
                    y,
                    ..
                } => {
                    let point = Point::new(x, y);
                    if bounds(LANGUAGE).contains_point(point) {
                        settings.language = settings.language.toggled();
                        *status = settings
                            .language
                            .save()
                            .err()
                            .map_or_else(String::new, |error| error.to_string());
                    } else if bounds(START).contains_point(point) {
                        start = true;
                    } else if pending.is_none() {
                        editing = if bounds(SEARCH).contains_point(point) {
                            Edit::Search
                        } else if settings.mode != Mode::Local
                            && bounds(ADDRESS).contains_point(point)
                        {
                            Edit::Address
                        } else {
                            Edit::None
                        };
                        address_selected = editing == Edit::Address;
                        if editing != Edit::None {
                            video.text_input().start();
                        } else {
                            video.text_input().stop();
                        }
                        if bounds(PREVIOUS).contains_point(point) {
                            settings.change_page(false);
                        } else if bounds(NEXT).contains_point(point) {
                            settings.change_page(true);
                        } else if let Some((_, &index)) = settings
                            .filtered()
                            .iter()
                            .skip(settings.page * PAGE_SIZE)
                            .take(PAGE_SIZE)
                            .enumerate()
                            .find(|(row, _)| game_rect(*row).contains_point(point))
                        {
                            settings.select_game(index);
                            status.clear();
                        } else if bounds(ROM).contains_point(point) {
                            if let Some(path) = open_file(
                                settings.language.tr("Open arcade ROM"),
                                &settings.rom,
                                Some((&["*.zip"], settings.language.tr("ROM archive"))),
                            ) {
                                settings.rom = path;
                            }
                        } else if bounds(SAVE).contains_point(point) {
                            if x < 683 {
                                pick_stage = true;
                                break;
                            } else if x >= CLEAR_COLUMN {
                                settings.stage = None;
                                settings.save = None;
                            } else {
                                let default = settings.default_save();
                                if let Some(path) = open_file(
                                    settings.language.tr("Open KWK4 state"),
                                    settings.save.as_deref().unwrap_or(&default),
                                    Some((
                                        &["*.ks?", "*.kwk", "*.kwk4"],
                                        settings.language.tr("KWK4 save"),
                                    )),
                                ) {
                                    settings.stage = None;
                                    settings.save = Some(path);
                                }
                            }
                        } else if settings.entry().driver == Some("kof97")
                            && bounds(BIOS).contains_point(point)
                        {
                            choose_optional(
                                x,
                                &mut settings.bios,
                                settings.language.tr("Open Neo Geo BIOS"),
                                &settings.rom.with_file_name("neogeo.zip"),
                                Some((&["*.zip"], settings.language.tr("BIOS archive"))),
                            );
                        } else if bounds(INPUT).contains_point(point) {
                            if x < 683 {
                                configure_pad = true;
                                break;
                            }
                            choose_optional(
                                x,
                                &mut settings.input,
                                settings.language.tr("Open input configuration"),
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
                editing = Edit::None;
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
        if pick_stage {
            let default = settings.default_save();
            let game = &settings.entry().id;
            let directory = default.parent().unwrap_or_else(|| Path::new("."));
            match stage_picker::run(&sdl, &mut events, game, directory, settings.language) {
                Ok(Some(start)) => {
                    settings.stage = None;
                    settings.save = None;
                    match start {
                        stage_picker::Start::Default => {}
                        stage_picker::Start::Stage(index) => settings.stage = Some(index),
                        stage_picker::Start::Checkpoint(path) => settings.save = Some(path),
                    }
                    status.clear();
                }
                Ok(None) => {}
                Err(error) => *status = error.to_string(),
            }
            dirty = true;
        }
        if configure_pad {
            match pad_setup::run(
                &sdl,
                &mut events,
                settings.input.as_deref(),
                settings.language,
            ) {
                Ok(Some(path)) => {
                    settings.input = Some(path);
                    *status = "Joystick settings saved and applied".to_owned();
                }
                Ok(None) => {}
                Err(error) => *status = error.to_string(),
            }
            dirty = true;
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
    Rect::new(
        32 + (index % 4) as i32 * 191,
        94 + (index / 4) as i32 * 54,
        183,
        49,
    )
}

fn draw(
    canvas: &mut Canvas<Window>,
    settings: &Settings,
    status: &str,
    pending: bool,
    editing: Edit,
) -> Result<(), String> {
    canvas.set_draw_color(Color::RGB(14, 23, 34));
    canvas.clear();
    text(canvas, 32, 26, "KAWAKS RUST", 3, INK, 32)?;
    let lang = settings.language;
    let language_button = bounds(LANGUAGE);
    box_color(canvas, language_button, PANEL)?;
    text(
        canvas,
        690,
        33,
        if lang == Language::English {
            "中文"
        } else {
            "English"
        },
        1,
        ACCENT,
        12,
    )?;
    text(canvas, 320, 33, settings.entry().title(lang), 1, INK, 42)?;
    text(canvas, 32, 68, lang.tr("ROM LIBRARY"), 1, ACCENT, 16)?;
    box_color(
        canvas,
        bounds(SEARCH),
        if editing == Edit::Search {
            Color::RGB(46, 83, 105)
        } else {
            PANEL
        },
    )?;
    text(
        canvas,
        236,
        68,
        if settings.search.is_empty() {
            lang.tr("Search name or system")
        } else {
            &settings.search
        },
        1,
        if settings.search.is_empty() {
            MUTED
        } else {
            INK
        },
        37,
    )?;
    let filtered = settings.filtered();
    let pages = filtered.len().div_ceil(PAGE_SIZE).max(1);
    text(
        canvas,
        558,
        68,
        &format!("{}/{}  [{}]", settings.page + 1, pages, filtered.len()),
        1,
        MUTED,
        16,
    )?;
    for (button, label) in [(PREVIOUS, "<"), (NEXT, ">")] {
        let rect = bounds(button);
        box_color(canvas, rect, PANEL)?;
        text(canvas, rect.x() + 16, 68, label, 1, ACCENT, 1)?;
    }
    if filtered.is_empty() {
        text(canvas, 32, 112, lang.tr("No matching ROMs"), 1, MUTED, 24)?;
    }
    for (row, &index) in filtered
        .iter()
        .skip(settings.page * PAGE_SIZE)
        .take(PAGE_SIZE)
        .enumerate()
    {
        let entry = &settings.catalog[index];
        let rect = game_rect(row);
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
            entry.title(lang),
            1,
            if entry.driver.is_some() { INK } else { MUTED },
            20,
        )?;
        let detail = if !entry.present {
            lang.tr("ROM MISSING").to_owned()
        } else if entry.driver.is_none() {
            format!("{} / {}", entry.id, lang.tr("N/A"))
        } else {
            format!("{} / {}", entry.id, entry.platform)
        };
        text(canvas, rect.x() + 9, rect.y() + 31, &detail, 1, MUTED, 20)?;
    }
    let bios_enabled = settings.entry().driver == Some("kof97");
    let stage = settings.stage.and_then(|index| {
        kawaks_rs::Dino::starting_stages(&settings.entry().id)
            .get(usize::from(index))
            .copied()
    });
    let rows = [
        (ROM, "ROM", Some(settings.rom.as_path()), "Choose ZIP", true),
        (
            SAVE,
            "STARTING STAGE",
            settings.save.as_deref(),
            stage.unwrap_or("Normal start"),
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
            "INPUT CONFIG",
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
            lang.tr(label),
            1,
            if enabled { ACCENT } else { MUTED },
            16,
        )?;
        let display = path.map(|path| path.display().to_string());
        text(
            canvas,
            174,
            rect.y() + 16,
            display.as_deref().unwrap_or_else(|| lang.tr(placeholder)),
            1,
            INK,
            if region == INPUT || region == SAVE {
                48
            } else {
                61
            },
        )?;
        if enabled {
            if region == INPUT {
                text(
                    canvas,
                    580,
                    rect.y() + 16,
                    lang.tr("CONFIGURE"),
                    1,
                    ACCENT,
                    9,
                )?;
            } else if region == SAVE {
                text(canvas, 580, rect.y() + 16, lang.tr("CHOOSE"), 1, ACCENT, 9)?;
            }
            text(canvas, 699, rect.y() + 16, lang.tr("OPEN"), 1, ACCENT, 8)?;
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
        text(
            canvas,
            rect.x() + 8,
            rect.y() + 12,
            lang.tr(name),
            1,
            INK,
            10,
        )?;
    }
    if settings.mode != Mode::Local {
        box_color(
            canvas,
            bounds(ADDRESS),
            if editing == Edit::Address {
                Color::RGB(46, 83, 105)
            } else {
                PANEL
            },
        )?;
        text(canvas, 311, 486, &settings.address, 1, INK, 39)?;
    }
    box_color(canvas, bounds(AUDIO), PANEL)?;
    text(
        canvas,
        658,
        486,
        lang.tr(if settings.mute {
            "AUDIO OFF"
        } else {
            "AUDIO ON"
        }),
        1,
        INK,
        14,
    )?;
    box_color(
        canvas,
        bounds(START),
        if pending || settings.entry().driver.is_some() {
            ACCENT
        } else {
            MUTED
        },
    )?;
    text(
        canvas,
        621,
        555,
        lang.tr(if pending {
            "CANCEL"
        } else if settings.entry().driver.is_some() {
            "PLAY"
        } else {
            "N/A"
        }),
        2,
        Color::RGB(14, 23, 34),
        10,
    )?;
    for (index, line) in wrap(lang.tr(status), 67).iter().take(3).enumerate() {
        text(canvas, 32, 543 + index as i32 * 17, line, 1, MUTED, 67)?;
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
    let advance = |ch| {
        if i18n::wide_glyph(ch).is_some() {
            14
        } else {
            8
        }
    };
    let width = max * 8;
    let clipped = value.chars().map(advance).sum::<usize>() > width;
    let suffix = if clipped && width >= 24 { "..." } else { "" };
    let limit = width - suffix.len() * 8;
    let mut pos = 0;
    let glyphs = value
        .chars()
        .take_while(|&ch| {
            pos += advance(ch);
            pos <= limit
        })
        .chain(suffix.chars());
    let mut pixels = Vec::new();
    let mut offset = 0;
    for ch in glyphs {
        if let Some(bitmap) = i18n::wide_glyph(ch) {
            for (row, pair) in bitmap.chunks_exact(2).enumerate() {
                let bits = u16::from_le_bytes([pair[0], pair[1]]);
                for column in 0..14 {
                    if bits & (1 << column) != 0 {
                        pixels.push(Rect::new(
                            x + (offset + column) * scale,
                            y + (row as i32 - 3) * scale,
                            scale as u32,
                            scale as u32,
                        ));
                    }
                }
            }
        } else if let Some(bitmap) = BASIC_FONTS.get(ch).or_else(|| BASIC_FONTS.get('?')) {
            for (row, bits) in bitmap.into_iter().enumerate() {
                for column in 0..8 {
                    if bits & (1 << column) != 0 {
                        pixels.push(Rect::new(
                            x + (offset + column) * scale,
                            y + row as i32 * scale,
                            scale as u32,
                            scale as u32,
                        ));
                    }
                }
            }
        }
        offset += advance(ch) as i32;
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
        draw(&mut canvas, &settings, "", false, Edit::None).unwrap();
        let rgb = canvas
            .read_pixels(None, sdl2::pixels::PixelFormatEnum::RGB24)
            .unwrap();
        let pixel = |x, y| {
            let offset = (y * WIDTH as usize + x) * 3;
            &rgb[offset..offset + 3]
        };
        assert_eq!(
            pixel(game_rect(5).x() as usize + 3, game_rect(5).y() as usize + 4),
            &[46, 83, 105]
        );
        assert_eq!(pixel(610, 543), &[146, 225, 240]);
    }
}
