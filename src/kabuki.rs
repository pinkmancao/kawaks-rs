// Kabuki operation order and CPS1 QSound keys from MAME's capcom/kabuki.cpp.
// Copyright Nicola Salmoria. See THIRD_PARTY.md for the BSD-3-Clause notice.

use crate::rom::Game;

pub(crate) fn decode_fixed_rom(raw: &[u8], game: Game) -> (Vec<u8>, Vec<u8>) {
    let (first, second, address_key, xor_key): (u32, u32, u16, u8) = match game {
        Game::Dino => (0x76543210, 0x24601357, 0x4343, 0x43),
        Game::Wof | Game::Wofa => (0x01234567, 0x54163072, 0x5151, 0x51),
        Game::Ssf2 => unreachable!("CPS2 Z80 ROM is unencrypted"),
        Game::Punisher => (0x67452103, 0x75316024, 0x2222, 0x22),
    };
    let select_bits = |key, shifts: [u32; 4]| shifts.map(|shift| ((key >> shift) & 7u32) as u8);
    let keys = [
        select_bits(first, [0, 4, 8, 12]),
        select_bits(first, [28, 24, 20, 16]),
        select_bits(second, [12, 8, 4, 0]),
        select_bits(second, [16, 20, 24, 28]),
    ];
    let data = raw
        .iter()
        .take(0x8000)
        .enumerate()
        .map(|(address, &value)| {
            decode(
                value,
                ((address as u16) ^ 0x1fc0)
                    .wrapping_add(address_key)
                    .wrapping_add(1),
                keys,
                xor_key,
            )
        })
        .collect();
    let opcodes = raw
        .iter()
        .take(0x8000)
        .enumerate()
        .map(|(address, &value)| {
            decode(
                value,
                (address as u16).wrapping_add(address_key),
                keys,
                xor_key,
            )
        })
        .collect();
    (data, opcodes)
}

fn decode(value: u8, selector: u16, keys: [[u8; 4]; 4], xor_key: u8) -> u8 {
    let [lo, hi] = selector.to_le_bytes();
    let value = swap_pairs(value, lo, keys[0]).rotate_left(1);
    let value = (swap_pairs(value, lo, keys[1]) ^ xor_key).rotate_left(1);
    let value = swap_pairs(value, hi, keys[2]).rotate_left(1);
    swap_pairs(value, hi, keys[3])
}

fn swap_pairs(mut byte: u8, selector: u8, bits: [u8; 4]) -> u8 {
    for (lane, select_bit) in bits.into_iter().enumerate() {
        let shift = lane * 2;
        let pair = (byte >> shift) & 3;
        if selector & (1 << select_bit) != 0 && matches!(pair, 1 | 2) {
            byte ^= 3 << shift;
        }
    }
    byte
}
