mod legacy_adpcm;
mod legacy_fm;
mod legacy_psg;
mod legacy_sound;

use std::{io, path::Path};

use emu198x_yamaha_ym2610::Ym2610;
use r68k::{
    cpu::{ConfiguredCore, ProcessingState},
    interrupts::AutoInterruptController,
    ram::{AddressBus, AddressSpace},
};
use z80emu::{
    host::{BreakCause, TsCounter},
    Cpu, CpuDebug, InterruptMode, Io, Memory, Prefix, StkReg16, Z80NMOS,
};

use crate::{
    invalid, restore_main,
    rom::{Part, RomArchive},
    save::{le16, le32, swap_words, Reader},
    FrameInput, FrameReport, AUDIO_SAMPLES_PER_FRAME, FRAME_HEIGHT, FRAME_WIDTH,
};

pub struct NeoRoms {
    program: Vec<u8>,
    sprites: Vec<u8>,
    fixed: Vec<u8>,
    sound: Vec<u8>,
    samples: Vec<u8>,
    bios: Vec<u8>,
    system_fixed: Vec<u8>,
    zoom: Vec<u8>,
}

impl NeoRoms {
    pub fn load_kof97(cart: impl AsRef<Path>, bios: impl AsRef<Path>) -> io::Result<Self> {
        let mut cart = RomArchive::open("kof97", cart)?;
        let mut system = RomArchive::open("Neo Geo BIOS", bios)?;
        Ok(Self {
            program: cart.build(
                &[
                    Part::word("232-p1.p1", 0x7db81ad9, 0, 0x100000),
                    Part::word("232-p2.sp2", 0x158b23f6, 0x100000, 0x400000),
                ],
                0x500000,
            )?,
            sprites: cart.build(
                &[
                    Part::byte("232-c1.c1", 0x5f8bf0a1, 0, 0x800000),
                    Part::byte("232-c2.c2", 0xe4d45c81, 1, 0x800000),
                    Part::byte("232-c3.c3", 0x581d6618, 0x1000000, 0x800000),
                    Part::byte("232-c4.c4", 0x49bb1e68, 0x1000001, 0x800000),
                    Part::byte("232-c5.c5", 0x34fc4e51, 0x2000000, 0x400000),
                    Part::byte("232-c6.c6", 0x4ff4d47b, 0x2000001, 0x400000),
                ],
                0x2800000,
            )?,
            fixed: cart.build(
                &[Part::linear("232-s1.s1", 0x8514ecf5, 0, 0x20000)],
                0x20000,
            )?,
            sound: cart.build(
                &[Part::linear("232-m1.m1", 0x45348747, 0, 0x20000)],
                0x20000,
            )?,
            samples: cart.build(
                &[
                    Part::linear("232-v1.v1", 0x22a2b5b5, 0, 0x400000),
                    Part::linear("232-v2.v2", 0x2304e744, 0x400000, 0x400000),
                    Part::linear("232-v3.v3", 0x759eb954, 0x800000, 0x400000),
                ],
                0xc00000,
            )?,
            bios: system.build(&[Part::word("sp-s2.sp1", 0x9036d879, 0, 0x20000)], 0x20000)?,
            system_fixed: system.build(
                &[Part::linear("sfix.sfix", 0xc2ea0cfd, 0, 0x20000)],
                0x20000,
            )?,
            zoom: system.build(
                &[Part::linear("000-lo.lo", 0x5a86cff2, 0, 0x20000)],
                0x20000,
            )?,
        })
    }

    pub fn region_crc32(&self) -> [u32; 4] {
        let combined = |chunks: &[&[u8]]| {
            let mut hash = crc32fast::Hasher::new();
            for chunk in chunks {
                hash.update(chunk);
            }
            hash.finalize()
        };
        [
            combined(&[&self.program, &self.bios]),
            combined(&[&self.sprites, &self.fixed, &self.system_fixed, &self.zoom]),
            crc32fast::hash(&self.sound),
            crc32fast::hash(&self.samples),
        ]
    }
}

pub struct NeoKwk4 {
    cpu: [u8; 0x80],
    z80: [u8; 0x31],
    work: Vec<u8>,
    video: Vec<u8>,
    backup: Vec<u8>,
    palette: Vec<u8>,
    card: Vec<u8>,
    audio_ram: Vec<u8>,
    registers: [u8; 0x19],
    legacy_fm: Vec<u8>,
    legacy_timers: [u8; 0x14],
    legacy_psg: [u8; 0xf8],
    tail: [u8; 0x3d],
}

