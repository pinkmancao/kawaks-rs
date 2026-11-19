struct Sbox {
    table: [u8; 64],
    inputs: [i8; 6],
    outputs: [u8; 2],
}

include!("cps2_tables.rs");

struct BoxLookup {
    input: [u8; 256],
    output: [u8; 64],
}

impl From<&Sbox> for BoxLookup {
    fn from(sbox: &Sbox) -> Self {
        Self {
            input: std::array::from_fn(|value| {
                sbox.inputs
                    .iter()
                    .enumerate()
                    .filter(|(_, bit)| **bit >= 0)
                    .fold(0, |bits, (i, bit)| bits | (((value >> bit) & 1) as u8) << i)
            }),
            output: sbox.table.map(|value| {
                ((value & 1) << sbox.outputs[0]) | (((value >> 1) & 1) << sbox.outputs[1])
            }),
        }
    }
}

fn round(value: u8, boxes: &[BoxLookup; 4], key: u32) -> u8 {
    boxes.iter().enumerate().fold(0, |result, (i, sbox)| {
        let index = sbox.input[usize::from(value)] ^ ((key >> (6 * i)) & 63) as u8;
        result | sbox.output[usize::from(index)]
    })
}

fn feistel(
    value: u16,
    groups: (&[u8; 8], &[u8; 8]),
    boxes: &[[BoxLookup; 4]; 4],
    key: [u32; 4],
) -> u16 {
    let (a, b) = groups;
    let gather = |positions: &[u8; 8]| {
        positions
            .iter()
            .enumerate()
            .fold(0, |bits, (i, p)| bits | (((value >> p) & 1) as u8) << i)
    };
    let (mut left, mut right) = (gather(b), gather(a));
    left ^= round(right, &boxes[0], key[0]);
    right ^= round(left, &boxes[1], key[1]);
    left ^= round(right, &boxes[2], key[2]);
    right ^= round(left, &boxes[3], key[3]);
    a.iter().zip(b).enumerate().fold(0, |value, (i, (pa, pb))| {
        value | u16::from((left >> i) & 1) << pa | u16::from((right >> i) & 1) << pb
    })
}

fn keys(master: u64, schedule: &[u8; 96], extra: &[(usize, u8, u8)]) -> [u32; 4] {
    let mut result = [0; 4];
    for (i, bit) in schedule.iter().enumerate() {
        result[i / 24] |= (((master >> bit) & 1) as u32) << (i % 24);
    }
    for &(word, from, to) in extra {
        result[word] ^= ((result[word] >> from) & 1) << to;
    }
    result
}

pub(crate) fn decrypt_ssf2(rom: &[u8]) -> Vec<u8> {
    // SSF2's key and 4 MiB encryption range are documented in MAME's
    // historical CPS2 key table. The raw program remains mapped for data.
    const MASTER: u64 = 0xabcdef01_23456789;
    let first = [&FN1_R1_BOXES, &FN1_R2_BOXES, &FN1_R3_BOXES, &FN1_R4_BOXES]
        .map(|boxes| std::array::from_fn(|i| BoxLookup::from(&boxes[i])));
    let second = [&FN2_R1_BOXES, &FN2_R2_BOXES, &FN2_R3_BOXES, &FN2_R4_BOXES]
        .map(|boxes| std::array::from_fn(|i| BoxLookup::from(&boxes[i])));
    let first_key = keys(
        MASTER,
        &KEY1_BITS,
        &[
            (0, 1, 4),
            (0, 2, 5),
            (0, 8, 11),
            (1, 0, 5),
            (1, 8, 11),
            (2, 1, 5),
            (2, 8, 11),
        ],
    );
    let mut opcodes = rom.to_vec();
    for low in 0..0x10000 {
        let seed = feistel(low as u16, (&FN1_A, &FN1_B), &first, first_key);
        let subkey = SEED_BITS
            .iter()
            .enumerate()
            .fold(0, |bits, (i, bit)| bits | u64::from((seed >> bit) & 1) << i);
        let key = keys(
            MASTER ^ subkey,
            &KEY2_BITS,
            &[
                (0, 0, 5),
                (0, 6, 11),
                (1, 0, 5),
                (1, 1, 4),
                (2, 2, 5),
                (2, 3, 4),
                (2, 7, 11),
                (3, 1, 5),
            ],
        );
        for offset in (low * 2..rom.len()).step_by(0x20000) {
            let cipher = u16::from_be_bytes([rom[offset], rom[offset + 1]]);
            let decoded = feistel(cipher, (&FN2_A, &FN2_B), &second, key);
            opcodes[offset..offset + 2].copy_from_slice(&decoded.to_be_bytes());
        }
    }
    opcodes
}

pub(crate) fn unshuffle_graphics(rom: &mut [u8]) {
    fn unshuffle(words: &mut [u8]) {
        if words.len() <= 16 {
            return;
        }
        let quarter = words.len() / 4;
        let (low, high) = words.split_at_mut(quarter * 2);
        unshuffle(low);
        unshuffle(high);
        low[quarter..].swap_with_slice(&mut high[..quarter]);
    }
    for bank in rom.chunks_exact_mut(0x200000) {
        unshuffle(bank);
    }
}
