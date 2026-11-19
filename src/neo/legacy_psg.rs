use crate::save::le32;

const SAMPLE_STEP: i32 = 0x8000;

struct Counter {
    period: i32,
    remaining: i32,
}

impl Counter {
    fn retune(&mut self, period: i32) {
        self.remaining = (i64::from(self.remaining) + i64::from(period) - i64::from(self.period))
            .clamp(1, i64::from(i32::MAX / 2)) as i32;
        self.period = period;
    }

    fn defer(&mut self, duration: i32) {
        if self.remaining <= duration {
            self.remaining += duration;
        }
    }

    fn tone(&mut self, high: &mut bool, duration: i32) -> i32 {
        let first = duration.min(self.remaining);
        let mut on = if *high { first } else { 0 };
        self.remaining -= first;
        if self.remaining > 0 {
            return on;
        }
        *high = !*high;
        let (periods, tail) = (
            (duration - first) / self.period,
            (duration - first) % self.period,
        );
        on += ((periods + i32::from(*high)) / 2) * self.period;
        *high ^= periods & 1 != 0;
        if *high {
            on += tail;
        }
        self.remaining = self.period - tail;
        on
    }
}

pub(super) struct LegacyPsg {
    regs: [u8; 16],
    update_step: i32,
    counters: [Counter; 5],
    volume: [i32; 3],
    level_table: [i32; 32],
    envelope_volume: i32,
    tone_high: [bool; 3],
    noise_high: bool,
    rng: u32,
    envelope_level: i32,
    hold: bool,
    alternate: bool,
    attack: usize,
    holding: bool,
}

impl LegacyPsg {
    pub(super) fn restore(psg: &[u8; 0xf8]) -> Self {
        let level = |offset| (le32(psg, offset) as i32).clamp(0, 32767);
        Self {
            regs: psg[0x1c..0x2c].try_into().unwrap(),
            update_step: (le32(psg, 0x2c) as i32).clamp(1, SAMPLE_STEP),
            counters: std::array::from_fn(|counter| Counter {
                period: (le32(psg, 0x30 + counter * 4) as i32).clamp(1, 0x7fff_ffff / 2),
                remaining: (le32(psg, 0x44 + counter * 4) as i32).clamp(1, 0x7fff_ffff / 2),
            }),
            volume: std::array::from_fn(|i| level(0x58 + i * 4)),
            level_table: std::array::from_fn(|i| level(0x78 + i * 4)),
            envelope_volume: level(0x64),
            tone_high: std::array::from_fn(|i| psg[0x6b + i] != 0),
            noise_high: psg[0x6e] != 0,
            rng: (le32(psg, 0x74) & 0x1ffff).max(1),
            envelope_level: i32::from(psg[0x6f] & 31),
            hold: psg[0x70] != 0,
            alternate: psg[0x71] != 0,
            attack: usize::from(psg[0x72] & 31),
            holding: psg[0x73] != 0,
        }
    }

    pub(super) fn hash_state(&self, hash: &mut crc32fast::Hasher) {
        hash.update(&self.regs);
        for counter in &self.counters {
            hash.update(&counter.period.to_le_bytes());
            hash.update(&counter.remaining.to_le_bytes());
        }
        hash.update(&self.rng.to_le_bytes());
        hash.update(&self.tone_high.map(u8::from));
        hash.update(&[
            u8::from(self.noise_high),
            self.envelope_level as u8,
            self.attack as u8,
            u8::from(self.holding),
        ]);
    }

    #[cfg(test)]
    pub(super) fn compare_phase(&self, raw: &[u8]) -> [usize; 3] {
        let periods = self
            .counters
            .iter()
            .enumerate()
            .filter(|(i, c)| le32(raw, 0x44 + i * 4) != c.remaining as u32)
            .count();
        let high = self
            .tone_high
            .iter()
            .enumerate()
            .filter(|(i, high)| (raw[0x6b + i] != 0) != **high)
            .count();
        let envelope = usize::from(raw[0x6f] & 31 != self.envelope_level as u8);
        [periods, high, envelope]
    }

