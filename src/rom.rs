use std::{
    fs::File,
    io::{self, Read},
    path::Path,
    sync::Arc,
};

use crate::{invalid, kabuki::decode_fixed_rom, save::swap_words};

#[derive(Clone)]
pub struct DinoRoms(pub(crate) Arc<Regions>);

pub(crate) struct Regions {
    pub main_opcodes: Option<Vec<u8>>,
    pub game: Game,
    pub program: Vec<u8>,
    pub graphics: Vec<u8>,
    pub sound: Vec<u8>,
    pub sound_data: Vec<u8>,
    pub sound_opcodes: Vec<u8>,
    pub samples: Vec<u8>,
}

#[derive(Clone, Copy)]
enum Placement {
    Linear,
    GraphicsLane,
    ByteLane,
    WordsSwapped,
}

pub(crate) struct Part {
    name: &'static str,
    crc: u32,
    offset: usize,
    length: usize,
    placement: Placement,
}

const DINO_PROGRAM: &[Part] = &[
    Part::word("cde_23a.8f", 0x8f4e585e, 0, 0x80000),
    Part::word("cde_22a.7f", 0x9278aa12, 0x80000, 0x80000),
    Part::word("cde_21a.6f", 0x66d23de2, 0x100000, 0x80000),
];

const DINO_GRAPHICS: &[Part] = &[
    Part::graphics("cd-1m.3a", 0x8da4f917, 0, 0x80000),
    Part::graphics("cd-3m.5a", 0x6c40f603, 2, 0x80000),
    Part::graphics("cd-2m.4a", 0x09c8fc2d, 4, 0x80000),
    Part::graphics("cd-4m.6a", 0x637ff38f, 6, 0x80000),
    Part::graphics("cd-5m.7a", 0x470befee, 0x200000, 0x80000),
    Part::graphics("cd-7m.9a", 0x22bfb7a3, 0x200002, 0x80000),
    Part::graphics("cd-6m.8a", 0xe7599ac4, 0x200004, 0x80000),
    Part::graphics("cd-8m.10a", 0x211b4b15, 0x200006, 0x80000),
];

const DINO_SOUND: &[Part] = &[Part::linear("cd_q.5k", 0x605fdb0b, 0, 0x20000)];

const DINO_SAMPLES: &[Part] = &[
    Part::linear("cd-q1.1k", 0x60927775, 0, 0x80000),
    Part::linear("cd-q2.2k", 0x770f4c47, 0x80000, 0x80000),
    Part::linear("cd-q3.3k", 0x2f273ffc, 0x100000, 0x80000),
    Part::linear("cd-q4.4k", 0x2c67821d, 0x180000, 0x80000),
];

const WOF_PROGRAM: &[Part] = &[
    Part::word("tk2e_23c.8f", 0x0d708505, 0x000000, 0x80000),
    Part::word("tk2e_22c.7f", 0x608c17e3, 0x080000, 0x80000),
];

const WOF_GRAPHICS: &[Part] = &[
    Part::graphics("tk2-1m.3a", 0x0d9cb9bf, 0x000000, 0x80000),
    Part::graphics("tk2-3m.5a", 0x45227027, 0x000002, 0x80000),
    Part::graphics("tk2-2m.4a", 0xc5ca2460, 0x000004, 0x80000),
    Part::graphics("tk2-4m.6a", 0xe349551c, 0x000006, 0x80000),
    Part::graphics("tk2-5m.7a", 0x291f0f0b, 0x200000, 0x80000),
    Part::graphics("tk2-7m.9a", 0x3edeb949, 0x200002, 0x80000),
    Part::graphics("tk2-6m.8a", 0x1abd14d6, 0x200004, 0x80000),
    Part::graphics("tk2-8m.10a", 0xb27948e3, 0x200006, 0x80000),
];

const WOF_SOUND: &[Part] = &[Part::linear("tk2_qa.5k", 0xc9183a0d, 0x00000, 0x20000)];