impl NeoKwk4 {
    pub fn read(bytes: &[u8]) -> io::Result<Self> {
        let mut reader = Reader(bytes);
        if reader.take(4, "signature")? != b"KWK4" {
            return Err(invalid("expected KWK4"));
        }
        let state = Self {
            cpu: reader.fixed("Neo Geo 68000")?,
            z80: reader.fixed("Neo Geo Z80")?,
            work: reader.chunk(0x10000, "Neo Geo work RAM")?,
            video: reader.chunk(0x20000, "Neo Geo video RAM")?,
            backup: reader.chunk(0x10000, "Neo Geo backup RAM")?,
            palette: reader.chunk(0x4000, "Neo Geo palette banks")?,
            card: reader.chunk(0x1000, "Neo Geo memory card")?,
            audio_ram: reader.chunk(0x800, "Neo Geo Z80 RAM")?,
            registers: reader.fixed("Neo Geo video and I/O registers")?,
            legacy_timers: reader.fixed("Neo Geo sound timers")?,
            legacy_fm: reader.chunk(0x3420, "Neo Geo YM2610")?,
            legacy_psg: reader.fixed("Neo Geo PSG")?,
            tail: reader.fixed("Neo Geo driver tail")?,
        };
        if !reader.0.is_empty() {
            return Err(invalid("unexpected bytes after Neo Geo KWK4"));
        }
        Ok(state)
    }
    pub fn main_pc(&self) -> u32 {
        le32(&self.cpu, 0x50)
    }
    pub fn sound_pc(&self) -> u16 {
        le16(&self.z80, 12)
    }
}

struct Calendar {
    lines: u8,
    command: u8,
    mode: u8,
    shift: u64,
    ticks: u64,
    pulse_half_period: u64,
    pulse_epoch: u64,
}

impl Calendar {
    fn new() -> Self {
        Self {
            lines: 0,
            command: 0,
            mode: 0,
            shift: 0,
            ticks: 0,
            pulse_half_period: 12_000_000 / 128,
            pulse_epoch: 0,
        }
    }

    fn write(&mut self, value: u8) {
        let rising = value & !self.lines;
        if rising & 2 != 0 {
            let bit = self.command & 1;
            self.command = (self.command >> 1) | ((value & 1) << 3);
            if self.mode == 1 {
                self.shift = (self.shift >> 1) | (u64::from(bit) << 47);
            }
        }
        if rising & 4 != 0 {
            self.mode = self.command;
            match self.mode {
                3 => {
                    // A deterministic calendar epoch keeps peers synchronized.
                    let seconds = self.ticks / 12_000_000;
                    let bcd = |n| (n / 10 * 16) | (n % 10);
                    self.shift = bcd(seconds % 60)
                        | (bcd(seconds / 60 % 60) << 8)
                        | (bcd(seconds / 3600 % 24) << 16)
                        | (0x24_12_01 << 24);
                }
                4..=11 => {
                    self.pulse_half_period = match self.mode {
                        4..=7 => {
                            12_000_000 / (2 * [64, 256, 2048, 4096][usize::from(self.mode - 4)])
                        }
                        _ => 6_000_000 * [1, 10, 30, 60][usize::from(self.mode - 8)],
                    };
                    self.pulse_epoch = self.ticks;
                }
                _ => {}
            }
        }
        self.lines = value;
    }

    fn pins(&self) -> u8 {
        let tp = (((self.ticks - self.pulse_epoch) / self.pulse_half_period) & 1) as u8;
        let data = match self.mode {
            1 | 2 => self.shift & 1,
            15 => self.ticks * 8192 / 12_000_000 & 1,
            _ => self.ticks * 2 / 12_000_000 & 1,
        } as u8;
        (data << 7) | (tp << 6)
    }
}

struct RestoredAudio {
    adpcm: legacy_adpcm::LegacyAdpcm,
    timers: legacy_sound::Timers,
    samples: Vec<[i16; 2]>,
    output_samples: u64,
    fm: legacy_fm::LegacyFm,
    psg: legacy_psg::LegacyPsg,
    #[cfg(test)]
    voice_trace: Vec<[[i32; 4]; 7]>,
    #[cfg(test)]
    source_trace: Vec<([i32; 2], [i32; 3])>,
}

impl RestoredAudio {
    fn render(&mut self, count: usize, rom: &[u8]) {
        if count > AUDIO_SAMPLES_PER_FRAME - self.samples.len() {
            return;
        }
        self.psg.begin_chunk(count);
        for _ in 0..count {
            let fm_pan = self.fm.sample();
            let mut pan = fm_pan.map(i64::from);
            let lanes = self.adpcm.sample(rom);
            #[cfg(test)]
            self.voice_trace.push(lanes);
            for lane in lanes {
                for (sum, value) in pan.iter_mut().zip(lane) {
                    *sum += i64::from(value);
                }
            }
            let stereo = [pan[2] + pan[3], pan[1] + pan[3]];
            let [a, b, c] = self.psg.sample();
            #[cfg(test)]
            self.source_trace.push((
                [
                    fm_pan[2].wrapping_add(fm_pan[3]) >> 13,
                    fm_pan[1].wrapping_add(fm_pan[3]) >> 13,
                ],
                [a, b, c],
            ));
            let ssg = [(a + b) / 2, (c + b) / 2];
            self.samples.push(std::array::from_fn(|side| {
                let output = stereo[side].clamp(-0x1000_0000, 0x0fff_ffff) >> 13;
                let mixed = output + i64::from(ssg[side]);
                if mixed < -32768 {
                    -32767
                } else {
                    mixed.min(32767) as i16
                }
            }));
        }
        self.output_samples += count as u64;
    }
}