    pub(super) fn begin_chunk(&mut self, samples: usize) {
        let duration = samples as i32 * SAMPLE_STEP;
        for i in 0..3 {
            let disconnected = self.regs[7] & (1 << i) != 0;
            if disconnected || self.regs[8 + i] == 0 {
                self.counters[i].defer(duration);
                if disconnected {
                    self.tone_high[i] = true;
                }
            }
        }
        if self.regs[7] & 0x38 == 0x38 {
            self.counters[3].defer(duration);
        }
    }

    pub(super) fn sample(&mut self) -> [i32; 3] {
        let mut left = SAMPLE_STEP;
        let mut high_time = [0; 3];
        while left != 0 {
            let duration = self.counters[3].remaining.min(left);
            for (i, high) in high_time.iter_mut().enumerate() {
                let tone = self.counters[i].tone(&mut self.tone_high[i], duration);
                if self.noise_high || self.regs[7] & (8 << i) != 0 {
                    *high += tone;
                }
            }
            left -= duration;
            self.counters[3].remaining -= duration;
            if self.counters[3].remaining == 0 {
                self.noise_high ^= (self.rng + 1) & 2 != 0;
                if self.rng & 1 != 0 {
                    self.rng ^= 0x28000;
                }
                self.rng >>= 1;
                self.counters[3].remaining = self.counters[3].period;
            }
        }
        if !self.holding {
            let envelope = &mut self.counters[4];
            envelope.remaining -= SAMPLE_STEP;
            if envelope.remaining <= 0 {
                let steps = 1 + (-envelope.remaining) / envelope.period;
                envelope.remaining += steps * envelope.period;
                self.envelope_level -= steps;
                if self.envelope_level < 0 {
                    if self.hold {
                        if self.alternate {
                            self.attack ^= 31;
                        }
                        self.holding = true;
                        self.envelope_level = 0;
                    } else {
                        if self.alternate && self.envelope_level & 32 != 0 {
                            self.attack ^= 31;
                        }
                        self.envelope_level &= 31;
                    }
                }
                self.update_envelope_volume();
            }
        }
        std::array::from_fn(|i| high_time[i] * self.volume[i] / SAMPLE_STEP)
    }

    pub(super) fn write(&mut self, register: u8, value: u8) {
        let r = usize::from(register);
        if r >= 16 {
            return;
        }
        self.regs[r] = match r {
            1 | 3 | 5 | 13 => value & 15,
            6 | 8..=10 => value & 31,
            _ => value,
        };
        match r {
            0..=5 => {
                let zone = r / 2;
                let packed = u16::from_le_bytes([self.regs[zone * 2], self.regs[zone * 2 + 1]]);
                self.counters[zone].retune(i32::from(packed.max(1)) * self.update_step);
            }
            6 => self.counters[3].retune(i32::from(self.regs[6].max(1)) * self.update_step),
            8..=10 => self.update_volume(r - 8),
            11 | 12 => {
                let packed = u16::from_le_bytes([self.regs[11], self.regs[12]]);
                let period = (i32::from(packed) * self.update_step)
                    .max(self.update_step / 2)
                    .max(1);
                self.counters[4].retune(period);
            }
            13 => {
                self.attack = if value & 4 != 0 { 31 } else { 0 };
                self.hold = value & 8 == 0 || value & 1 != 0;
                self.alternate = if value & 8 == 0 {
                    self.attack != 0
                } else {
                    value & 2 != 0
                };
                self.envelope_level = 31;
                self.holding = false;
                self.counters[4].remaining = self.counters[4].period;
                self.update_envelope_volume();
            }
            _ => {}
        }
    }

    fn update_volume(&mut self, channel: usize) {
        let level = usize::from(self.regs[8 + channel] & 31);
        self.volume[channel] = match level {
            0 => 0,
            1..=15 => self.level_table[level * 2 + 1],
            _ => self.envelope_volume,
        };
    }

    fn update_envelope_volume(&mut self) {
        self.envelope_volume = self.level_table[self.envelope_level as usize ^ self.attack];
        for i in 0..3 {
            if self.regs[8 + i] & 16 != 0 {
                self.volume[i] = self.envelope_volume;
            }
        }
    }
}
