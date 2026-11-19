use crate::save::le32;

pub(super) struct Timers {
    pub(super) status: u8,
    mask: u8,
    mode: u8,
    values: [u32; 2],
    counts: [u32; 2],
    periods: [u32; 2],
}

impl Timers {
    pub(super) fn restore(fm: &[u8], timers: &[u8; 0x14]) -> Self {
        Self {
            status: fm[0x2b] & 0x83,
            mask: fm[0x2a] & 3,
            mode: fm[0x2c],
            values: [le32(fm, 0x30) & 1023, u32::from(fm[0x38])],
            counts: [le32(timers, 4), le32(timers, 8)],
            periods: [le32(timers, 12), le32(timers, 16)],
        }
    }

    pub(super) fn hash_state(&self, hash: &mut crc32fast::Hasher) {
        hash.update(&[self.status, self.mask, self.mode]);
        for value in self
            .values
            .into_iter()
            .chain(self.counts)
            .chain(self.periods)
        {
            hash.update(&value.to_le_bytes());
        }
    }

    pub(super) fn irq(&self) -> bool {
        self.status & self.mask != 0
    }

    fn period(&self, i: usize) -> u32 {
        let counts = if i == 0 {
            1024 - self.values[i]
        } else {
            (256 - self.values[i]) * 16
        };
        // 6 MHz Z80 cycles per 144 master clocks at 8 MHz.
        counts * 108
    }

    pub(super) fn write(&mut self, address: u8, value: u8) {
        match address {
            0x24 => self.values[0] = (self.values[0] & 3) | (u32::from(value) << 2),
            0x25 => self.values[0] = (self.values[0] & !3) | (u32::from(value) & 3),
            0x26 => self.values[1] = u32::from(value),
            0x27 => {
                self.mode = value;
                self.status &= !((value >> 4) & 3);
                for i in 0..2 {
                    if value & (1 << i) == 0 {
                        self.periods[i] = 0;
                    } else if self.periods[i] == 0 {
                        self.periods[i] = self.period(i);
                    }
                }
            }
            _ => {}
        }
    }

    // WinKawaks calls the timers after each 384-cycle slice and
    // discards the excess cycles when a period expires.
    pub(super) fn advance(&mut self, cycles: u32) -> bool {
        let mut a_expired = false;
        for i in 0..2 {
            self.counts[i] = self.counts[i].wrapping_add(cycles);
            if self.periods[i] != 0 && self.counts[i] >= self.periods[i] {
                self.counts[i] = 0;
                self.periods[i] = self.period(i);
                if self.mode & (4 << i) != 0 {
                    self.status |= 1 << i;
                }
                a_expired |= i == 0;
            }
        }
        a_expired
    }
}
