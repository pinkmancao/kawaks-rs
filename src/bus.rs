use r68k::ram::{AddressBus, AddressSpace, SUPERVISOR_PROGRAM, USER_PROGRAM};
use z80emu::{Io, Memory};

use crate::{qsound::QSound, rom::Game, DinoRoms, FrameInput};

#[derive(Clone)]
pub(crate) struct Cps2Io {
    pub objects: Vec<u8>,
    pub output: [u8; 16],
    pub object_bank: usize,
    extra_ram: Vec<u8>,
    pub sound_reset: bool,
}

impl Cps2Io {
    fn new() -> Self {
        Self {
            objects: vec![0; 0x10000],
            output: [0; 16],
            object_bank: 0,
            extra_ram: vec![0; 0x4002],
            sound_reset: false,
        }
    }

    fn object_address(&self, address: u32) -> usize {
        let offset = (address & 0xffff) as usize;
        (offset & 0x1fff) | ((offset & 0x8000) ^ (self.object_bank << 15))
    }
}

pub(crate) struct Bus {
    pub cps2: Option<Cps2Io>,
    pub roms: DinoRoms,
    pub work: Vec<u8>,
    pub graphics: Vec<u8>,
    pub registers: Vec<u8>,
    pub sound_ram: Vec<u8>,
    pub eeprom: [u8; 0x80],
    pub bank: u8,
    pub dips: [u8; 3],
    pub input: FrameInput,
    pub qsound_data: u16,
    pub qsound_registers: [u16; 256],
    pub qsound_commands: u64,
    #[cfg(test)]
    pub qsound_trace: Vec<(u8, u16)>,
    pub qsound: QSound,
    pub qsound_legacy_mixer: [u8; 0x2c],
    eeprom_lines: EepromLines,
}

impl Bus {
    pub fn new(roms: DinoRoms) -> Self {
        Self {
            cps2: matches!(roms.0.game, Game::Ssf2).then(Cps2Io::new),
            roms,
            work: vec![0; 0x10000],
            graphics: vec![0; 0x30000],
            registers: vec![0xff; 0x200],
            sound_ram: vec![0; 0x4000],
            eeprom: [0xff; 0x80],
            bank: 0,
            dips: [0xff; 3],
            input: FrameInput::default(),
            qsound_data: 0,
            qsound_registers: [0; 256],
            qsound_commands: 0,
            #[cfg(test)]
            qsound_trace: Vec::new(),
            qsound: QSound::default(),
            qsound_legacy_mixer: [0; 0x2c],
            eeprom_lines: EepromLines::default(),
        }
    }

    pub fn sound_opcode(&self, address: u16) -> u8 {
        match address {
            0..=0x7fff => self.roms.0.sound_opcodes[usize::from(address)],
            _ => self.read_debug(address),
        }
    }

    pub fn mix_audio_frame(&mut self) {
        self.qsound.mix_frame(&self.roms.0.samples);
    }