struct NeoBus {
    roms: std::sync::Arc<NeoRoms>,
    work: Vec<u8>,
    video: Vec<u8>,
    backup: Vec<u8>,
    palette: Vec<u8>,
    card: Vec<u8>,
    audio_ram: Vec<u8>,
    input: FrameInput,
    program_bank: usize,
    palette_bank: usize,
    cart_vectors: bool,
    cart_fixed: bool,
    backup_enabled: bool,
    dark: bool,
    scanline: u16,
    calendar: Calendar,
    video_pointer: u16,
    video_stride: u16,
    video_mode: u16,
    animation: u8,
    animation_step: u16,
    irq_counter: u32,
    z80_banks: [u8; 4],
    latch: u8,
    reply: u8,
    pending_nmi: bool,
    nmi_enabled: bool,
    ym: Ym2610,
    ym_addresses: [u8; 2],
    restored_audio: Option<RestoredAudio>,
    ym_write_hash: crc32fast::Hasher,
    sound_commands: u64,
}

impl NeoBus {
    fn new(roms: NeoRoms) -> Self {
        Self {
            roms: std::sync::Arc::new(roms),
            work: vec![0; 0x10000],
            video: vec![0; 0x20000],
            backup: vec![0; 0x10000],
            palette: vec![0; 0x4000],
            card: vec![0xff; 0x1000],
            audio_ram: vec![0; 0x800],
            input: FrameInput::default(),
            program_bank: 1,
            palette_bank: 0,
            cart_vectors: false,
            cart_fixed: false,
            backup_enabled: false,
            dark: false,
            scanline: 0,
            calendar: Calendar::new(),
            video_pointer: 0,
            video_stride: 0,
            video_mode: 0,
            animation: 0,
            animation_step: 0,
            irq_counter: 0,
            z80_banks: [0x1e, 0x0e, 0x06, 0x02],
            latch: 0,
            reply: 0,
            pending_nmi: false,
            nmi_enabled: false,
            ym: Ym2610::new(),
            ym_addresses: [0; 2],
            restored_audio: None,
            ym_write_hash: crc32fast::Hasher::new(),
            sound_commands: 0,
        }
    }

    fn restore_legacy_sound(&mut self, fm: &[u8], timers: &[u8; 0x14], psg: &[u8; 0xf8]) {
        self.restored_audio = Some(RestoredAudio {
            adpcm: legacy_adpcm::LegacyAdpcm::restore(fm),
            timers: legacy_sound::Timers::restore(fm, timers),
            samples: Vec::with_capacity(AUDIO_SAMPLES_PER_FRAME),
            output_samples: 0,
            fm: legacy_fm::LegacyFm::restore(fm),
            psg: legacy_psg::LegacyPsg::restore(psg),
            #[cfg(test)]
            voice_trace: Vec::new(),
            #[cfg(test)]
            source_trace: Vec::new(),
        });

        for (register, &value) in psg[0x1c..0x2a].iter().enumerate() {
            self.ym.ssg.select_register(register as u8);
            self.ym.ssg.write_data(value);
        }
        self.ym.ssg.select_register(psg[0x18] & 15);
        self.ym_addresses = [fm[0x28], fm[0x3188]];
        self.ym.write(0, self.ym_addresses[0]);
    }

    fn select_ym_address(&mut self, bank: usize) {
        let address = self.ym_addresses[bank];
        if self.ym.address != ((bank as u16) << 8) | u16::from(address) {
            self.ym.write((bank * 2) as u8, address);
        }
    }

    fn word(memory: &[u8], offset: usize) -> u16 {
        u16::from_be_bytes([memory[offset], memory[offset + 1]])
    }

    fn vram(&self, word: usize) -> u16 {
        Self::word(&self.video, (word & 0xffff) * 2)
    }
    fn video_read(&self, address: u32) -> u16 {
        match address & 6 {
            0 | 2 => self.vram(usize::from(self.video_pointer)),
            4 => self.video_stride,
            6 => ((self.scanline + 0xf8) << 7) | u16::from(self.animation & 7),
            _ => unreachable!(),
        }
    }

