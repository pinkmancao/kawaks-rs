use crate::save::le32;

const PHASE_UNIT: u32 = 1 << 16;
const FREQUENCY_BASE: f64 = 8_000_000.0 / (144.0 * 22_050.0);
const A_STEP: u32 = (FREQUENCY_BASE * 65_536.0 / 3.0) as u32;
const INDEX_DELTA: [i32; 8] = [-1, -1, -1, -1, 2, 5, 7, 9];
const B_STEP_SCALE: [i32; 8] = [57, 57, 57, 57, 77, 102, 128, 153];

#[derive(Default)]
struct Voice {
    playing: bool,
    nibble: u32,
    phase: u32,
    rate: u32,
    start: u32,
    end: u32,
    pan: usize,
    volume: i32,
    predictor: i32,
    step: i32,
    held: i32,
}

impl Voice {
    fn start(&mut self, step: i32, rom_size: usize) {
        self.playing = (self.start as usize) < rom_size && self.rate != 0;
        self.nibble = self.start * 2;
        self.phase = PHASE_UNIT.saturating_sub(self.rate);
        self.predictor = 0;
        self.step = step;
        self.held = 0;
    }

    fn decode(&mut self, rom: &[u8]) -> usize {
        let byte = rom.get((self.nibble / 2) as usize).copied().unwrap_or(0);
        let nibble = if self.nibble & 1 == 0 {
            byte >> 4
        } else {
            byte & 15
        };
        self.nibble = self.nibble.wrapping_add(1);
        usize::from(nibble)
    }
}

pub(super) struct LegacyAdpcm {
    a: [Voice; 6],
    a_levels: [u8; 6],
    total_level: u8,
    b: Voice,
    b_regs: [u8; 16],
    end_flags: u8,
    end_mask: u8,
    a_steps: [i32; 49],
}

impl LegacyAdpcm {
    pub(super) fn restore(fm: &[u8]) -> Self {
        let field = |offset| le32(fm, offset);
        let signed = |offset| field(offset) as i32;
        let a = std::array::from_fn(|channel| {
            let base = 0x3198 + channel * 0x38;
            Voice {
                playing: fm[base] != 0,
                nibble: field(base + 4),
                phase: field(base + 8) & 0xffff,
                rate: field(base + 12).min(PHASE_UNIT * 4),
                start: field(base + 16) & 0xff_ffff,
                end: field(base + 20) & 0xff_ffff,
                volume: signed(base + 28).clamp(0, 32768),
                pan: (field(base + 32) / 4).min(3) as usize,
                predictor: signed(base + 36).clamp(-6144, 6143),
                step: (signed(base + 40) / 16).clamp(0, 48),
                held: signed(base + 44).clamp(-0x1000_0000, 0x0fff_ffff),
            }
        });
        let b = Voice {
            playing: fm[0x33e0] != 0,
            nibble: field(0x33e4),
            phase: field(0x33e8) & 0xffff,
            rate: field(0x33ec).min(PHASE_UNIT * 4),
            start: field(0x33f0) & 0xff_ffff,
            end: field(0x33f4) & 0xff_ffff,
            volume: signed(0x33fc).clamp(0, 8192),
            pan: (field(0x3400) / 4).min(3) as usize,
            predictor: signed(0x3404).clamp(-32768, 32767),
            step: signed(0x3408).clamp(127, 24576),
            held: signed(0x340c).clamp(-0x1000_0000, 0x0fff_ffff),
        };
        Self {
            a,
            a_levels: std::array::from_fn(|ch| fm[0x3310 + ch * 4] & 31),
            total_level: fm[0x3309] & 63,
            b,
            b_regs: fm[0x33c8..0x33d8].try_into().unwrap(),
            end_flags: (fm[0x33a8] & 0x3f) | (fm[0x341c] & 0x80),
            end_mask: (0..6).fold(fm[0x33e1] & 0x80, |mask, ch| {
                mask | (fm[0x3199 + ch * 0x38] & (1 << ch))
            }),
            a_steps: std::array::from_fn(|i| (48.0 * 1.1_f64.powi(i as i32)) as i32),
        }
    }

    pub(super) fn hash_state(&self, hash: &mut crc32fast::Hasher) {
        hash.update(&[self.total_level, self.end_flags, self.end_mask]);
        hash.update(&self.a_levels);
        hash.update(&self.b_regs);
        for voice in self.a.iter().chain(std::iter::once(&self.b)) {
            hash.update(&[u8::from(voice.playing), voice.pan as u8]);
            for value in [
                voice.nibble,
                voice.phase,
                voice.rate,
                voice.start,
                voice.end,
            ] {
                hash.update(&value.to_le_bytes());
            }
            for value in [voice.volume, voice.predictor, voice.step, voice.held] {
                hash.update(&value.to_le_bytes());
            }
        }
    }

