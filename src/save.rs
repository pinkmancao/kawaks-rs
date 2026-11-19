use std::io;

use flate2::{Decompress, FlushDecompress, Status};

use crate::invalid;

/// The 1.65 CPS1 QSound layout. KWK4 has no game identity in its header.
pub struct Kwk4 {
    pub(crate) cps2: Option<Cps2State>,
    pub(crate) work: Vec<u8>,
    pub(crate) registers: Vec<u8>,
    pub(crate) graphics: Vec<u8>,
    pub(crate) main: [u8; 0x80],
    pub(crate) sound_ram: Vec<u8>,
    pub(crate) sound: [u8; 0x64],
    pub(crate) sound_bank: u8,
    pub(crate) eeprom: [u8; 0x80],
    pub(crate) channels: [u8; 0x300],
    pub(crate) mixer: [u8; 0x2c],
    pub(crate) tail: [u8; 0x20],
}

pub(crate) struct Cps2State {
    pub(crate) objects: Vec<u8>,
    pub(crate) output: [u8; 16],
    pub(crate) tail: [u8; 0x14],
    pub(crate) sound_enabled: bool,
}

impl Kwk4 {
    pub fn read_cps2(bytes: &[u8]) -> io::Result<Self> {
        let mut reader = Reader(bytes);
        if reader.take(4, "signature")? != b"KWK4" {
            return Err(invalid("expected KWK4"));
        }
        let work = reader.chunk(0x10000, "CPS2 work RAM")?;
        let registers = reader.chunk(0x200, "CPS2 registers")?;
        let graphics = reader.chunk(0x30000, "CPS2 graphics RAM")?;
        let objects = reader.chunk(0x10000, "CPS2 object RAM banks")?;
        let output = reader
            .chunk(0x10, "CPS2 object output")?
            .try_into()
            .unwrap();
        let main = reader.fixed("68000 state")?;
        let channels = reader.fixed("QSound channels")?;
        let sound_ram = reader.chunk(0x4000, "Z80 RAM")?;
        let sound = reader.fixed("Z80 state")?;
        let sound_bank = u32::from_le_bytes(reader.fixed("Z80 bank")?) as u8 & 15;
        let sound_enabled = u32::from_le_bytes(reader.fixed("Z80 control")?) != 0;
        let eeprom = reader.fixed("EEPROM")?;
        let tail = reader.fixed("CPS2 driver state")?;
        if !reader.0.is_empty() {
            return Err(invalid("unexpected bytes after CPS2 KWK4 state"));
        }
        Ok(Self {
            cps2: Some(Cps2State {
                objects,
                output,
                tail,
                sound_enabled,
            }),
            work,
            registers,
            graphics,
            main,
            sound_ram,
            sound,
            sound_bank,
            eeprom,
            channels,
            mixer: [0; 0x2c],
            tail: [0xff; 0x20],
        })
    }

    pub fn read(bytes: &[u8]) -> io::Result<Self> {
        let mut reader = Reader(bytes);
        if reader.take(4, "signature")? != b"KWK4" {
            return Err(invalid("expected KWK4"));
        }
        let result = Self {
            cps2: None,
            work: reader.chunk(0x10000, "work RAM")?,
            registers: reader.chunk(0x200, "CPS registers")?,
            graphics: reader.chunk(0x30000, "graphics RAM")?,
            main: reader.fixed("68000 state")?,
            sound_ram: reader.chunk(0x4000, "Z80 RAM")?,
            sound: reader.fixed("Z80 state")?,
            sound_bank: u32::from_le_bytes(reader.fixed("Z80 bank")?) as u8 & 15,
            eeprom: {
                reader.take(1, "Z80 compatibility byte")?;
                reader.fixed("EEPROM")?
            },
            channels: reader.fixed("QSound channels")?,
            mixer: reader.fixed("QSound mixer state")?,
            tail: reader.fixed("CPS1 driver state")?,
        };
        if !reader.0.is_empty() {
            return Err(invalid("unexpected bytes after CPS1 QSound KWK4 state"));
        }
        Ok(result)
    }

    pub fn main_pc(&self) -> u32 {
        le32(&self.main, 0x50)
    }

    pub fn sound_pc(&self) -> u16 {
        le16(&self.sound, 0x50)
    }
}

pub(crate) struct Reader<'a>(pub(crate) &'a [u8]);

impl<'a> Reader<'a> {
    pub(crate) fn take(&mut self, size: usize, field: &str) -> io::Result<&'a [u8]> {
        let (head, tail) = self
            .0
            .split_at_checked(size)
            .ok_or_else(|| invalid(format!("truncated {field}")))?;
        self.0 = tail;
        Ok(head)
    }

    pub(crate) fn fixed<const N: usize>(&mut self, field: &str) -> io::Result<[u8; N]> {
        Ok(self.take(N, field)?.try_into().unwrap())
    }

    pub(crate) fn chunk(&mut self, size: usize, field: &str) -> io::Result<Vec<u8>> {
        let length = u32::from_le_bytes(self.fixed(field)?) as usize;
        let compressed = self.take(length, field)?;
        let mut decoder = Decompress::new(true);
        let mut dest = vec![0; size + 1];
        let status = decoder
            .decompress(compressed, &mut dest, FlushDecompress::Finish)
            .map_err(|err| invalid(format!("invalid zlib data in {field}: {err}")))?;
        if status != Status::StreamEnd
            || decoder.total_in() != length as u64
            || decoder.total_out() != size as u64
        {
            return Err(invalid(format!(
                "incorrect compressed or decoded size for {field}"
            )));
        }
        dest.truncate(size);
        Ok(dest)
    }
}

pub(crate) fn le16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

pub(crate) fn le32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

pub(crate) fn swap_words(bytes: &mut [u8]) {
    for word in bytes.chunks_exact_mut(2) {
        word.swap(0, 1);
    }
}
