//! Headless CPS1 QSound machine for the dino ROM set.
mod bus;
mod cps2;
mod export;
mod kabuki;
mod neo;
mod qsound;
mod rom;
mod save;
mod video;

use bus::Bus;
use r68k::{
    cpu::{ConfiguredCore, ProcessingState},
    interrupts::{AutoInterruptController, InterruptController},
};
use save::{le16, le32, swap_words};
use std::io;
use z80emu::{
    host::{BreakCause, TsCounter},
    Cpu, CpuDebug, InterruptMode, Prefix, StkReg16, Z80NMOS,
};

pub use export::{write_png, WavWriter};
pub use neo::{NeoGeo, NeoKwk4, NeoRoms};
pub use qsound::{AUDIO_SAMPLES_PER_FRAME, AUDIO_SAMPLE_RATE};
pub use rom::DinoRoms;

pub trait ArcadeMachine {
    fn step_frame(&mut self, input: FrameInput) -> io::Result<FrameReport>;
    fn report(&self) -> FrameReport;
    fn render_rgb(&self) -> Vec<u8>;
    fn frame_audio(&self) -> &[[i16; 2]];
}

macro_rules! arcade_machine {
    ($machine:ty) => {
        impl ArcadeMachine for $machine {
            fn step_frame(&mut self, input: FrameInput) -> io::Result<FrameReport> {
                self.step_frame(input)
            }
            fn report(&self) -> FrameReport {
                self.report()
            }
            fn render_rgb(&self) -> Vec<u8> {
                self.render_rgb()
            }
            fn frame_audio(&self) -> &[[i16; 2]] {
                self.frame_audio()
            }
        }
    };
}
arcade_machine!(Dino);
arcade_machine!(NeoGeo);
pub use save::Kwk4;
pub use video::{FRAME_HEIGHT, FRAME_WIDTH};

const MAIN_CYCLES_PER_LINE: u64 = 640;
const LINES: u64 = 262;
const VBLANK_DURATION: u64 = 5;
const SOUND_CYCLES_PER_SLICE: u64 = 4 * 512;
const SOUND_SLICES: u64 = 65;
const SOUND_IRQ_PERIOD: u64 = 31_683;

