use crate::save::le32;

const ACTIVE_CHANNELS: [usize; 4] = [1, 2, 4, 5];
const ATTACK_END: i32 = 0x1000_0000;
const ENVELOPE_END: i32 = 0x1fff_ffff;
const ATTENUATION_LIMIT: i32 = 0xb55;
const ATTACK: u32 = 6;
const DECAY: u32 = 1;
const SUSTAIN: u32 = 2;
const RELEASE: u32 = 5;
// Each output feeds the next operator's phase, several phases, or the mix.
const ROUTES: [[usize; 4]; 8] = [
    [2, 3, 4, 5],
    [3, 3, 4, 5],
    [4, 3, 4, 5],
    [2, 4, 4, 5],
    [2, 5, 4, 5],
    [0, 5, 5, 5],
    [2, 5, 5, 5],
    [5, 5, 5, 5],
];
const FM_NOTES: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 3, 3, 3, 3, 3, 3];

struct Operator {
    phase: i32,
    increment: i32,
    detune: usize,
    multiplier: i32,
    level: i32,
    pending_level: i32,
    scale_shift: u8,
    scale_index: usize,
    rate_offsets: [Option<usize>; 4],
    rates: [i32; 4],
    stage: u32,
    position: i32,
    target: i32,
    slope: i32,
    sustain: i32,
    am_enable: bool,
    am: i32,
}

impl Operator {
    fn restore(block: &[u8]) -> Self {
        let word = |offset| le32(block, offset);
        Self {
            phase: word(0x2c) as i32,
            increment: word(0x30) as i32,
            detune: (word(0) as usize >> 7) & 7,
            multiplier: (word(0x28) as i32).clamp(1, 30),
            level: (word(0x54) as i32).clamp(0, 4095),
            pending_level: (word(8) as i32).clamp(0, 4095),
            scale_shift: block[0x0c] & 3,
            scale_index: usize::from(block[0x25] & 31),
            rate_offsets: [0x10, 0x14, 0x18, 0x20].map(|offset| {
                let code = word(offset);
                (code != u32::MAX).then_some(code as usize / 4)
            }),
            rates: [0x44, 0x48, 0x4c, 0x50].map(|offset| word(offset) as i32),
            stage: word(0x34),
            position: (word(0x38) as i32).clamp(0, ENVELOPE_END),
            target: (word(0x3c) as i32).clamp(0, ENVELOPE_END + 1),
            slope: word(0x40) as i32,
            sustain: (word(0x1c) as i32).clamp(ATTACK_END, ENVELOPE_END),
            am_enable: block[0x58] != 0,
            am: word(0x5c) as i32,
        }
    }

    fn reload(&mut self, frequency: i32, key_code: u8, tables: &Tables) {
        self.increment = (frequency + tables.detune[self.detune][usize::from(key_code)])
            .wrapping_mul(self.multiplier);
        let index = usize::from(key_code >> self.scale_shift);
        if self.scale_index != index {
            self.scale_index = index;
            // A pitch change recomputes rates for future stages. The active
            // slope changes on a rate write or an envelope transition.
            for rate in 0..4 {
                self.rates[rate] = self.rate_value(rate, self.rate_offsets[rate], tables);
            }
        }
    }

    fn set_rate(&mut self, index: usize, offset: Option<usize>, tables: &Tables) {
        self.rate_offsets[index] = offset;
        self.rates[index] = self.rate_value(index, offset, tables);
        if self.stage == [ATTACK, DECAY, SUSTAIN, RELEASE][index] {
            self.slope = self.rates[index];
        }
    }

    fn rate_value(&self, index: usize, offset: Option<usize>, tables: &Tables) -> i32 {
        let pool = if index == 0 {
            &tables.attack
        } else {
            &tables.decay
        };
        offset
            .and_then(|value| pool.get(value + self.scale_index))
            .copied()
            .unwrap_or(0)
    }

    fn key(&mut self, on: bool, tables: &Tables) {
        if on && self.stage == RELEASE {
            self.phase = 0;
            self.stage = ATTACK;
            self.position = 0;
            self.target = ATTACK_END;
            self.slope = self.rates[0];
        } else if !on && self.stage != RELEASE {
            if self.position < ATTACK_END {
                self.position =
                    ATTACK_END + (tables.attack_curve[(self.position >> 16) as usize] << 16);
            }
            self.stage = RELEASE;
            self.target = ENVELOPE_END;
            self.slope = self.rates[3];
        }
    }