    pub fn main_read8(&self, addr: u32) -> u8 {
        let addr = addr & 0xff_ffff;
        if let Some(cps2) = &self.cps2 {
            let players = self.input.players;
            match addr {
                0x400000..=0x40000f => return cps2.output[(addr & 15) as usize],
                0x618000..=0x619fff => {
                    return if addr & 1 != 0 {
                        self.sound_ram[shared_offset(addr, 0)]
                    } else {
                        0xff
                    }
                }
                0x660000..=0x664001 => return cps2.extra_ram[(addr - 0x660000) as usize],
                0x700000..=0x70ffff => return cps2.objects[cps2.object_address(addr)],
                0x804000 => return !player_bits(players[1]),
                0x804001 => return !player_bits(players[0]),
                0x804011 => {
                    return !(((players[0] >> 7) as u8 & 7) | ((players[1] >> 7) as u8 & 3) << 4)
                }
                0x804020 => {
                    let p1 = players[0];
                    let p2 = players[1];
                    let pressed = (p1 >> 12 & 1)
                        | (p2 >> 11 & 2)
                        | (p1 >> 9 & 0x10)
                        | (p2 >> 8 & 0x20)
                        | (p2 >> 3 & 0x40);
                    return !(pressed as u8);
                }
                0x804021 => return 0xfe | u8::from(self.eeprom_lines.data_out),
                _ => {}
            }
        }
        match addr {
            0..=0x3fffff => self.roms.0.program[addr as usize],
            0x900000..=0x92ffff => self.graphics[(addr - 0x900000) as usize],
            0xff0000..=0xffffff => self.work[(addr - 0xff0000) as usize],
            0x800000..=0x80ffff => self.io_read(addr),
            0xf00000..=0xf0ffff => self.roms.0.sound[((addr >> 1) & 0x7fff) as usize],
            0xf18000..=0xf19fff => self.sound_ram[shared_offset(addr, 0)],
            0xf1c001 if !matches!(self.roms.0.game, crate::rom::Game::Punisher) => {
                let buttons = self.input.players[2];
                let directions_actions = player_bits(buttons) & 0x3f;
                let coin = ((buttons >> 13) as u8 & 1) << 6;
                let start = ((buttons >> 12) as u8 & 1) << 7;
                !(directions_actions | coin | start)
            }
            0xf1c007 => u8::from(self.eeprom_lines.data_out),
            0xf1e000..=0xf1ffff => self.sound_ram[shared_offset(addr, 0x3000)],
            _ => 0xff,
        }
    }

    pub fn hash_device_state(&self, hash: &mut crc32fast::Hasher) {
        hash.update(&self.eeprom_lines.command.to_le_bytes());
        hash.update(&[
            self.eeprom_lines.bits,
            self.eeprom_lines.clock.into(),
            self.eeprom_lines.data_out.into(),
            self.eeprom_lines.write_enabled.into(),
        ]);
        if let Some((word, bits)) = self.eeprom_lines.read {
            hash.update(&[1, bits]);
            hash.update(&word.to_le_bytes());
        } else {
            hash.update(&[0]);
        }
        if let Some(cps2) = &self.cps2 {
            hash.update(&cps2.objects);
            hash.update(&cps2.output);
            hash.update(&cps2.extra_ram);
            hash.update(&[cps2.object_bank as u8, u8::from(cps2.sound_reset)]);
        }
        self.qsound.hash_state(hash);
        hash.update(&self.qsound_legacy_mixer);
    }

    fn io_read(&self, addr: u32) -> u8 {
        let players = self.input.players;
        match addr {
            0x800000 | 0x800010 => !player_bits(players[1]),
            0x800001 | 0x800011 => !player_bits(players[0]),
            0x800018 | 0x800019 | 0x800028 | 0x800029 => !coin_service_bits(players),
            0x80001a | 0x80002a => self.dips[0],
            0x80001c | 0x80002c => self.dips[1],
            0x80001e | 0x80002e => self.dips[2],
            0x80014e | 0x80414e if matches!(self.roms.0.game, crate::rom::Game::Punisher) => 0x0c,
            0x80014f | 0x80414f if matches!(self.roms.0.game, crate::rom::Game::Punisher) => 0,
            0x804021 => 0xfe | u8::from(self.eeprom_lines.data_out),
            _ => self.registers[(addr & 0x1ff) as usize],
        }
    }

    fn program_read8(&self, space: AddressSpace, address: u32) -> u8 {
        let address = address & 0xff_ffff;
        if matches!(space, SUPERVISOR_PROGRAM | USER_PROGRAM) && address <= 0x3fffff {
            if let Some(opcodes) = &self.roms.0.main_opcodes {
                return opcodes[address as usize];
            }
        }
        self.main_read8(address)
    }