    pub(super) fn flags(&self) -> u8 {
        self.end_flags & self.end_mask
    }

    pub(super) fn sample(&mut self, rom: &[u8]) -> [[i32; 4]; 7] {
        let mut lanes = [[0; 4]; 7];
        let b = &mut self.b;
        if b.playing {
            b.phase += b.rate;
            while b.phase >= PHASE_UNIT {
                b.phase -= PHASE_UNIT;
                if b.nibble == b.end * 2 {
                    if self.b_regs[0] & 0x10 != 0 {
                        b.nibble = b.start * 2;
                        b.predictor = 0;
                        b.step = 127;
                    } else {
                        self.end_flags |= self.end_mask & 0x80;
                        b.playing = false;
                        b.held = 0;
                        break;
                    }
                }
                let nibble = b.decode(rom);
                b.held = b.predictor * b.volume;
                let delta = (1 + 2 * (nibble as i32 & 7)) * b.step / 8;
                let delta = if nibble & 8 == 0 { delta } else { -delta };
                b.predictor = (b.predictor + delta).clamp(-32768, 32767);
                b.step = (b.step * B_STEP_SCALE[nibble & 7] / 64).clamp(127, 24576);
            }
            lanes[0][b.pan] = b.held;
        }
        for (ch, voice) in self.a.iter_mut().enumerate() {
            if !voice.playing {
                continue;
            }
            voice.phase += voice.rate;
            while voice.phase >= PHASE_UNIT {
                voice.phase -= PHASE_UNIT;
                if (voice.nibble ^ (voice.end * 2)) & 0x1f_ffff == 0 {
                    voice.playing = false;
                    self.end_flags |= self.end_mask & (1 << ch);
                    break;
                }
                let nibble = voice.decode(rom);
                let delta = (1 + 2 * (nibble as i32 & 7)) * self.a_steps[voice.step as usize] / 8;
                let delta = if nibble & 8 == 0 { delta } else { -delta };
                voice.predictor = (voice.predictor + delta).clamp(-6144, 6143);
                voice.step = (voice.step + INDEX_DELTA[nibble & 7]).clamp(0, 48);
                voice.held = voice.predictor * voice.volume;
            }
            if voice.playing {
                lanes[ch + 1][voice.pan] = voice.held;
            }
        }
        lanes
    }

    pub(super) fn write(&mut self, bank: usize, address: u8, value: u8, rom_size: usize) {
        if bank == 0 {
            match address {
                0x10..=0x1b => self.write_b(usize::from(address - 0x10), value, rom_size),
                0x1c => {
                    self.end_mask = !value & 0xbf;
                    self.end_flags &= self.end_mask;
                }
                _ => {}
            }
            return;
        }
        let channel = usize::from(address & 7);
        match address {
            0 => {
                for (ch, voice) in self.a.iter_mut().enumerate() {
                    if value & (1 << ch) != 0 {
                        if value & 0x80 == 0 {
                            voice.rate = A_STEP;
                            voice.start(0, rom_size);
                        } else {
                            voice.playing = false;
                        }
                    }
                }
            }
            1 => {
                self.total_level = value & 63;
                for ch in 0..6 {
                    self.update_a_volume(ch);
                }
            }
            8..=13 => {
                self.a_levels[channel] = value & 31;
                self.a[channel].pan = usize::from(value >> 6);
                self.update_a_volume(channel);
            }
            0x10..=0x15 => {
                self.a[channel].start = (self.a[channel].start & 0xff0000) | (u32::from(value) << 8)
            }
            0x18..=0x1d => {
                self.a[channel].start = (self.a[channel].start & 0xff00) | (u32::from(value) << 16)
            }
            0x20..=0x25 => {
                self.a[channel].end =
                    (self.a[channel].end & 0xff0000) | (u32::from(value) << 8) | 255
            }
            0x28..=0x2d => {
                self.a[channel].end = (self.a[channel].end & 0xffff) | (u32::from(value) << 16)
            }
            _ => {}
        }
    }

    fn update_a_volume(&mut self, channel: usize) {
        let attenuation = f64::from((63 - self.total_level) + (31 - self.a_levels[channel]));
        let voice = &mut self.a[channel];
        voice.volume = (32768.0 * 10.0_f64.powf(-attenuation * 0.75 / 20.0)) as i32;
        voice.held = voice.predictor * voice.volume;
    }

