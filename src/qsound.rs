use crate::save::le32;

pub const AUDIO_SAMPLE_RATE: u32 = 22_050;
pub const AUDIO_SAMPLES_PER_FRAME: usize = 367;
const PHASE_BITS: u32 = 16;

#[derive(Clone, Copy, Default)]
struct Voice {
    bank: u32,
    address: u32,
    step: u32,
    loop_length: u32,
    end: u32,
    volume: u16,
    active: bool,
    gain: [u16; 2],
    held: i8,
    phase: u32,
}

impl Voice {
    fn next(&mut self, rom: &[u8]) -> [i32; 2] {
        if !self.active || self.bank as usize >= rom.len() {
            return [0; 2];
        }
        let advance = self.phase >> PHASE_BITS;
        self.phase &= (1 << PHASE_BITS) - 1;
        if advance != 0 {
            self.address += advance;
            if self.address >= self.end {
                if self.loop_length == 0 {
                    self.active = false;
                    return [0; 2];
                }
                self.address = self.end.wrapping_sub(self.loop_length) & 0xffff;
            }
            self.held = rom
                .get((self.bank + self.address) as usize)
                .copied()
                .unwrap_or(0) as i8;
        }
        self.phase += self.step;
        self.gain.map(|gain| {
            let volume = (i32::from(gain) * i32::from(self.volume)) >> 8;
            (i32::from(self.held) * volume) >> 5
        })
    }
}

#[derive(Clone, Default)]
pub(crate) struct QSound {
    voices: [Voice; 16],
    pub pcm: Vec<[i16; 2]>,
}

impl QSound {
    pub fn restore(&mut self, channels: &[u8; 0x300]) {
        for (voice, record) in self.voices.iter_mut().zip(channels.chunks_exact(0x30)) {
            // WinKawaks saves its mixer state as twelve little-endian dwords.
            // The saved pitch step and pan gains were already scaled by WinKawaks.
            *voice = Voice {
                bank: le32(record, 0) & 0x7f0000,
                address: le32(record, 4) & 0xffff,
                step: le32(record, 8).min(0x11_ffff),
                loop_length: le32(record, 0x0c) & 0xffff,
                end: le32(record, 0x10) & 0xffff,
                volume: le32(record, 0x14) as u16,
                active: le32(record, 0x1c) != 0,
                gain: [0x20, 0x24].map(|offset| le32(record, offset).min(256) as u16),
                held: le32(record, 0x28) as i8,
                phase: le32(record, 0x2c).min(0x12_ffff),
            };
        }
    }

    pub fn write(&mut self, register: u8, data: u16) {
        match register {
            0x00..=0x7f => {
                let channel = usize::from(register / 8);
                let value = u32::from(data);
                let voice = &mut self.voices[channel];
                match register & 7 {
                    0 => self.voices[(channel + 1) & 15].bank = (value & 0x7f) << 16,
                    1 => voice.address = value,
                    2 => {
                        // WinKawaks uses an effective QSound clock of 4 MHz / 166,
                        // converts Q4.12 to Q16.16, then rescales to the mixer rate.
                        voice.step = (u64::from(value) * 64_000_000
                            / (166 * u64::from(AUDIO_SAMPLE_RATE)))
                            as u32;
                        if data == 0 {
                            voice.active = false;
                        }
                    }
                    4 => voice.loop_length = value,
                    5 => voice.end = value,
                    6 => {
                        voice.volume = data;
                        if data != 0 && !voice.active {
                            voice.phase = 0;
                            voice.held = 0;
                        }
                        voice.active = data != 0;
                    }
                    _ => {}
                }
            }
            0x80..=0x8f => {
                let pan = u32::from(data.wrapping_sub(0x10) & 0x3f).min(32);
                self.voices[usize::from(register - 0x80)].gain =
                    [32 - pan, pan].map(|side| (side * 2048).isqrt() as u16);
            }
            _ => {}
        }
    }

    pub fn mix_frame(&mut self, rom: &[u8]) {
        // WinKawaks generates a complete segment after all sound CPU slices.
        self.mix(rom, AUDIO_SAMPLES_PER_FRAME);
    }

    fn mix(&mut self, rom: &[u8], count: usize) {
        self.pcm.clear();
        for _ in 0..count {
            let mut stereo = [0i32; 2];
            for voice in &mut self.voices {
                let sample = voice.next(rom);
                for channel in 0..2 {
                    let mixed = stereo[channel] + sample[channel];
                    stereo[channel] = if mixed < i32::from(i16::MIN) {
                        -32767
                    } else {
                        mixed.min(i32::from(i16::MAX))
                    };
                }
            }
            self.pcm.push(stereo.map(|value| value as i16));
        }
    }

    pub fn hash_state(&self, hash: &mut crc32fast::Hasher) {
        for voice in self.voices {
            for value in [
                voice.bank,
                voice.address,
                voice.step,
                voice.loop_length,
                voice.end,
                voice.phase,
                u32::from(voice.volume),
                u32::from(voice.gain[0]),
                u32::from(voice.gain[1]),
            ] {
                hash.update(&value.to_le_bytes());
            }
            hash.update(&[u8::from(voice.active), voice.held as u8]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_commands_select_the_next_voice_bank_and_start_playback() {
        let mut mixer = QSound::default();
        let mut rom = vec![0; 0x10008];
        rom[0x10002] = 80;
        mixer.write(0, 1);
        mixer.write(9, 1);
        mixer.write(10, 4096);
        mixer.write(13, 8);
        mixer.write(0x81, 0x10);
        mixer.write(14, 512);
        mixer.mix(&rom, 3);
        assert_eq!(mixer.pcm, [[0, 0], [1280, 0], [0, 0]]);
    }

    #[test]
    fn restored_voice_honors_phase_loop_and_stereo_gains() {
        let mut state = [0; 0x300];
        for (offset, value) in [
            (4, 1),
            (8, 65536),
            (12, 2),
            (16, 4),
            (20, 256),
            (28, 1),
            (32, 256),
            (36, 128),
            (40, -16),
            (44, 65536),
        ] {
            state[offset..offset + 4].copy_from_slice(&i32::to_le_bytes(value));
        }
        let mut mixer = QSound::default();
        mixer.restore(&state);
        mixer.mix(&[0, 0, 64, 192], 3);
        assert_eq!(mixer.pcm, [[512, 256], [-512, -256], [512, 256]]);
        mixer.write(6, 0);
        mixer.mix(&[0, 0, 64, 192], 1);
        assert_eq!(mixer.pcm[0], [0, 0]);
    }
}