    pub fn main_write8(&mut self, addr: u32, value: u8) {
        let addr = addr & 0xff_ffff;
        if let Some(cps2) = &mut self.cps2 {
            match addr {
                0x400000..=0x40000f => {
                    cps2.output[(addr & 15) as usize] = value;
                    return;
                }
                0x618000..=0x619fff => {
                    if addr & 1 != 0 {
                        self.sound_ram[shared_offset(addr, 0)] = value;
                    }
                    return;
                }
                0x660000..=0x664001 => {
                    cps2.extra_ram[(addr - 0x660000) as usize] = value;
                    return;
                }
                0x700000..=0x70ffff => {
                    let offset = cps2.object_address(addr);
                    cps2.objects[offset] = value;
                    return;
                }
                0x804041 => {
                    cps2.sound_reset = value & 8 == 0;
                    return;
                }
                0x8040e1 => {
                    cps2.object_bank = usize::from(value & 1);
                    return;
                }
                _ => {}
            }
        }
        match addr {
            0xff0000..=0xffffff => self.work[(addr - 0xff0000) as usize] = value,
            0x900000..=0x92ffff => self.graphics[(addr - 0x900000) as usize] = value,
            0x804040 => self
                .eeprom_lines
                .drive(&mut self.eeprom, value ^ 0x40, 4, 5, 6),
            0xf1c007 => self.eeprom_lines.drive(&mut self.eeprom, value, 0, 6, 7),
            0x800100..=0x8001ff | 0x804100..=0x8041ff => {
                self.registers[(addr & 0x1ff) as usize] = value
            }
            0xf18000..=0xf19fff => self.sound_ram[shared_offset(addr, 0)] = value,
            0xf1e000..=0xf1ffff => self.sound_ram[shared_offset(addr, 0x3000)] = value,
            _ => {}
        }
    }
}

fn shared_offset(addr: u32, base: usize) -> usize {
    base + ((addr >> 1) & 0x0fff) as usize
}

fn player_bits(input: u16) -> u8 {
    let pressed = input as u8;
    (pressed & 0x0f).reverse_bits() >> 4 | (pressed & 0x70)
}

fn coin_service_bits(players: [u16; 4]) -> u8 {
    let mut result = 0;
    for (slot, buttons) in players.into_iter().take(2).enumerate() {
        result |= (((buttons >> 13) & 1) as u8) << slot;
        result |= (((buttons >> 12) & 1) as u8) << (4 + slot);
    }
    result
}

impl AddressBus for Bus {
    fn read_byte(&self, space: AddressSpace, address: u32) -> u32 {
        self.program_read8(space, address).into()
    }

    fn read_word(&self, space: AddressSpace, address: u32) -> u32 {
        let address = address & 0xff_ffff;
        let high = if matches!(address, 0xf18000..=0xf19fff | 0xf1c000..=0xf1ffff) {
            0xff
        } else {
            self.program_read8(space, address)
        };
        u16::from_be_bytes([high, self.program_read8(space, address + 1)]).into()
    }

    fn read_long(&self, space: AddressSpace, address: u32) -> u32 {
        (self.read_word(space, address) << 16) | self.read_word(space, address.wrapping_add(2))
    }

    fn write_byte(&mut self, _: AddressSpace, address: u32, value: u32) {
        self.main_write8(address, value as u8);
    }

    fn write_word(&mut self, _: AddressSpace, address: u32, value: u32) {
        let address = address & 0xff_ffff;
        let [high, low] = (value as u16).to_be_bytes();
        if !matches!(address, 0xf18000..=0xf19fff | 0xf1c000..=0xf1ffff) {
            self.main_write8(address, high);
        }
        self.main_write8(address + 1, low);
    }

    fn write_long(&mut self, space: AddressSpace, address: u32, value: u32) {
        self.write_word(space, address, value >> 16);
        self.write_word(space, address.wrapping_add(2), value);
    }

    fn copy_from(&mut self, other: &Self) {
        self.roms = other.roms.clone();
        self.cps2.clone_from(&other.cps2);
        self.work.clone_from(&other.work);
        self.graphics.clone_from(&other.graphics);
        self.registers.clone_from(&other.registers);
        self.sound_ram.clone_from(&other.sound_ram);
        self.eeprom = other.eeprom;
        self.bank = other.bank;
        self.dips = other.dips;
        self.input = other.input;
        self.qsound_data = other.qsound_data;
        self.qsound_registers = other.qsound_registers;
        self.qsound_commands = other.qsound_commands;
        self.qsound.clone_from(&other.qsound);
        self.qsound_legacy_mixer = other.qsound_legacy_mixer;
        self.eeprom_lines = other.eeprom_lines;
    }
}