const WOF_SAMPLES: &[Part] = &[
    Part::linear("tk2-q1.1k", 0x611268cf, 0x000000, 0x80000),
    Part::linear("tk2-q2.2k", 0x20f55ca9, 0x080000, 0x80000),
    Part::linear("tk2-q3.3k", 0xbfcf6f52, 0x100000, 0x80000),
    Part::linear("tk2-q4.4k", 0x36642e88, 0x180000, 0x80000),
];

const WOFA_PROGRAM: &[Part] = &[
    Part::word("tk2a_23c.8f", 0x2e024628, 0x000000, 0x80000),
    Part::word("tk2a_22c.7f", 0x900ad4cd, 0x080000, 0x80000),
];

const PUNISHER_PROGRAM: &[Part] = &[
    Part::byte("pse_26.11e", 0x389a99d2, 0x000000, 0x20000),
    Part::byte("pse_30.11f", 0x68fb06ac, 0x000001, 0x20000),
    Part::byte("pse_27.12e", 0x3eb181c3, 0x040000, 0x20000),
    Part::byte("pse_31.12f", 0x37108e7b, 0x040001, 0x20000),
    Part::byte("pse_24.9e", 0x0f434414, 0x080000, 0x20000),
    Part::byte("pse_28.9f", 0xb732345d, 0x080001, 0x20000),
    Part::byte("pse_25.10e", 0xb77102e2, 0x0c0000, 0x20000),
    Part::byte("pse_29.10f", 0xec037bce, 0x0c0001, 0x20000),
    Part::word("ps_21.6f", 0x8affa5a9, 0x100000, 0x80000),
];

const PUNISHER_GRAPHICS: &[Part] = &[
    Part::graphics("ps-1m.3a", 0x77b7ccab, 0x000000, 0x80000),
    Part::graphics("ps-3m.5a", 0x0122720b, 0x000002, 0x80000),
    Part::graphics("ps-2m.4a", 0x64fa58d4, 0x000004, 0x80000),
    Part::graphics("ps-4m.6a", 0x60da42c8, 0x000006, 0x80000),
    Part::graphics("ps-5m.7a", 0xc54ea839, 0x200000, 0x80000),
    Part::graphics("ps-7m.9a", 0x04c5acbd, 0x200002, 0x80000),
    Part::graphics("ps-6m.8a", 0xa544f4cc, 0x200004, 0x80000),
    Part::graphics("ps-8m.10a", 0x8f02f436, 0x200006, 0x80000),
];

const PUNISHER_SOUND: &[Part] = &[Part::linear("ps_q.5k", 0x49ff4446, 0x00000, 0x20000)];

const PUNISHER_SAMPLES: &[Part] = &[
    Part::linear("ps-q1.1k", 0x31fd8726, 0x000000, 0x80000),
    Part::linear("ps-q2.2k", 0x980a9eef, 0x080000, 0x80000),
    Part::linear("ps-q3.3k", 0x0dd44491, 0x100000, 0x80000),
    Part::linear("ps-q4.4k", 0xbed42f03, 0x180000, 0x80000),
];

const SSF2_PROGRAM: &[Part] = &[
    Part::word("ssfe-03b", 0xaf654792, 0x000000, 0x80000),
    Part::word("ssfe.04", 0xb082aa67, 0x080000, 0x80000),
    Part::word("ssfe.05", 0x02b9c137, 0x100000, 0x80000),
    Part::word("ssfe-06b", 0x1c8e44a8, 0x180000, 0x80000),
    Part::word("ssfe.07", 0x2409001d, 0x200000, 0x80000),
];

const SSF2_GRAPHICS: &[Part] = &[
    Part::graphics("ssf.13m", 0xcf94d275, 0x000000, 0x200000),
    Part::graphics("ssf.15m", 0x5eb703af, 0x000002, 0x200000),
    Part::graphics("ssf.17m", 0xffa60e0f, 0x000004, 0x200000),
    Part::graphics("ssf.19m", 0x34e825c5, 0x000006, 0x200000),
    Part::graphics("ssf.14m", 0xb7cc32e7, 0x800000, 0x100000),
    Part::graphics("ssf.16m", 0x8376ad18, 0x800002, 0x100000),
    Part::graphics("ssf.18m", 0xf5b1b336, 0x800004, 0x100000),
    Part::graphics("ssf.20m", 0x459d5c6b, 0x800006, 0x100000),
];