    fn step(&mut self, pm: i32, am: i32, tables: &Tables) -> i32 {
        self.phase = self
            .phase
            .wrapping_add(self.increment)
            .wrapping_add(pm.wrapping_mul(self.increment) / 1024);
        self.position = self.position.wrapping_add(self.slope);
        if self.position >= self.target {
            match self.stage {
                ATTACK => {
                    self.stage = DECAY;
                    self.position = ATTACK_END;
                    self.target = self.sustain;
                    self.slope = self.rates[1];
                }
                DECAY => {
                    self.stage = SUSTAIN;
                    self.position = self.sustain;
                    self.target = ENVELOPE_END;
                    self.slope = self.rates[2];
                }
                _ => {
                    self.position = ENVELOPE_END;
                    self.target = ENVELOPE_END + 1;
                    self.slope = 0;
                }
            }
        }
        self.position = self.position.clamp(0, ENVELOPE_END);
        let position = self.position >> 16;
        let envelope = if position < 4096 {
            tables.attack_curve[position as usize]
        } else {
            position - 4096
        };
        let amplitude = ((self.am.wrapping_mul(am) as u32) >> 16) as i32;
        envelope + self.level + amplitude
    }
}

struct Channel {
    operators: [Operator; 4],
    pan: usize,
    algorithm: usize,
    feedback: u8,
    history: [i32; 2],
    pm: i32,
    am: i32,
    frequency: i32,
    fnum_high: u8,
    key_code: u8,
    dirty: bool,
}

struct Tables {
    detune: [[i32; 32]; 8],
    attack: [i32; 94],
    decay: [i32; 94],
    fnum: [i32; 2048],
    attack_curve: [i32; 4096],
    sine: [usize; 2048],
    level: [i32; 3328],
}

impl Tables {
    fn restore(raw: &[u8]) -> Self {
        let word = |offset| le32(raw, offset) as i32;
        Self {
            detune: std::array::from_fn(|detune| {
                std::array::from_fn(|code| word(0x40 + (detune * 32 + code) * 4))
            }),
            attack: std::array::from_fn(|i| word(0x440 + i * 4)),
            decay: std::array::from_fn(|i| word(0x5b8 + i * 4)),
            fnum: std::array::from_fn(|i| word(0x758 + i * 4)),
            attack_curve: std::array::from_fn(|i| {
                (4096.0 * ((4095 - i) as f64 / 4096.0).powi(8)) as i32
            }),
            sine: std::array::from_fn(|i| {
                let half = i & 1023;
                if half == 0 {
                    3328
                } else {
                    let angle = half.min(1024 - half) as f64 * std::f64::consts::PI / 1024.0;
                    ((-angle.sin().log10() * 20.0 * (4096.0 / 96.0)) as usize).min(3328)
                }
            }),
            level: std::array::from_fn(|i| {
                (67_108_863.0 / 10.0_f64.powf(i as f64 * 0.001171875)) as i32
            }),
        }
    }

    fn output(&self, phase: i32, attenuation: i32) -> i32 {
        if !(0..ATTENUATION_LIMIT).contains(&attenuation) {
            return 0;
        }
        let index = ((phase / 8192) & 2047) as usize;
        let volume = self
            .level
            .get(self.sine[index] + attenuation as usize)
            .copied()
            .unwrap_or(0);
        if index < 1024 {
            volume
        } else {
            -volume
        }
    }

    fn frequency(&self, high: u8, low: u8) -> (i32, u8) {
        let block = (high >> 3) & 7;
        let number = (usize::from(high & 7) << 8) | usize::from(low);
        (
            self.fnum[number] >> (7 - block),
            (block << 2) | FM_NOTES[number >> 7],
        )
    }
}

pub(super) struct LegacyFm {
    channels: [Channel; 6],
    tables: Box<Tables>,
    lfo_phase: u32,
    lfo_increment: u32,
    lfo_rates: [u32; 8],
    mode: u8,
    special: [(i32, u8); 3],
    special_high: [u8; 3],
}