    fn video_write(&mut self, address: u32, value: u16) {
        match address & 14 {
            0 => {
                self.video_pointer = if value & 0x8000 != 0 {
                    value & 0x87ff
                } else {
                    value
                }
            }
            2 => {
                let offset = usize::from(self.video_pointer) * 2;
                self.video[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
                self.video_pointer = (self.video_pointer & 0x8000)
                    | (self.video_pointer.wrapping_add(self.video_stride) & 0x7fff);
                if self.video_pointer & 0x8000 != 0 {
                    self.video_pointer &= 0x87ff;
                }
            }
            4 => self.video_stride = value,
            6 => self.video_mode = value,
            8 => self.irq_counter = (self.irq_counter & 0xffff) | u32::from(value) << 16,
            10 => self.irq_counter = (self.irq_counter & 0xffff0000) | u32::from(value),
            _ => {}
        }
    }

    fn read8(&self, address: u32) -> u8 {
        let address = address & 0xff_ffff;
        let offset = address as usize;
        match address {
            0..=0x7f if !self.cart_vectors => self.roms.bios[offset],
            0..=0x0fffff => self.roms.program[offset],
            0x100000..=0x1fffff => self.work[offset & 0xffff],
            0x200000..=0x2fffff => {
                self.roms.program[(self.program_bank << 20) | (offset & 0xfffff)]
            }
            0x300000..=0x31ffff => {
                if address & 0x81 == 0 {
                    !(self.input.players[0] as u8)
                } else {
                    0xff
                }
            }
            0x320000..=0x33ffff => {
                if address & 1 == 0 {
                    self.reply
                } else {
                    let coins =
                        (self.input.players[0] >> 13 & 1) | (self.input.players[1] >> 12 & 2);
                    (0x1f | self.calendar.pins()) & !(coins as u8)
                }
            }
            0x340000..=0x35ffff if address & 1 == 0 => !(self.input.players[1] as u8),
            0x380000..=0x39ffff if address & 1 == 0 => {
                let starts =
                    (self.input.players[0] >> 12 & 1) | ((self.input.players[1] >> 12 & 1) << 2);
                0xff & !(starts as u8)
            }
            0x3c0000..=0x3dffff => self.video_read(address).to_be_bytes()[offset & 1],
            0x400000..=0x7fffff => self.palette[self.palette_bank * 0x2000 + (offset & 0x1fff)],
            0x800000..=0x800fff => self.card[offset & 0xfff],
            0xc00000..=0xcfffff => self.roms.bios[offset & 0x1ffff],
            0xd00000..=0xdfffff => self.backup[offset & 0xffff],
            _ => 0xff,
        }
    }

    fn write8(&mut self, address: u32, value: u8) {
        let address = address & 0xff_ffff;
        let offset = address as usize;
        match address {
            0x100000..=0x1fffff => self.work[offset & 0xffff] = value,
            0x2ffff0..=0x2fffff if address & 1 != 0 => {
                let bank = usize::from(value & 7) + 1;
                self.program_bank = if bank < 5 { bank } else { 1 };
            }
            0x320000..=0x33ffff if address & 1 == 0 => {
                self.latch = value;
                self.pending_nmi = true;
                self.sound_commands += 1;
            }
            0x380051 => self.calendar.write(value),
            0x3a0000..=0x3bffff if address & 1 != 0 => {
                let set = address & 0x10 != 0;
                match address & 0xe {
                    0 => self.dark = set,
                    2 => self.cart_vectors = set,
                    10 => self.cart_fixed = set,
                    12 => self.backup_enabled = set,
                    14 => self.palette_bank = usize::from(!set),
                    _ => {}
                }
            }
            0x400000..=0x7fffff => {
                self.palette[self.palette_bank * 0x2000 + (offset & 0x1fff)] = value
            }
            0xd00000..=0xdfffff if self.backup_enabled => {
                // WinKawaks 1.65 selects this KOF97 SRAM compatibility
                // latch at boot (42c52b), then filters writes at 42e2a0.
                if address != 0xd00101 || value != 1 {
                    self.backup[offset & 0xffff] = value;
                }
            }
            _ => {}
        }
    }
}

impl AddressBus for NeoBus {
    fn read_byte(&self, _: AddressSpace, address: u32) -> u32 {
        self.read8(address).into()
    }
    fn read_word(&self, _: AddressSpace, address: u32) -> u32 {
        u16::from_be_bytes([self.read8(address), self.read8(address.wrapping_add(1))]).into()
    }
    fn read_long(&self, space: AddressSpace, address: u32) -> u32 {
        self.read_word(space, address) << 16 | self.read_word(space, address.wrapping_add(2))
    }
    fn write_byte(&mut self, _: AddressSpace, address: u32, value: u32) {
        if matches!(address & 0xff_ffff, 0x3c0000..=0x3dffff) && address & 1 == 0 {
            self.video_write(address, u16::from(value as u8) * 0x0101);
        } else {
            self.write8(address, value as u8);
        }
    }
    fn write_word(&mut self, _: AddressSpace, address: u32, value: u32) {
        if matches!(address & 0xff_ffff, 0x3c0000..=0x3dffff) {
            self.video_write(address, value as u16);
        } else {
            let [upper, lower] = (value as u16).to_be_bytes();
            self.write8(address, upper);
            self.write8(address.wrapping_add(1), lower);
        }
    }
    fn write_long(&mut self, space: AddressSpace, address: u32, value: u32) {
        self.write_word(space, address, value >> 16);
        self.write_word(space, address.wrapping_add(2), value);
    }
    fn copy_from(&mut self, other: &Self) {
        // Used by r68k's context copy. A Neo Geo machine owns its sound chip
        // and currently does not expose context cloning.
        self.roms = other.roms.clone();
        self.work.clone_from(&other.work);
        self.video.clone_from(&other.video);
        self.backup.clone_from(&other.backup);
        self.palette.clone_from(&other.palette);
        self.card.clone_from(&other.card);
        self.audio_ram.clone_from(&other.audio_ram);
    }
}

impl Memory for NeoBus {
    type Timestamp = u64;
    fn read_debug(&self, address: u16) -> u8 {
        match address {
            0..=0x7fff => self.roms.sound[usize::from(address)],
            0xf800..=0xffff => self.audio_ram[usize::from(address & 0x7ff)],
            _ => {
                let region = match address {
                    0x8000..=0xbfff => 3,
                    0xc000..=0xdfff => 2,
                    0xe000..=0xefff => 1,
                    _ => 0,
                };
                let shift = 11 + region;
                let bank = (usize::from(self.z80_banks[region]) << shift) & 0x1ffff;
                self.roms.sound[bank | (usize::from(address) & ((1 << shift) - 1))]
            }
        }
    }
    fn write_mem(&mut self, address: u16, value: u8, _: u64) {
        if address >= 0xf800 {
            self.audio_ram[usize::from(address & 0x7ff)] = value;
        }
    }
}

impl Io for NeoBus {
    type Timestamp = u64;
    type WrIoBreak = ();
    type RetiBreak = ();