const SSF2_SOUND: &[Part] = &[Part::linear("ssf-01a", 0x71fcdfc9, 0x00000, 0x20000)];

const SSF2_SAMPLES: &[Part] = &[
    Part::linear("ssf.q01", 0xa6f9da5c, 0x000000, 0x080000),
    Part::linear("ssf.q02", 0x8c66ae26, 0x080000, 0x080000),
    Part::linear("ssf.q03", 0x695cc2ca, 0x100000, 0x080000),
    Part::linear("ssf.q04", 0x9d9ebe32, 0x180000, 0x080000),
    Part::linear("ssf.q05", 0x4770e7b7, 0x200000, 0x080000),
    Part::linear("ssf.q06", 0x4e79c951, 0x280000, 0x080000),
    Part::linear("ssf.q07", 0xcdd14313, 0x300000, 0x080000),
    Part::linear("ssf.q08", 0x6f5a088c, 0x380000, 0x080000),
];

impl Part {
    const fn new(
        name: &'static str,
        crc: u32,
        offset: usize,
        length: usize,
        placement: Placement,
    ) -> Self {
        Self {
            name,
            crc,
            offset,
            length,
            placement,
        }
    }
    pub const fn word(name: &'static str, crc: u32, offset: usize, length: usize) -> Self {
        Self::new(name, crc, offset, length, Placement::WordsSwapped)
    }
    pub const fn byte(name: &'static str, crc: u32, offset: usize, length: usize) -> Self {
        Self::new(name, crc, offset, length, Placement::ByteLane)
    }
    pub const fn graphics(name: &'static str, crc: u32, offset: usize, length: usize) -> Self {
        Self::new(name, crc, offset, length, Placement::GraphicsLane)
    }
    pub const fn linear(name: &'static str, crc: u32, offset: usize, length: usize) -> Self {
        Self::new(name, crc, offset, length, Placement::Linear)
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Game {
    Dino,
    Wof,
    Wofa,
    Punisher,
    Ssf2,
}

impl Game {
    pub fn cps_b(self) -> (usize, [usize; 4], [u16; 3]) {
        match self {
            Self::Dino => (0x14a, [0x14c, 0x14e, 0x140, 0x142], [0x16; 3]),
            Self::Wof => (0x162, [0x164, 0x166, 0x168, 0x16a], [0x10, 0x08, 0x04]),
            Self::Wofa | Self::Ssf2 => (0x166, [0x168, 0x16a, 0x16c, 0x16e], [0x02, 0x04, 0x08]),
            Self::Punisher => (0x152, [0x154, 0x156, 0x148, 0x14a], [0x04, 0x02, 0x20]),
        }
    }
}

impl DinoRoms {
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::load_game("dino", path)
    }

    pub fn load_game(name: &str, path: impl AsRef<Path>) -> io::Result<Self> {
        let (game, regions) = match name {
            "dino" => (
                Game::Dino,
                [DINO_PROGRAM, DINO_GRAPHICS, DINO_SOUND, DINO_SAMPLES],
            ),
            "wof" => (
                Game::Wof,
                [WOF_PROGRAM, WOF_GRAPHICS, WOF_SOUND, WOF_SAMPLES],
            ),
            "wofa" => (
                Game::Wofa,
                [WOFA_PROGRAM, WOF_GRAPHICS, WOF_SOUND, WOF_SAMPLES],
            ),
            "punisher" => (
                Game::Punisher,
                [
                    PUNISHER_PROGRAM,
                    PUNISHER_GRAPHICS,
                    PUNISHER_SOUND,
                    PUNISHER_SAMPLES,
                ],
            ),
            "ssf2" => (
                Game::Ssf2,
                [SSF2_PROGRAM, SSF2_GRAPHICS, SSF2_SOUND, SSF2_SAMPLES],
            ),
            _ => return Err(invalid(format!("unsupported game {name:?}"))),
        };
        let [program, graphics, sound, samples] = regions;
        let mut archive = RomArchive::open(name, path)?;
        let program = archive.build(program, 0x400000)?;
        let sound = archive.build(sound, 0x20000)?;
        let (main_opcodes, graphics, sound_data, sound_opcodes, sample_size) =
            if matches!(game, Game::Ssf2) {
                let mut graphics = archive.build(graphics, 0xc00000)?;
                crate::cps2::unshuffle_graphics(&mut graphics);
                (
                    Some(crate::cps2::decrypt_ssf2(&program)),
                    graphics,
                    sound[..0x8000].to_vec(),
                    sound[..0x8000].to_vec(),
                    0x400000,
                )
            } else {
                let (data, opcodes) = decode_fixed_rom(&sound, game);
                (
                    None,
                    archive.build(graphics, 0x400000)?,
                    data,
                    opcodes,
                    0x200000,
                )
            };
        Ok(Self(Arc::new(Regions {
            game,
            program,
            main_opcodes,
            graphics,
            sound,
            sound_data,
            sound_opcodes,
            samples: archive.build(samples, sample_size)?,
        })))
    }

    pub fn region_crc32(&self) -> [u32; 4] {
        let r = &self.0;
        [&r.program, &r.graphics, &r.sound, &r.samples].map(|bytes| crc32fast::hash(bytes))
    }
}

pub(crate) struct RomArchive {
    name: String,
    zip: zip::ZipArchive<File>,
    entries: Vec<(String, u64, u32)>,
}

impl RomArchive {
    pub fn open(name: &str, path: impl AsRef<Path>) -> io::Result<Self> {
        let mut zip = zip::ZipArchive::new(File::open(path)?)?;
        let entries: Vec<_> = (0..zip.len())
            .map(|i| {
                let file = zip.by_index_raw(i)?;
                Ok((file.name().to_owned(), file.size(), file.crc32()))
            })
            .collect::<io::Result<_>>()?;
        Ok(Self {
            name: name.into(),
            zip,
            entries,
        })
    }