impl Memory for Bus {
    type Timestamp = u64;

    fn read_debug(&self, address: u16) -> u8 {
        let offset = address as usize;
        match address {
            0..=0x7fff => self.roms.0.sound_data[offset],
            0x8000..=0xbfff => self
                .roms
                .0
                .sound
                .get(offset + usize::from(self.bank) * 0x4000)
                .copied()
                .unwrap_or(0xff),
            0xd007 => 0x80,
            _ => self.sound_ram[offset - 0xc000],
        }
    }

    fn read_opcode(&mut self, pc: u16, _: u16, _: u64) -> u8 {
        self.sound_opcode(pc)
    }

    fn write_mem(&mut self, address: u16, value: u8, _: u64) {
        match address {
            0xd000 => self.qsound_data = (self.qsound_data & 0xff) | (u16::from(value) << 8),
            0xd001 => self.qsound_data = (self.qsound_data & 0xff00) | u16::from(value),
            0xd002 => {
                self.qsound.write(value, self.qsound_data);
                self.qsound_registers[value as usize] = self.qsound_data;
                self.qsound_commands += 1;
                #[cfg(test)]
                self.qsound_trace.push((value, self.qsound_data));
            }
            0xd003 => self.bank = value & 15,
            0xc000..=0xffff => self.sound_ram[usize::from(address - 0xc000)] = value,
            _ => {}
        }
    }
}

impl Io for Bus {
    type Timestamp = u64;
    type WrIoBreak = ();
    type RetiBreak = ();
}

#[derive(Clone, Copy)]
struct EepromLines {
    clock: bool,
    command: u32,
    bits: u8,
    read: Option<(u16, u8)>,
    data_out: bool,
    write_enabled: bool,
}

impl Default for EepromLines {
    fn default() -> Self {
        Self {
            clock: false,
            command: 0,
            bits: 0,
            read: None,
            data_out: true,
            write_enabled: false,
        }
    }
}

impl EepromLines {
    fn drive(
        &mut self,
        data: &mut [u8; 128],
        value: u8,
        data_bit: u8,
        clock_bit: u8,
        select_bit: u8,
    ) {
        if value & (1 << select_bit) != 0 {
            self.bits = 0;
            self.command = 0;
            self.read = None;
            self.data_out = true;
        } else if value & (1 << clock_bit) != 0 && !self.clock {
            let bit = u32::from((value >> data_bit) & 1);
            self.shift(data, bit);
        }
        self.clock = value & (1 << clock_bit) != 0;
    }

    fn shift(&mut self, data: &mut [u8; 128], bit: u32) {
        if let Some((word, remaining)) = self.read {
            self.data_out = remaining != 0 && (word >> remaining.saturating_sub(1)) & 1 != 0;
            self.read = Some((word, remaining.saturating_sub(1)));
            return;
        }
        if (self.bits == 0 && bit == 0) || self.bits >= 25 {
            return;
        }
        self.command = (self.command << 1) | bit;
        self.bits += 1;
        if self.bits == 9 {
            let offset = (self.command & 63) as usize * 2;
            match (self.command >> 6) & 3 {
                0 => self.write_enabled = self.command & 0x30 == 0x30,
                2 => {
                    self.data_out = false;
                    self.read = Some((u16::from_be_bytes([data[offset], data[offset + 1]]), 16));
                }
                3 if self.write_enabled => data[offset..offset + 2].fill(0xff),
                _ => {}
            }
        }
        if self.bits == 25 && self.write_enabled && (self.command >> 22) & 3 == 1 {
            let offset = ((self.command >> 16) & 63) as usize * 2;
            data[offset..offset + 2].copy_from_slice(&(self.command as u16).to_be_bytes());
        }
    }
}