    fn write_b(&mut self, register: usize, value: u8, rom_size: usize) {
        self.b_regs[register] = value;
        let word = |offset| {
            u32::from(u16::from_le_bytes([
                self.b_regs[offset],
                self.b_regs[offset + 1],
            ]))
        };
        match register {
            0 if value & 0x80 != 0 => {
                self.b.end = self.b.end.min(rom_size.saturating_sub(1) as u32);
                self.b.start(127, rom_size);
            }
            0 if value & 1 != 0 => self.b.playing = false,
            1 => self.b.pan = usize::from(value >> 6),
            2 | 3 => self.b.start = word(2) << 8,
            4 | 5 => self.b.end = (word(4) << 8) | 255,
            9 | 10 => self.b.rate = (f64::from(word(9)) * FREQUENCY_BASE) as u32,
            11 => {
                let old = self.b.volume;
                self.b.volume = i32::from(value) * 32;
                if old != 0 {
                    self.b.held =
                        (i64::from(self.b.held) * i64::from(self.b.volume) / i64::from(old)) as i32;
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{FrameInput, NeoGeo, NeoKwk4, NeoRoms, AUDIO_SAMPLES_PER_FRAME};
    use std::{fs, path::PathBuf};

    #[test]
    #[ignore = "requires WinKawaks Neo Geo mixer trace and local KOF97 ROMs"]
    fn adpcm_voices_match_win165() {
        let project = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let fixture = std::env::var_os("KAWAKS_FIXTURE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| project.join(".."));
        let capture = std::env::var_os("KAWAKS_NEO_COMPONENT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| project.join("outputs/kof97-ym2610-reference"));
        let save = NeoKwk4::read(&fs::read(capture.join("saves/kof97.ks0")).unwrap()).unwrap();
        let roms = NeoRoms::load_kof97(
            fixture.join("roms/neogeo/kof97.zip"),
            fixture.join("roms/neogeo/neogeo.zip"),
        )
        .unwrap();
        let mut machine = NeoGeo::from_kwk4(roms, save).unwrap();
        let log = fs::read_to_string(capture.join("slot0/capture.log")).unwrap();
        let mut words = log
            .lines()
            .filter(|line| !line.contains(".printf"))
            .flat_map(str::split_whitespace);
        let mut samples = 0;
        let mut previous = [0; 3];
        let mut lanes = [[0; 4]; 7];
        let mut differences = [0; 7];
        let mut machine_frames = 0;
        let mut fm_changes = 0;
        let mut point_count = 0;
        let mut last_point = None;
        while let Some(tag) = words.next() {
            if tag != "D" && tag != "F" && tag != "A" {
                continue;
            }
            let index = if tag == "D" {
                if samples % AUDIO_SAMPLES_PER_FRAME == 0 {
                    machine.step_frame(FrameInput::default()).unwrap();
                    machine_frames += 1;
                }
                let rendered = machine.main.mem.restored_audio.as_ref().unwrap();
                lanes = rendered.voice_trace[samples % AUDIO_SAMPLES_PER_FRAME];
                previous = [0; 3];
                samples += 1;
                0
            } else {
                usize::from_str_radix(words.next().unwrap(), 16).unwrap()
            };
            let current: [i32; 3] = std::array::from_fn(|_| {
                u32::from_str_radix(words.next().unwrap(), 16).unwrap() as i32
            });
            let point = (tag, index, current);
            if tag != "D" && last_point == Some(point) {
                // CDB may emit a duplicate line while the debugger logs a burst.
                continue;
            }
            last_point = Some(point);
            assert_eq!(
                point_count % 11,
                match tag {
                    "D" => 0,
                    "F" => 1 + index,
                    _ => 5 + index,
                },
                "sample {samples} trace point order"
            );
            point_count += 1;
            if tag != "F" {
                let lane_index = if tag == "D" { 0 } else { index + 1 };
                let lane = lanes[lane_index];
                for (offset, pan) in [2, 1, 3].into_iter().enumerate() {
                    if current[offset] - previous[offset] != lane[pan] {
                        differences[lane_index] += 1;
                        if differences[lane_index] == 1 {
                            eprintln!("sample {samples} lane {lane_index} pan {pan}: WinKawaks {}, Rust {}", current[offset] - previous[offset], lane[pan]);
                        }
                    }
                }
            } else {
                fm_changes += usize::from(current != previous);
            }
            previous = current;
        }
        assert_eq!(machine_frames, 24);
        assert_eq!(point_count, samples * 11);
        assert_eq!(samples, 24 * AUDIO_SAMPLES_PER_FRAME);
        assert_eq!(
            fm_changes, 0,
            "the reference should have no active FM output"
        );
        assert_eq!(
            differences, [0; 7],
            "Delta-T, then ADPCM-A channels 0 through 5"
        );
    }
}