    pub fn build(&mut self, parts: &[Part], size: usize) -> io::Result<Vec<u8>> {
        let name = &self.name;
        let mut result = vec![0; size];
        for part in parts {
            let length = part.length;
            let matches =
                |(_, size, crc): &(String, u64, u32)| *size == length as u64 && *crc == part.crc;
            let index = self
                .entries
                .iter()
                .position(|entry| entry.0.eq_ignore_ascii_case(part.name) && matches(entry))
                .or_else(|| self.entries.iter().position(matches))
                .ok_or_else(|| {
                    invalid(format!(
                        "missing {name} ROM {} (CRC {:08x})",
                        part.name, part.crc
                    ))
                })?;
            let mut bytes = Vec::with_capacity(length);
            self.zip
                .by_index(index)?
                .take(length as u64 + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() != length || crc32fast::hash(&bytes) != part.crc {
                return Err(invalid(format!("corrupt {name} ROM {}", part.name)));
            }
            match part.placement {
                Placement::Linear => {
                    result[part.offset..part.offset + length].copy_from_slice(&bytes)
                }
                Placement::WordsSwapped => {
                    swap_words(&mut bytes);
                    result[part.offset..part.offset + length].copy_from_slice(&bytes);
                }
                Placement::ByteLane => {
                    for (dst, value) in result[part.offset..].iter_mut().step_by(2).zip(bytes) {
                        *dst = value;
                    }
                }
                Placement::GraphicsLane => place_graphics_lane(&mut result[part.offset..], &bytes),
            }
        }
        Ok(result)
    }
}

fn place_graphics_lane(dst: &mut [u8], src: &[u8]) {
    for (index, word) in src.chunks_exact(2).enumerate() {
        let offset = index * 8;
        dst[offset..offset + 2].copy_from_slice(word);
    }
}