    fn read_io(&mut self, port: u16, _: u64) -> (u8, Option<std::num::NonZeroU16>) {
        let value = match port as u8 {
            0 => self.latch,
            5 => {
                self.select_ym_address(0);
                self.ym.read(1)
            }
            6 => self
                .restored_audio
                .as_ref()
                .map_or_else(|| self.ym.read(2), |audio| audio.adpcm.flags()),
            4 => self
                .restored_audio
                .as_ref()
                .map_or_else(|| self.ym.read(0), |audio| audio.timers.status),
            7 => self.ym.read(3),
            8..=11 | 0x18..=0x1b => {
                self.z80_banks[usize::from(port & 3)] = (port >> 8) as u8;
                0
            }
            _ => 0xff,
        };
        (value, None)
    }
    fn write_io(
        &mut self,
        port: u16,
        value: u8,
        _: u64,
    ) -> (Option<()>, Option<std::num::NonZeroU16>) {
        match port as u8 {
            0 => self.latch = 0,
            4..=7 => {
                let chip_port = port as u8 - 4;
                let bank = usize::from(chip_port / 2);
                if chip_port & 1 == 0 {
                    self.ym_addresses[bank] = value;
                } else {
                    self.select_ym_address(bank);
                    if let Some(audio) = &mut self.restored_audio {
                        let address = self.ym_addresses[bank];
                        if bank == 1 || address >= 0x10 {
                            audio.render(100, &self.roms.samples);
                        }
                        if bank == 0 {
                            audio.timers.write(address, value);
                            audio.psg.write(address, value);
                        }
                        audio.fm.write(bank, address, value);
                        audio
                            .adpcm
                            .write(bank, address, value, self.roms.samples.len());
                    }
                }
                self.ym.write(chip_port, value);
                self.ym_write_hash.update(&[port as u8, value]);
            }
            8 => self.nmi_enabled = true,
            0x18 => self.nmi_enabled = false,
            0x0c => self.reply = value,
            _ => {}
        }
        (None, None)
    }
    fn is_irq(&mut self, _: u64) -> bool {
        self.restored_audio
            .as_ref()
            .map_or_else(|| self.ym.irq(), |audio| audio.timers.irq())
    }
}

pub struct NeoGeo {
    main: ConfiguredCore<AutoInterruptController, NeoBus>,
    sound: Z80NMOS,
    sound_clock: TsCounter<u64>,
    main_cycles: u64,
    sound_deadline: u64,
    frame: u64,
    pcm: Vec<[i16; 2]>,
}

impl NeoGeo {
    pub fn new(roms: NeoRoms) -> Self {
        let mut main =
            ConfiguredCore::new_with(0, AutoInterruptController::new(), NeoBus::new(roms));
        main.reset();
        Self {
            main,
            sound: Z80NMOS::default(),
            sound_clock: TsCounter::from(0),
            main_cycles: 0,
            sound_deadline: 0,
            frame: 0,
            pcm: Vec::new(),
        }
    }

