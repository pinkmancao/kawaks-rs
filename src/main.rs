use std::{
    env,
    ffi::OsString,
    fs::{File, OpenOptions},
    io::{self, BufWriter, Read},
    path::PathBuf,
    process::ExitCode,
};

#[cfg(feature = "sdl2")]
mod frontend;
#[cfg(feature = "sdl2")]
mod gui;
mod netplay;

use kawaks_rs::{
    write_png, ArcadeMachine, Dino, DinoRoms, FrameInput, Kwk4, NeoGeo, NeoKwk4, NeoRoms, WavWriter,
};

const USAGE: &str =
    "Usage: kawaks-rs --gui | kawaks-rs --rom PATH [--frames COUNT] [--load-state PATH] [--png PATH] [--wav PATH] [--game dino|wof|wofa|punisher|ssf2|kof97] [--bios PATH] [--window] [--input-config PATH] [--mute] [--net-listen IP:PORT | --net-connect IP:PORT]";

fn main() -> ExitCode {
    let result = run(env::args_os().skip(1));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("kawaks-rs: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: impl Iterator<Item = OsString>) -> io::Result<()> {
    let mut args = args.peekable();
    if args.peek().is_some_and(|arg| arg == "--gui") {
        args.next();
        if args.next().is_some() {
            return Err(bad_argument("--gui takes its options in the launcher"));
        }
        #[cfg(feature = "sdl2")]
        return gui::run();
        #[cfg(not(feature = "sdl2"))]
        return Err(bad_argument("--gui requires a build with --features sdl2"));
    }
    #[cfg(feature = "sdl2")]
    if args.peek().is_none() {
        return gui::run();
    }
    let mut rom = None;
    let mut bios = None;
    let mut load_state = None;
    let mut frames = None;
    let mut game = String::from("dino");
    let mut window = false;
    let mut mute = false;
    let mut input_config = None;
    let mut net_listen = None;
    let mut net_connect = None;
    let mut png = None;
    let mut wav = None;
    while let Some(flag) = args.next() {
        let arg = match flag.to_str() {
            Some("--help" | "-h") => {
                println!("{USAGE}");
                return Ok(());
            }
            Some("--window") => {
                window = true;
                continue;
            }
            Some("--mute") => {
                mute = true;
                continue;
            }
            Some(
                "--rom" | "--load-state" | "--frames" | "--png" | "--wav" | "--game" | "--bios"
                | "--input-config" | "--net-listen" | "--net-connect",
            ) => flag.to_str().unwrap(),
            _ => return Err(bad_argument(format!("unknown option {:?}\n{USAGE}", flag))),
        };
        let value = args
            .next()
            .ok_or_else(|| bad_argument(format!("{arg} requires a value")))?;
        match arg {
            "--game" => {
                game = value
                    .into_string()
                    .map_err(|_| bad_argument("game must be UTF-8"))?
            }
            "--input-config" => input_config = Some(PathBuf::from(value)),
            "--net-listen" | "--net-connect" => {
                let address = value
                    .to_str()
                    .and_then(|s| s.parse::<std::net::SocketAddr>().ok())
                    .ok_or_else(|| {
                        bad_argument("netplay requires a numeric IP address and port")
                    })?;
                if arg == "--net-listen" {
                    net_listen = Some(address);
                } else {
                    net_connect = Some(address);
                }
            }
            "--bios" => bios = Some(PathBuf::from(value)),
            "--rom" => rom = Some(PathBuf::from(value)),
            "--load-state" => load_state = Some(PathBuf::from(value)),
            "--png" => png = Some(PathBuf::from(value)),
            "--wav" => wav = Some(PathBuf::from(value)),
            "--frames" => {
                frames = Some(
                    value
                        .to_str()
                        .and_then(|s| s.parse::<u64>().ok())
                        .ok_or_else(|| {
                            bad_argument("frame count must be a non-negative integer")
                        })?,
                );
            }
            _ => unreachable!(),
        }
    }
    if net_listen.is_some() && net_connect.is_some() {
        return Err(bad_argument("choose one netplay role"));
    }
    if !window && (mute || input_config.is_some()) {
        return Err(bad_argument("--mute and --input-config require --window"));
    }
    if window && (png.is_some() || wav.is_some()) {
        return Err(bad_argument("use headless mode to export PNG or WAV"));
    }
    if window && !cfg!(feature = "sdl2") {
        return Err(bad_argument(
            "--window requires a build with --features sdl2",
        ));
    }
    let rom = rom.ok_or_else(|| bad_argument(USAGE))?;
    let (mut machine, rom_crc) = load_machine(&game, rom, bios, load_state)?;
    let [main, graphics, z80, samples] = rom_crc;
    println!(
        "ROM crc32 main={main:08x} graphics={graphics:08x} z80={z80:08x} samples={samples:08x}"
    );
    if png.is_some() && png == wav {
        return Err(bad_argument("PNG and WAV require different paths"));
    }
    let mut report = machine.report();
    let mut peer = match (net_listen, net_connect) {
        (Some(address), None) => Some(netplay::Peer::listen(address, rom_crc, report.state_crc32)?),
        (None, Some(address)) => Some(netplay::Peer::connect(
            address,
            rom_crc,
            report.state_crc32,
        )?),
        _ => None,
    };
    #[cfg(feature = "sdl2")]
    if window {
        frontend::run(
            machine.as_mut(),
            peer.as_mut(),
            input_config.as_deref(),
            frames,
            mute,
            &game,
        )?;
        print_report(machine.report());
        return Ok(());
    }
    let mut wav = wav
        .map(create_output)
        .transpose()?
        .map(WavWriter::new)
        .transpose()?;
    for _ in 0..frames.unwrap_or(1) {
        let input = if let Some(peer) = &mut peer {
            peer.exchange(0, report)?
        } else {
            FrameInput::default()
        };
        report = machine.step_frame(input)?;
        if let Some(wav) = &mut wav {
            wav.write_samples(machine.frame_audio())?;
        }
    }
    if let Some(wav) = wav {
        wav.finish()?;
    }
    if let Some(path) = png {
        write_png(create_output(path)?, &machine.render_rgb())?;
    }
    print_report(report);
    Ok(())
}

fn load_machine(
    game: &str,
    rom: PathBuf,
    bios: Option<PathBuf>,
    load_state: Option<PathBuf>,
) -> io::Result<(Box<dyn ArcadeMachine>, [u32; 4])> {
    let saved = load_state.map(read_save).transpose()?;
    Ok(if game == "kof97" {
        let bios = bios.unwrap_or_else(|| rom.with_file_name("neogeo.zip"));
        let roms = NeoRoms::load_kof97(rom, bios)?;
        let crc = roms.region_crc32();
        let neo = if let Some(bytes) = saved {
            let state = NeoKwk4::read(&bytes)?;
            print_saved_pcs(state.main_pc(), state.sound_pc());
            NeoGeo::from_kwk4(roms, state)?
        } else {
            NeoGeo::new(roms)
        };
        (Box::new(neo), crc)
    } else {
        if bios.is_some() {
            return Err(bad_argument("--bios is for Neo Geo"));
        }
        let roms = DinoRoms::load_game(game, rom)?;
        let crc = roms.region_crc32();
        let cps = if let Some(bytes) = saved {
            let state = if game == "ssf2" {
                Kwk4::read_cps2(&bytes)?
            } else {
                Kwk4::read(&bytes)?
            };
            print_saved_pcs(state.main_pc(), state.sound_pc());
            Dino::from_kwk4(roms, state)?
        } else {
            Dino::new(roms)
        };
        (Box::new(cps), crc)
    })
}

fn read_save(path: PathBuf) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 1024 * 1024 {
        return Err(bad_argument("KWK4 file exceeds 1 MiB"));
    }
    Ok(bytes)
}

fn print_saved_pcs(main: u32, sound: u16) {
    println!("KWK4 imported main_pc={main:06x} z80_pc={sound:04x}");
}

fn print_report(report: kawaks_rs::FrameReport) {
    println!(
        "frame={} main_cycles={} z80_cycles={} main_pc={:06x} z80_pc={:04x} ram={:08x} vram={:08x} qsound_commands={} state={:08x}",
        report.frame, report.main_cycles, report.sound_cycles, report.main_pc, report.sound_pc,
        report.work_crc32, report.graphics_ram_crc32, report.qsound_commands, report.state_crc32,
    );
}

fn create_output(path: PathBuf) -> io::Result<BufWriter<File>> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map(BufWriter::new)
        .map_err(|error| io::Error::new(error.kind(), format!("{}: {error}", path.display())))
}

fn bad_argument(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}