impl LegacyFm {
    pub(super) fn restore(raw: &[u8]) -> Self {
        Self {
            channels: std::array::from_fn(|i| {
                let base = 0x2780 + i * 0x1ac;
                let bytes = &raw[base..base + 0x1ac];
                Channel {
                    operators: std::array::from_fn(|i| Operator::restore(&bytes[i * 0x60..])),
                    pan: usize::from(bytes[0x180] & 3),
                    algorithm: usize::from(bytes[0x181] & 7),
                    feedback: bytes[0x182].min(31),
                    history: [le32(bytes, 0x184) as i32, le32(bytes, 0x188) as i32],
                    pm: le32(bytes, 0x19c) as i32,
                    am: le32(bytes, 0x1a0) as i32,
                    frequency: le32(bytes, 0x1a4) as i32,
                    fnum_high: bytes[0x1a8],
                    key_code: bytes[0x1a9] & 31,
                    dirty: le32(bytes, 0x30) == u32::MAX,
                }
            }),
            tables: Box::new(Tables::restore(raw)),
            lfo_phase: le32(raw, 0x2758),
            lfo_increment: le32(raw, 0x275c),
            lfo_rates: std::array::from_fn(|i| le32(raw, 0x2760 + i * 4)),
            mode: raw[0x2c],
            special: std::array::from_fn(|i| {
                (le32(raw, 0x740 + i * 4) as i32, raw[0x74f + i] & 31)
            }),
            special_high: raw[0x74c..0x74f].try_into().unwrap(),
        }
    }

    pub(super) fn hash_state(&self, hash: &mut crc32fast::Hasher) {
        for value in [self.lfo_phase, self.lfo_increment] {
            hash.update(&value.to_le_bytes());
        }
        hash.update(&[self.mode]);
        for channel in &self.channels {
            hash.update(&[channel.pan as u8, channel.algorithm as u8, channel.feedback]);
            for value in channel.history {
                hash.update(&value.to_le_bytes());
            }
            for op in &channel.operators {
                for value in [
                    op.phase,
                    op.increment,
                    op.position,
                    op.target,
                    op.slope,
                    op.level,
                ] {
                    hash.update(&value.to_le_bytes());
                }
                hash.update(&op.stage.to_le_bytes());
            }
        }
    }

    #[cfg(test)]
    pub(super) fn compare_snapshot(&self, raw: &[u8]) -> [usize; 4] {
        let mut differences = [0; 4];
        for index in ACTIVE_CHANNELS {
            let base = 0x2780 + index * 0x1ac;
            for (slot, op) in self.channels[index].operators.iter().enumerate() {
                let start = base + slot * 0x60;
                for (i, (field, value)) in [
                    (0x2c, op.phase),
                    (0x30, op.increment),
                    (
                        0x34,
                        match op.stage {
                            ATTACK => 0x4036c0,
                            DECAY => 0x4036e0,
                            SUSTAIN => 0x403700,
                            RELEASE => 0x4030f0,
                            _ => 0,
                        },
                    ),
                    (0x38, op.position),
                ]
                .into_iter()
                .enumerate()
                {
                    differences[i] += usize::from(le32(raw, start + field) != value as u32);
                }
            }
        }
        differences
    }

    pub(super) fn sample(&mut self) -> [i32; 4] {
        let lfo = if self.lfo_increment != 0 {
            self.lfo_phase = self.lfo_phase.wrapping_add(self.lfo_increment);
            let phase = (self.lfo_phase >> 23) as i32;
            phase.min(512 - phase) << 8
        } else {
            0
        };
        let mut mix = [0i32; 4];
        for index in ACTIVE_CHANNELS {
            let channel = &mut self.channels[index];
            if channel.dirty {
                let base = (channel.frequency, channel.key_code);
                let pitch = if index == 2 && self.mode & 0xc0 != 0 {
                    [self.special[1], self.special[0], self.special[2], base]
                } else {
                    [base; 4]
                };
                for (operator, (frequency, code)) in channel.operators.iter_mut().zip(pitch) {
                    operator.reload(frequency, code, &self.tables);
                }
                channel.dirty = false;
            }
            let pm = if self.lfo_increment != 0 {
                channel.pm.wrapping_mul(lfo - 32768) / 65536
            } else {
                0
            };
            let amplitude = channel
                .operators
                .each_mut()
                .map(|op| op.step(pm, lfo, &self.tables));
            let mut bus = [0i32; 6];
            for (i, slot) in [0, 2, 1, 3].into_iter().enumerate() {
                let operator = &channel.operators[slot];
                let mut phase = operator.phase.wrapping_add(bus[i + 1]);
                if i == 0 && amplitude[slot] < ATTENUATION_LIMIT && channel.feedback != 0 {
                    phase = phase.wrapping_add(
                        channel.history[0].wrapping_add(channel.history[1]) >> channel.feedback,
                    );
                    channel.history[1] = channel.history[0];
                }
                let output = self.tables.output(phase, amplitude[slot]);
                if i == 0 && amplitude[slot] < ATTENUATION_LIMIT {
                    channel.history[0] = output;
                }
                let route = ROUTES[channel.algorithm][i];
                if route == 0 {
                    bus[2..=4].fill(output);
                } else {
                    bus[route] = bus[route].wrapping_add(output);
                }
            }
            mix[channel.pan] = mix[channel.pan].wrapping_add(bus[5]);
        }
        mix
    }