    pub fn from_kwk4(roms: NeoRoms, mut state: NeoKwk4) -> io::Result<Self> {
        let mut neo = Self::new(roms);
        restore_main(&mut neo.main, &state.cpu)?;
        for memory in [
            &mut state.work,
            &mut state.video,
            &mut state.backup,
            &mut state.palette,
        ] {
            swap_words(memory);
        }
        let bus = &mut neo.main.mem;
        bus.restore_legacy_sound(&state.legacy_fm, &state.legacy_timers, &state.legacy_psg);
        bus.work = state.work;
        bus.video = state.video;
        bus.backup = state.backup;
        bus.palette = state.palette;
        bus.card = state.card;
        bus.audio_ram = state.audio_ram;
        let regs = &state.registers;
        bus.video_stride = le16(regs, 0);
        bus.video_pointer = le16(regs, 2);
        bus.animation = le16(regs, 11) as u8;
        bus.video_mode = (le16(regs, 13) & 0xff) << 8;
        bus.animation_step = bus.video_mode >> 8;
        bus.backup_enabled = regs[10] == 0;
        bus.palette_bank = usize::from(le32(regs, 21) != 0);
        bus.program_bank = (usize::from(regs[16] & 7) + 1).min(4);
        bus.cart_vectors = true;
        bus.cart_fixed = true;
        bus.latch = state.z80[0x30];
        bus.reply = state.z80[0x2f];
        bus.nmi_enabled = true;
        for (region, offset) in [0x2c, 0x2a, 0x28, 0x26].into_iter().enumerate() {
            bus.z80_banks[region] = state.z80[offset + 1];
        }

        // Include the original chip records in the initial-state fingerprint.
        for bytes in [
            &state.legacy_fm[..],
            &state.legacy_timers,
            &state.legacy_psg,
            &state.tail,
        ] {
            bus.ym_write_hash.update(bytes);
        }
        let s = &state.z80;
        let pairs = [StkReg16::AF, StkReg16::BC, StkReg16::DE, StkReg16::HL];
        for (index, reg) in pairs.into_iter().enumerate() {
            neo.sound.set_reg16(reg, le16(s, 16 + 2 * index));
        }
        neo.sound.ex_af_af();
        neo.sound.exx();
        for (index, reg) in pairs.into_iter().enumerate() {
            neo.sound.set_reg16(reg, le16(s, index * 2));
        }
        neo.sound.set_index16(Prefix::Xdd, le16(s, 8));
        neo.sound.set_index16(Prefix::Yfd, le16(s, 10));
        neo.sound.set_pc(le16(s, 12));
        neo.sound.set_sp(le16(s, 14));
        neo.sound.set_iffs(le16(s, 24) != 0, le16(s, 26) != 0);
        let [r, i] = le16(s, 28).to_le_bytes();
        neo.sound.set_i(i);
        neo.sound.set_r(r);
        neo.sound.set_im(match le16(s, 30) {
            0 => InterruptMode::Mode0,
            1 => InterruptMode::Mode1,
            2 => InterruptMode::Mode2,
            _ => return Err(invalid("invalid Neo Geo Z80 interrupt mode")),
        });
        if le16(s, 36) != 0 {
            neo.sound.halt();
        }
        Ok(neo)
    }

    pub fn step_frame(&mut self, input: FrameInput) -> io::Result<FrameReport> {
        self.main.mem.input = input;
        #[cfg(test)]
        if let Some(audio) = &mut self.main.mem.restored_audio {
            audio.voice_trace.clear();
            audio.source_trace.clear();
        }
        let start = self.frame * 262 * 768;
        for scanline in 0..262 {
            self.main.mem.scanline = scanline;
            self.main.mem.calendar.ticks = start + u64::from(scanline) * 768;
            let end = start + (u64::from(scanline) + 1) * 768;
            if self.main_cycles < end {
                let used = self.main.execute((end - self.main_cycles) as i32).0;
                if used <= 0 || self.main.processing_state == ProcessingState::Halted {
                    return Err(invalid(format!(
                        "Neo Geo 68000 halted at {:06x}",
                        self.main.pc
                    )));
                }
                self.main_cycles += used as u64;
            }
            self.sound_deadline += 384;
            while self.sound_clock.0 .0 < self.sound_deadline {
                let bus = &mut self.main.mem;
                let before = self.sound_clock.0 .0;
                if bus.pending_nmi && bus.nmi_enabled && self.sound.nmi(bus, &mut self.sound_clock)
                {
                    bus.pending_nmi = false;
                } else if let Err(BreakCause::WriteIo(_) | BreakCause::Reti(_)) = self
                    .sound
                    .execute_next(bus, &mut self.sound_clock, None::<fn(CpuDebug)>)
                {
                    return Err(invalid("unexpected Neo Geo Z80 host break"));
                }
                // The legacy mixer advances by 367 samples at 22,050 Hz
                // per board frame. Convert that interval to 8 MHz chip clocks.
                let chip_clocks = |cycles| cycles * 229_375 / 173_313;
                if bus.restored_audio.is_none() {
                    for _ in chip_clocks(before)..chip_clocks(self.sound_clock.0 .0) {
                        bus.ym.tick(&bus.roms.samples, &bus.roms.samples);
                    }
                }
            }
            let bus = &mut self.main.mem;
            if let Some(audio) = &mut bus.restored_audio {
                if audio.timers.advance(384) {
                    audio.render(100, &bus.roms.samples);
                }
            }
        }
        let bus = &mut self.main.mem;
        self.frame += 1;
        self.main.int_ctrl.request_interrupt(1);
        if bus.animation_step >= bus.video_mode >> 8 {
            bus.animation_step = 0;
            bus.animation = bus.animation.wrapping_add(1);
        } else {
            bus.animation_step += 1;
        }
        self.pcm.clear();
        if let Some(audio) = &mut bus.restored_audio {
            audio.render(
                AUDIO_SAMPLES_PER_FRAME - audio.samples.len(),
                &bus.roms.samples,
            );
            self.pcm.append(&mut audio.samples);
            return Ok(self.report());
        }
        let samples = bus.ym.take_samples();
        for i in 0..AUDIO_SAMPLES_PER_FRAME {
            let sample = samples[i * samples.len() / AUDIO_SAMPLES_PER_FRAME];
            let ssg = (sample.ssg * 5000.0) as i32;
            self.pcm.push(
                sample
                    .fm
                    .map(|x| (i32::from(x) + ssg).clamp(-32768, 32767) as i16),
            );
        }
        Ok(self.report())
    }