/// Active-high logical buttons in WinKawaks input order.
/// Bits 0..3 are up/down/left/right, bits 4..9 are buttons 1..6.
/// Bit 12 is start and bit 13 is coin.
#[derive(Clone, Copy, Default, Debug)]
pub struct FrameInput {
    pub players: [u16; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameReport {
    pub frame: u64,
    pub main_cycles: u64,
    pub sound_cycles: u64,
    pub main_pc: u32,
    pub sound_pc: u16,
    pub work_crc32: u32,
    pub graphics_ram_crc32: u32,
    pub qsound_commands: u64,
    pub state_crc32: u32,
}

pub struct Dino {
    main: ConfiguredCore<AutoInterruptController, Bus>,
    sound: Z80NMOS,
    video: video::Video,
    sound_clock: TsCounter<u64>,
    main_deadline: u64,
    main_elapsed: u64,
    sound_elapsed: u64,
    irq_phase: u64,
    sound_irq_cycles: u64,
    sound_irq: bool,
    frame: u64,
    #[cfg(test)]
    sound_slices: Vec<reference_tests::SoundSlice>,
    #[cfg(test)]
    irq_accepts: Vec<(bool, u16, u64)>,
}

impl Dino {
    pub fn new(roms: DinoRoms) -> Self {
        let mut main = ConfiguredCore::new_with(0, AutoInterruptController::new(), Bus::new(roms));
        main.reset();
        let video = video::Video::new(&main.mem);
        Self {
            main,
            video,
            sound: Z80NMOS::default(),
            sound_clock: TsCounter::from(0),
            main_deadline: 0,
            main_elapsed: 0,
            sound_elapsed: 0,
            irq_phase: 0,
            sound_irq_cycles: 0,
            sound_irq: false,
            frame: 0,
            #[cfg(test)]
            sound_slices: Vec::new(),
            #[cfg(test)]
            irq_accepts: Vec::new(),
        }
    }

    /// Import the hardware state understood by this core. Rebuild all memory
    /// mappings from the board definition rather than restoring x86 pointers.
    pub fn from_kwk4(roms: DinoRoms, mut state: Kwk4) -> io::Result<Self> {
        let mut machine = Self::new(roms);
        if machine.main.mem.cps2.is_some() != state.cps2.is_some() {
            return Err(invalid(
                "KWK4 board layout does not match the selected game",
            ));
        }
        let main = &mut machine.main;
        restore_main(main, &state.main)?;

        swap_words(&mut state.work);
        swap_words(&mut state.registers);
        swap_words(&mut state.graphics);
        main.mem.work = state.work;
        main.mem.registers = state.registers;
        main.mem.graphics = state.graphics;
        main.mem.sound_ram = state.sound_ram;
        main.mem.eeprom = state.eeprom;
        if let (Some(io), Some(mut cps2)) = (&mut main.mem.cps2, state.cps2) {
            swap_words(&mut cps2.objects);
            swap_words(&mut cps2.output);
            io.objects = cps2.objects;
            io.output = cps2.output;
            io.sound_reset = !cps2.sound_enabled;
            main.mem.registers[0x150..0x152].copy_from_slice(&le16(&cps2.tail, 8).to_be_bytes());
            main.mem.registers[0x152..0x154].copy_from_slice(&le16(&cps2.tail, 12).to_be_bytes());
        }
        main.mem.bank = state.sound_bank;
        main.mem.dips = [state.tail[0], state.tail[4], state.tail[8]];
        main.mem.qsound.restore(&state.channels);
        main.mem.qsound_legacy_mixer = state.mixer;
        machine.sound_irq = state.sound[0x62] != 0;
        machine.video = video::Video::new(&main.mem);
        restore_sound(&mut machine.sound, &state.sound)?;
        Ok(machine)
    }

    pub fn step_frame(&mut self, input: FrameInput) -> io::Result<FrameReport> {
        self.main.mem.input = input;
        self.video.latch_objects(&self.main.mem);
        let cycles_per_line = if self.main.mem.cps2.is_some() {
            755
        } else {
            MAIN_CYCLES_PER_LINE
        };
        self.advance_main((LINES - VBLANK_DURATION) * cycles_per_line)?;
        self.main.int_ctrl.request_interrupt(2);
        self.advance_main(VBLANK_DURATION * cycles_per_line)?;

        #[cfg(test)]
        self.sound_slices.clear();
        for _ in 0..SOUND_SLICES {
            #[cfg(test)]
            self.irq_accepts.clear();
            let start = self.sound_elapsed;
            let deadline = start + SOUND_CYCLES_PER_SLICE;
            // execute_next exposes DD/FD prefixes as separate steps. A legacy
            // slice can end only after the complete indexed instruction.
            while self.sound_elapsed <= deadline || self.sound.get_prefix().is_some() {
                if self.sound_irq && self.sound.is_irq_allowed() {
                    #[cfg(test)]
                    self.irq_accepts
                        .push((false, self.sound.get_pc(), self.sound_elapsed - start));
                    self.accept_sound_irq()?;
                } else {
                    self.execute_sound_instruction()?;
                }
            }
            // WinKawaks runs each slice with a fresh budget and measures the
            // executed cycles, including any IRQ charged after the previous slice.
            let cycles = (self.sound_elapsed - start) + self.sound_irq_cycles;
            #[cfg(test)]
            let before_irq = self.sound.get_pc();
            self.irq_phase += cycles;
            self.sound_irq_cycles = 0;
            if self.irq_phase >= SOUND_IRQ_PERIOD {
                self.irq_phase -= SOUND_IRQ_PERIOD;
                self.sound_irq = true;
                if self.sound.is_irq_allowed() {
                    let before = self.sound_elapsed;
                    #[cfg(test)]
                    self.irq_accepts.push((true, self.sound.get_pc(), cycles));
                    self.accept_sound_irq()?;
                    self.sound_irq_cycles = self.sound_elapsed - before;
                }
            }
            #[cfg(test)]
            self.sound_slices.push(reference_tests::SoundSlice {
                pc: before_irq,
                cycles,
                after: self.sound.get_pc(),
                phase: self.irq_phase,
                pending: self.sound_irq,
                irqs: self.irq_accepts.clone(),
            });
        }
        self.video.latch_palette_base(&self.main.mem);
        self.main.mem.mix_audio_frame();
        self.frame += 1;
        Ok(self.report())
    }

    fn execute_sound_instruction(&mut self) -> io::Result<()> {
        let pc = self.sound.get_pc();
        let bus = &mut self.main.mem;
        let legacy_block_transfer = !self.sound.is_halt()
            && bus.sound_opcode(pc) == 0xed
            && matches!(bus.sound_opcode(pc.wrapping_add(1)), 0xb0 | 0xb8);
        let result = self
            .sound
            .execute_next(bus, &mut self.sound_clock, None::<fn(CpuDebug)>);
        if let Err(BreakCause::WriteIo(_) | BreakCause::Reti(_)) = result {
            return Err(invalid("unexpected host-side Z80 break"));
        }
        // WinKawaks charges 16 cycles per byte for both LDIR and LDDR,
        // including each iteration that rewinds PC when the budget is spent.
        if legacy_block_transfer {
            self.sound_clock = TsCounter::from(self.sound_elapsed + 16);
        }
        self.sound_elapsed = self.sound_clock.0 .0;
        Ok(())
    }

    fn accept_sound_irq(&mut self) -> io::Result<()> {
        self.sound_irq = false;
        // Use the CPU core's interrupt semantics and WinKawaks's cycle charge.
        let cycles = match self.sound.get_im() {
            InterruptMode::Mode0 => 13,
            InterruptMode::Mode1 | InterruptMode::Mode2 => 11,
        };
        let result = self
            .sound
            .irq(
                &mut self.main.mem,
                &mut self.sound_clock,
                None::<fn(CpuDebug)>,
            )
            .expect("IRQ allowed at this instruction boundary");
        if result.is_err() {
            return Err(invalid("unexpected host-side Z80 IRQ break"));
        }
        self.sound_elapsed += cycles;
        self.sound_clock = TsCounter::from(self.sound_elapsed);
        Ok(())
    }

    fn advance_main(&mut self, budget: u64) -> io::Result<()> {
        self.main_deadline += budget;
        if self.main_deadline > self.main_elapsed {
            let consumed = self
                .main
                .execute((self.main_deadline - self.main_elapsed) as i32)
                .0;
            if consumed <= 0 || self.main.processing_state == ProcessingState::Halted {
                return Err(invalid(format!("68000 halted at {:06x}", self.main.pc)));
            }
            self.main_elapsed += consumed as u64;
        }
        Ok(())
    }

    /// Render the current video state, including immediately after a KWK4 restore.
    pub fn render_rgb(&self) -> Vec<u8> {
        video::render(&self.main.mem, &self.video)
    }

    /// Signed, interleaved stereo sample frames from the most recent step.
    pub fn frame_audio(&self) -> &[[i16; 2]] {
        &self.main.mem.qsound.pcm
    }

    pub fn read_main_byte(&self, address: u32) -> u8 {
        self.main.mem.main_read8(address)
    }

    pub fn report(&self) -> FrameReport {
        let bus = &self.main.mem;
        let work_crc32 = crc32fast::hash(&bus.work);
        let graphics_ram_crc32 = crc32fast::hash(&bus.graphics);
        let mut state = crc32fast::Hasher::new();
        for value in self.main.dar {
            state.update(&value.to_le_bytes());
        }
        state.update(&self.main.pc.to_le_bytes());
        state.update(&self.main.status_register().to_le_bytes());
        state.update(&self.main.usp().to_le_bytes());
        state.update(&self.main.ssp().to_le_bytes());
        state.update(&[self.main.int_ctrl.highest_priority()]);
        for reg in [StkReg16::AF, StkReg16::BC, StkReg16::DE, StkReg16::HL] {
            state.update(&self.sound.get_reg16(reg).to_le_bytes());
            state.update(&self.sound.get_alt_reg16(reg).to_le_bytes());
        }
        for value in [
            self.sound.get_pc(),
            self.sound.get_sp(),
            self.sound.get_ir(),
            self.sound.get_memptr(),
            self.sound.get_index16(Prefix::Xdd),
            self.sound.get_index16(Prefix::Yfd),
        ] {
            state.update(&value.to_le_bytes());
        }
        let (iff1, iff2) = self.sound.get_iffs();
        state.update(&[
            iff1.into(),
            iff2.into(),
            self.sound.get_im() as u8,
            self.sound.is_halt().into(),
            self.sound.is_after_ei().into(),
            self.sound.get_prefix().map_or(0, Prefix::to_code),
        ]);

        for value in [
            self.frame,
            self.main_elapsed,
            self.sound_elapsed,
            self.irq_phase,
            self.sound_irq_cycles,
            bus.qsound_commands,
        ] {
            state.update(&value.to_le_bytes());
        }
        for region in [&bus.work, &bus.graphics, &bus.registers, &bus.sound_ram] {
            state.update(region);
        }
        state.update(&bus.eeprom);
        bus.hash_device_state(&mut state);
        self.video.hash_state(&mut state);
        state.update(&[bus.bank, u8::from(self.sound_irq)]);
        state.update(&bus.dips);
        state.update(&bus.qsound_data.to_le_bytes());
        for value in bus.qsound_registers {
            state.update(&value.to_le_bytes());
        }
        FrameReport {
            frame: self.frame,
            main_cycles: self.main_elapsed,
            sound_cycles: self.sound_elapsed,
            main_pc: self.main.pc,
            sound_pc: self.sound.get_pc(),
            work_crc32,
            graphics_ram_crc32,
            qsound_commands: bus.qsound_commands,
            state_crc32: state.finalize(),
        }
    }
}

fn restore_main<B: r68k::ram::AddressBus>(
    main: &mut ConfiguredCore<AutoInterruptController, B>,
    state: &[u8; 0x80],
) -> io::Result<()> {
    let pc = le32(state, 0x50);
    if pc > 0xff_ffff || pc & 1 != 0 {
        return Err(invalid(format!("invalid saved 68000 PC {pc:08x}")));
    }
    let flags = le32(state, 0x48);
    let sr = ((le16(state, 0x44) & 0xff) << 8)
        | ((le16(state, 0x4c) & 1) << 4)
        | (((flags >> 4) as u16) & 0x0c)
        | (((flags >> 10) as u16) & 2)
        | (flags as u16 & 1);
    main.sr_to_flags(sr);
    main.dar = std::array::from_fn(|reg| le32(state, reg * 4));
    main.inactive_ssp = le32(state, 0x40);
    main.inactive_usp = le32(state, 0x70);
    main.pc = pc;
    main.prefetch_addr = (pc & !3) ^ 4;
    let pending = state[0x54] & 7;
    if pending != 0 {
        main.int_ctrl.request_interrupt(pending);
    }
    Ok(())
}

fn restore_sound(cpu: &mut Z80NMOS, legacy: &[u8; 0x64]) -> io::Result<()> {
    let register_pairs = [StkReg16::AF, StkReg16::BC, StkReg16::DE, StkReg16::HL];
    for (index, reg) in register_pairs.into_iter().enumerate() {
        cpu.set_reg16(reg, le16(legacy, 0x34 + index * 4));
    }
    cpu.ex_af_af();
    cpu.exx();
    for (index, reg) in register_pairs.into_iter().enumerate() {
        cpu.set_reg16(reg, le16(legacy, 0x24 + index * 4));
    }
    cpu.set_index16(Prefix::Xdd, le16(legacy, 0x44));
    cpu.set_index16(Prefix::Yfd, le16(legacy, 0x48));
    cpu.set_sp(le16(legacy, 0x4c));
    cpu.set_pc(le16(legacy, 0x50));
    cpu.set_i(legacy[0x60]);
    cpu.set_r(legacy[0x61]);
    cpu.set_iffs(legacy[0x18] & 1 != 0, legacy[0x18] & 2 != 0);
    cpu.set_im(match le32(legacy, 0x1c) {
        0 => InterruptMode::Mode0,
        1 => InterruptMode::Mode1,
        2 => InterruptMode::Mode2,
        _ => return Err(invalid("invalid saved Z80 interrupt mode")),
    });
    if le32(legacy, 0x20) != 0 {
        cpu.halt();
    }
    Ok(())
}

pub(crate) fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod reference_tests;