    pub(super) fn write(&mut self, bank: usize, register: u8, value: u8) {
        if bank == 0 && register < 0x30 {
            match register {
                0x22 => {
                    self.lfo_increment = if value & 8 != 0 {
                        self.lfo_rates[usize::from(value & 7)]
                    } else {
                        0
                    }
                }
                0x27 => {
                    self.mode = value;
                }
                0x28 if value & 3 != 3 => {
                    let channel = usize::from(value & 3) + if value & 4 != 0 { 3 } else { 0 };
                    for (i, slot) in [0, 2, 1, 3].into_iter().enumerate() {
                        self.channels[channel].operators[slot]
                            .key(value & (0x10 << i) != 0, &self.tables);
                    }
                }
                _ => {}
            }
            return;
        }
        let local_channel = usize::from(register & 3);
        if local_channel == 3 {
            return;
        }
        let index = bank * 3 + local_channel;
        let channel = &mut self.channels[index];
        let register_group = register & 0xf0;
        let slot = &mut channel.operators[usize::from((register >> 2) & 3)];
        match register_group {
            0x30 => {
                slot.detune = usize::from((value >> 4) & 7);
                slot.multiplier = (i32::from(value & 15) * 2).max(1);
                channel.dirty = true;
            }
            0x40 => {
                let level = i32::from(value & 127);
                slot.pending_level = (level * 129) >> 2;
                if index != 2 || self.mode & 0x80 == 0 {
                    slot.level = slot.pending_level;
                }
            }
            0x50 => {
                slot.scale_shift = 3 - (value >> 6);
                let rate = usize::from(value & 31);
                slot.set_rate(0, (rate != 0).then_some(rate * 2), &self.tables);
                channel.dirty = true;
            }
            0x60 | 0x70 => {
                if register_group == 0x60 {
                    slot.am_enable = value & 0x80 != 0;
                    slot.am = if slot.am_enable { channel.am } else { 0 };
                }
                let rate = usize::from(value & 31);
                slot.set_rate(
                    usize::from((register_group >> 4) - 5),
                    (rate != 0).then_some(rate * 2),
                    &self.tables,
                );
            }
            0x80 => {
                let level = i32::from(value >> 4);
                slot.sustain = ATTACK_END + ((if level == 15 { 31 } else { level }) << 23);
                slot.set_rate(3, Some(usize::from(value & 15) * 4 + 2), &self.tables);
            }
            0xa0 => match register & 12 {
                0 => {
                    (channel.frequency, channel.key_code) =
                        self.tables.frequency(channel.fnum_high, value);
                    channel.dirty = true;
                }
                4 => channel.fnum_high = value & 63,
                8 if bank == 0 => {
                    self.special[local_channel] = self
                        .tables
                        .frequency(self.special_high[local_channel], value);
                    self.channels[2].dirty = true;
                }
                12 if bank == 0 => self.special_high[local_channel] = value & 63,
                _ => {}
            },
            0xb0 if register & 4 == 0 => {
                channel.algorithm = usize::from(value & 7);
                let feedback = (value >> 3) & 7;
                channel.feedback = if feedback != 0 { 9 - feedback } else { 0 };
            }
            0xb0 => {
                channel.pan = usize::from(value >> 6);
                channel.am = [0, 59, 251, 503][usize::from((value >> 4) & 3)];
                channel.pm = [0, 3, 6, 9, 13, 19, 38, 77][usize::from(value & 7)];
                for op in &mut channel.operators {
                    op.am = if op.am_enable { channel.am } else { 0 };
                }
            }
            _ => {}
        }
    }
}