    pub fn frame_audio(&self) -> &[[i16; 2]] {
        &self.pcm
    }

    pub fn report(&self) -> FrameReport {
        let bus = &self.main.mem;
        let mut hash = bus.ym_write_hash.clone();
        for bytes in [
            &bus.work,
            &bus.video,
            &bus.backup,
            &bus.palette,
            &bus.card,
            &bus.audio_ram,
        ] {
            hash.update(bytes);
        }
        for register in self.main.dar {
            hash.update(&register.to_le_bytes());
        }
        for reg in [StkReg16::AF, StkReg16::BC, StkReg16::DE, StkReg16::HL] {
            hash.update(&self.sound.get_reg16(reg).to_le_bytes());
            hash.update(&self.sound.get_alt_reg16(reg).to_le_bytes());
        }
        for word in [
            self.main.status_register(),
            self.sound.get_pc(),
            self.sound.get_sp(),
            self.sound.get_index16(Prefix::Xdd),
            self.sound.get_index16(Prefix::Yfd),
            bus.video_pointer,
            bus.video_stride,
            bus.video_mode,
        ] {
            hash.update(&word.to_le_bytes());
        }
        hash.update(&bus.z80_banks);
        hash.update(&bus.ym_addresses);
        if let Some(audio) = &bus.restored_audio {
            hash.update(&audio.output_samples.to_le_bytes());
            audio.adpcm.hash_state(&mut hash);
            audio.timers.hash_state(&mut hash);
            audio.fm.hash_state(&mut hash);
            audio.psg.hash_state(&mut hash);
        }
        hash.update(&[
            bus.latch,
            bus.reply,
            bus.animation,
            bus.program_bank as u8,
            bus.palette_bank as u8,
            u8::from(bus.cart_fixed),
            u8::from(bus.cart_vectors),
            u8::from(bus.dark),
            u8::from(bus.backup_enabled),
            u8::from(bus.pending_nmi),
            u8::from(bus.nmi_enabled),
        ]);
        hash.update(&bus.irq_counter.to_le_bytes());
        hash.update(&bus.animation_step.to_le_bytes());
        hash.update(&[bus.calendar.lines, bus.calendar.command, bus.calendar.mode]);
        for value in [
            bus.calendar.ticks,
            bus.calendar.shift,
            bus.calendar.pulse_epoch,
            bus.calendar.pulse_half_period,
        ] {
            hash.update(&value.to_le_bytes());
        }
        hash.update(&self.main.pc.to_le_bytes());
        hash.update(&self.main_cycles.to_le_bytes());
        hash.update(&self.sound_clock.0 .0.to_le_bytes());
        for channels in &self.pcm {
            for sample in channels {
                hash.update(&sample.to_le_bytes());
            }
        }
        FrameReport {
            frame: self.frame,
            main_cycles: self.main_cycles,
            sound_cycles: self.sound_clock.0 .0,
            main_pc: self.main.pc,
            sound_pc: self.sound.get_pc(),
            work_crc32: crc32fast::hash(&bus.work),
            graphics_ram_crc32: crc32fast::hash(&bus.video),
            qsound_commands: bus.sound_commands,
            state_crc32: hash.finalize(),
        }
    }

    pub fn render_rgb(&self) -> Vec<u8> {
        let bus = &self.main.mem;
        const W: usize = 320;
        const MARGIN: usize = (FRAME_WIDTH - W) / 2;
        const ZOOM_X: [u16; 16] = [
            0x0080, 0x0880, 0x0888, 0x2888, 0x288a, 0x2a8a, 0x2aaa, 0xaaaa, 0xaaea, 0xbaea, 0xbaeb,
            0xbbeb, 0xbbef, 0xfbef, 0xfbff, 0xffff,
        ];
        let palette: Vec<[u8; 3]> = bus.palette[bus.palette_bank * 0x2000..][..0x2000]
            .chunks_exact(2)
            .map(|p| {
                let value = u16::from_be_bytes([p[0], p[1]]);
                [
                    ((value >> 7) & 30) | ((value >> 14) & 1),
                    ((value >> 3) & 30) | ((value >> 13) & 1),
                    ((value << 1) & 30) | ((value >> 12) & 1),
                ]
                .map(|v| (v << 3) as u8)
            })
            .collect();
        let mut output = vec![0; FRAME_WIDTH * FRAME_HEIGHT * 3];
        let fixed = if bus.cart_fixed {
            &bus.roms.fixed
        } else {
            &bus.roms.system_fixed
        };
        for (y, row) in output.chunks_exact_mut(FRAME_WIDTH * 3).enumerate() {
            let line = y + 16;
            let visible = &mut row[MARGIN * 3..(MARGIN + W) * 3];
            let mut pens = [0xfff; W];
            let (mut sx, mut sy, mut zoomx, mut zoomy, mut height) = (0, 0, 0, 0, 0);
            let mut visible_sprites = 0;
            for sprite in 0..381usize {
                let control = usize::from(bus.vram(0x8200 + sprite));
                let zoom = usize::from(bus.vram(0x8000 + sprite));
                if control & 0x40 != 0 {
                    sx = (sx + zoomx + 1) & 0x1ff;
                } else {
                    sx = usize::from(bus.vram(0x8400 + sprite) >> 7);
                    sy = (512 - (control >> 7)) & 0x1ff;
                    zoomy = zoom & 0xff;
                    height = control & 63;
                }
                zoomx = (zoom >> 8) & 15;
                let sprite_line = line.wrapping_sub(sy) & 0x1ff;
                if height == 0 || sprite_line >= height * 16 {
                    continue;
                }
                visible_sprites += 1;
                if visible_sprites > 96 {
                    break;
                }
                if (W..=496).contains(&sx) {
                    continue;
                }
                let mut inverse = sprite_line & 0x100 != 0;
                let mut zoom_line = sprite_line & 0xff;
                if inverse {
                    zoom_line ^= 0xff;
                }
                if height > 32 {
                    zoom_line %= 2 * (zoomy + 1);
                    if zoom_line > zoomy {
                        zoom_line = 2 * zoomy + 1 - zoom_line;
                        inverse = !inverse;
                    }
                }
                let entry = usize::from(bus.roms.zoom[zoomy * 256 + zoom_line]);
                let mut tile = entry >> 4;
                let mut py = entry & 15;
                if inverse {
                    tile ^= 31;
                    py ^= 15;
                }
                let index = sprite * 64 + tile * 2;
                let attr = usize::from(bus.vram(index + 1));
                let mut code = usize::from(bus.vram(index)) | ((attr & 0xf0) << 12);
                if bus.video_mode & 8 == 0 {
                    let mask = if attr & 8 != 0 {
                        7
                    } else if attr & 4 != 0 {
                        3
                    } else {
                        0
                    };
                    code = (code & !mask) | (usize::from(bus.animation) & mask);
                }
                if attr & 2 != 0 {
                    py ^= 15;
                }
                let mut screen_x = sx;
                for pixel in 0..16 {
                    if ZOOM_X[zoomx] & (0x8000 >> pixel) == 0 {
                        continue;
                    }
                    let target = screen_x & 0x1ff;
                    screen_x += 1;
                    if target >= W {
                        continue;
                    }
                    let px = if attr & 1 != 0 { pixel ^ 15 } else { pixel };
                    let offset = code * 128 + py * 4 + if px & 8 == 0 { 64 } else { 0 };
                    if let Some(data) = bus.roms.sprites.get(offset..offset + 4) {
                        let bit = px & 7;
                        let pen = [0, 2, 1, 3]
                            .into_iter()
                            .enumerate()
                            .fold(0, |v, (plane, i)| {
                                v | usize::from((data[i] >> bit) & 1) << plane
                            });
                        if pen != 0 {
                            pens[target] = (attr >> 8) * 16 + pen;
                        }
                    }
                }
            }
            for x in 0..40 {
                let entry = usize::from(bus.vram(0x7000 + x * 32 + line / 8));
                let tile = (entry & 0xfff) * 32 + line % 8;
                for (pair, offset) in [16, 24, 0, 8].into_iter().enumerate() {
                    let byte = fixed[tile + offset];
                    for (pixel, pen) in [byte & 15, byte >> 4].into_iter().enumerate() {
                        if pen != 0 {
                            pens[x * 8 + pair * 2 + pixel] = (entry >> 12) * 16 + usize::from(pen);
                        }
                    }
                }
            }
            for (pixel, pen) in visible.chunks_exact_mut(3).zip(pens) {
                pixel.copy_from_slice(&palette[pen]);
            }
        }
        output
    }
}

#[cfg(test)]
mod active_audio_tests;
