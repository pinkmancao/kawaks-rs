use crate::bus::Bus;

pub const FRAME_WIDTH: usize = 384;
pub const FRAME_HEIGHT: usize = 224;
const LEFT: usize = 64;
const TOP: usize = 16;
const PIXELS: usize = FRAME_WIDTH * FRAME_HEIGHT;
const PALETTE_BASE: usize = 0x10a;

pub(crate) struct Video {
    objects: [u8; 0x800],
    palette_base: u16,
    cps2: Option<Cps2Video>,
}

struct Cps2Video {
    objects: Vec<u8>,
    output: [u8; 16],
}

impl Video {
    pub fn new(bus: &Bus) -> Self {
        let mut video = Self {
            objects: [0; 0x800],
            palette_base: 0,
            cps2: bus.cps2.as_ref().map(|_| Cps2Video {
                objects: vec![0; 0x2000],
                output: [0; 16],
            }),
        };
        video.latch_objects(bus);
        video.latch_palette_base(bus);
        video
    }

    pub fn latch_objects(&mut self, bus: &Bus) {
        if let (Some(video), Some(io)) = (&mut self.cps2, &bus.cps2) {
            let start = io.object_bank * 0x8000;
            video
                .objects
                .copy_from_slice(&io.objects[start..start + 0x2000]);
            video.output = io.output;
            return;
        }
        let register = word(&bus.registers, 0x100);
        if register >= 0x9000 {
            let offset = usize::from((register & 0xffc0) - 0x9000) << 8;
            if let Some(objects) = bus.graphics.get(offset..offset + self.objects.len()) {
                self.objects.copy_from_slice(objects);
            }
        }
    }

    pub fn latch_palette_base(&mut self, bus: &Bus) {
        self.palette_base = word(&bus.registers, PALETTE_BASE);
    }

    #[cfg(test)]
    pub fn objects(&self) -> &[u8; 0x800] {
        &self.objects
    }

    pub fn hash_state(&self, hash: &mut crc32fast::Hasher) {
        hash.update(&self.objects);
        hash.update(&self.palette_base.to_le_bytes());
        if let Some(cps2) = &self.cps2 {
            hash.update(&cps2.objects);
            hash.update(&cps2.output);
        }
    }
}

pub(crate) fn render(bus: &Bus, video: &Video) -> Vec<u8> {
    let mut colors = vec![0xbff; PIXELS];
    let mut priority = vec![false; PIXELS];
    let mut claimed = vec![false; PIXELS];
    let (control, mask_registers, enables) = bus.roms.0.game.cps_b();
    let layer_control = word(&bus.registers, control);
    let masks = mask_registers.map(|register| word(&bus.registers, register));
    let video_control = word(&bus.registers, 0x122);
    let order = [6, 8, 10, 12].map(|shift| usize::from((layer_control >> shift) & 3));
    if let Some(cps2) = &video.cps2 {
        let mut coverage = vec![0; PIXELS];
        let mut layer_priorities = [0; 3];
        for (index, layer) in order
            .into_iter()
            .filter(|&layer| layer != 0)
            .take(3)
            .enumerate()
        {
            layer_priorities[index] = (word(&cps2.output, 4) >> (4 * layer)) & 15;
            if layer_control & enables[layer - 1] != 0
                && (layer == 1 || video_control & (1 << layer) != 0)
            {
                let mut opaque = vec![false; PIXELS];
                draw_tilemap(
                    bus,
                    layer,
                    video_control,
                    [0xffff; 4],
                    &mut colors,
                    &mut opaque,
                );
                for (bits, visible) in coverage.iter_mut().zip(opaque) {
                    *bits |= u8::from(visible) << index;
                }
            }
        }
        draw_cps2_sprites(
            bus,
            cps2,
            &coverage,
            layer_priorities,
            &mut colors,
            &mut claimed,
        );
    } else {
        for shift in [6, 8, 10, 12] {
            let layer = usize::from((layer_control >> shift) & 3);
            if layer == 0 {
                draw_sprites(bus, &video.objects, &mut colors, &priority, &mut claimed);
                priority.fill(false);
            } else {
                // Only the tilemap immediately below the sprite plane can mask sprites.
                priority.fill(false);
                let enabled = layer_control & enables[layer - 1] != 0
                    && (layer == 1 || video_control & (1 << layer) != 0);
                if enabled {
                    draw_tilemap(bus, layer, video_control, masks, &mut colors, &mut priority);
                }
            }
        }
    }
    let palette = palette(bus, video.palette_base);
    if video_control & 0x8000 != 0 {
        colors.reverse();
    }
    colors
        .into_iter()
        .flat_map(|index| palette[index])
        .collect()
}

fn palette(bus: &Bus, register: u16) -> [[u8; 3]; 0xc00] {
    let mut palette = [[0; 3]; 0xc00];
    if let Some(source) = register.checked_sub(0x9000) {
        let base = usize::from(source) << 8;
        if let Some(data) = bus.graphics.get(base..) {
            for (rgb, bytes) in palette.iter_mut().zip(data.chunks_exact(2)) {
                let value = u16::from_be_bytes([bytes[0], bytes[1]]);
                let brightness = value >> 12;
                let scale = if brightness == 0 { 0 } else { brightness + 2 };
                *rgb = [8, 4, 0].map(|shift| {
                    let pen = value >> shift & 15;
                    if bus.cps2.is_some() {
                        (pen * (brightness + 16) * 16 / 31) as u8
                    } else {
                        (pen * scale) as u8
                    }
                });
            }
        }
    }
    palette
}

fn draw_tilemap(
    bus: &Bus,
    layer: usize,
    video_control: u16,
    masks: [u16; 4],
    colors: &mut [usize],
    priority: &mut [bool],
) {
    let regs = &bus.registers;
    let tile_base = base(bus, 0x100 + 2 * layer, 0x4000);
    let size = 4 << layer;
    let group_size = 256 / size;
    let scroll_x = usize::from(word(regs, 0x10c + (layer - 1) * 4));
    let scroll_y = usize::from(word(regs, 0x10e + (layer - 1) * 4));
    let line_base = base(bus, 0x108, 0x800);
    let line_offset = usize::from(word(regs, 0x120));
    for y in 0..FRAME_HEIGHT {
        let screen_y = y + TOP;
        let world_y = screen_y + scroll_y;
        let row = (world_y / size) & 63;
        let row_index = row % group_size + (row / group_size) * group_size * 64;
        let row_scroll = if layer == 2 && video_control & 1 != 0 {
            usize::from(vram_word(
                bus,
                line_base + ((screen_y + line_offset) & 0x3ff) * 2,
            ))
        } else {
            0
        };
        for x in 0..FRAME_WIDTH {
            let world_x = x + LEFT + scroll_x + row_scroll;
            let col = (world_x / size) & 63;
            let index = row_index + col * group_size;
            let address = tile_base + index * 4;
            let mut code = usize::from(vram_word(bus, address));
            let attr = vram_word(bus, address + 2);
            if layer == 3 {
                code &= 0x3fff;
            }
            if bus.cps2.is_some() {
                code += 0x800000 / (size * if size == 32 { 16 } else { 8 });
            }
            let tile_x = flipped(world_x % size, size, attr & 0x20 != 0);
            let tile_y = flipped(world_y % size, size, attr & 0x40 != 0);
            let half = if layer == 1 { (index >> 5) & 1 } else { 0 };
            let pen = tile_pen(&bus.roms.0.graphics, size, code, tile_x + 8 * half, tile_y);
            if pen != 15 {
                let dst = y * FRAME_WIDTH + x;
                colors[dst] = layer * 0x200 + usize::from(attr & 0x1f) * 16 + pen;
                priority[dst] = masks[usize::from((attr >> 7) & 3)] & (1 << pen) != 0;
            }
        }
    }
}

fn draw_cps2_sprites(
    bus: &Bus,
    video: &Cps2Video,
    coverage: &[u8],
    priorities: [u16; 3],
    colors: &mut [usize],
    claimed: &mut [bool],
) {
    let sprites = video.objects.chunks_exact(8);
    let count = sprites
        .clone()
        .take_while(|entry| word(entry, 2) < 0x8000 && word(entry, 6) < 0xff00)
        .count();
    let mut masks = [0u8; 8];
    let mut mask0 = 0xaa;
    let mut mask1 = 0xcc;
    if priorities[0] > priorities[1] {
        mask0 &= !0x88;
    }
    if priorities[0] > priorities[2] {
        mask0 &= !0xa0;
    }
    if priorities[1] > priorities[2] {
        mask1 &= !0xc0;
    }
    masks[0] = 0xff;
    for (level, mask) in masks.iter_mut().enumerate().skip(1) {
        let above = priorities.map(|pri| level as u16 <= pri);
        *mask = if above == [true; 3] {
            0xfe
        } else {
            (if above[0] { mask0 } else { 0 })
                | (if above[1] { mask1 } else { 0 })
                | (if above[2] { 0xf0 } else { 0 })
        };
    }
    let shift_x = usize::from(word(&video.output, 8));
    let shift_y = usize::from(word(&video.output, 10));
    for sprite in video.objects[..count * 8].chunks_exact(8).rev() {
        let x = usize::from(word(sprite, 0));
        let y = usize::from(word(sprite, 2));
        let code = usize::from(word(sprite, 4)) | ((y & 0x6000) << 3);
        let attr = word(sprite, 6);
        let mask = masks[(x >> 13) & 7];
        let (width, height) = (
            usize::from((attr >> 8) & 15) + 1,
            usize::from(attr >> 12) + 1,
        );
        let (flip_x, flip_y) = (attr & 0x20 != 0, attr & 0x40 != 0);
        let (x, y) = if attr & 0x80 != 0 {
            (x, y)
        } else {
            (x.wrapping_sub(shift_x), y.wrapping_sub(shift_y))
        };
        for row in 0..height {
            for col in 0..width {
                let sx = x.wrapping_add(16 * col) & 0x3ff;
                let sy = y.wrapping_add(16 * row) & 0x3ff;
                let tile = (code & !15) | ((code + flipped(col, width, flip_x)) & 15);
                let tile = tile + 16 * flipped(row, height, flip_y);
                if sx >= FRAME_WIDTH || sy >= FRAME_HEIGHT {
                    continue;
                }
                for py in 0..16.min(FRAME_HEIGHT - sy) {
                    for px in 0..16.min(FRAME_WIDTH - sx) {
                        let dst = (sy + py) * FRAME_WIDTH + sx + px;
                        if claimed[dst] {
                            continue;
                        }
                        let pen = tile_pen(
                            &bus.roms.0.graphics,
                            16,
                            tile,
                            flipped(px, 16, flip_x),
                            flipped(py, 16, flip_y),
                        );
                        if pen != 15 {
                            claimed[dst] = true;
                            if mask & (1 << coverage[dst]) == 0 {
                                colors[dst] = usize::from(attr & 31) * 16 + pen;
                            }
                        }
                    }
                }
            }
        }
    }
}

fn draw_sprites(
    bus: &Bus,
    objects: &[u8; 0x800],
    colors: &mut [usize],
    priority: &[bool],
    claimed: &mut [bool],
) {
    for offset in (0..objects.len()).step_by(8) {
        let attr = word(objects, offset + 6);
        if attr == 0xff00 {
            break;
        }
        let x = usize::from(word(objects, offset));
        let y = usize::from(word(objects, offset + 2));
        let code = usize::from(word(objects, offset + 4));
        let width = usize::from((attr >> 8) & 15) + 1;
        let height = usize::from(attr >> 12) + 1;
        let flip_x = attr & 0x20 != 0;
        let flip_y = attr & 0x40 != 0;
        let palette_offset = usize::from(attr & 0x1f) * 16;
        for cell_y in 0..height {
            for cell_x in 0..width {
                let sx = (x + 16 * cell_x) & 0x1ff;
                let sy = (y + 16 * cell_y) & 0x1ff;
                let tile = (code & !15)
                    + ((code + flipped(cell_x, width, flip_x)) & 15)
                    + 16 * flipped(cell_y, height, flip_y);
                for py in 0..16 {
                    let screen_y = sy + py;
                    if !(TOP..TOP + FRAME_HEIGHT).contains(&screen_y) {
                        continue;
                    }
                    for px in 0..16 {
                        let screen_x = sx + px;
                        if !(LEFT..LEFT + FRAME_WIDTH).contains(&screen_x) {
                            continue;
                        }
                        let dst = (screen_y - TOP) * FRAME_WIDTH + screen_x - LEFT;
                        if claimed[dst] {
                            continue;
                        }
                        let pen = tile_pen(
                            &bus.roms.0.graphics,
                            16,
                            tile,
                            flipped(px, 16, flip_x),
                            flipped(py, 16, flip_y),
                        );
                        if pen != 15 {
                            claimed[dst] = true;
                            if !priority[dst] {
                                colors[dst] = palette_offset + pen;
                            }
                        }
                    }
                }
            }
        }
    }
}

fn flipped(index: usize, size: usize, flip: bool) -> usize {
    if flip {
        size - 1 - index
    } else {
        index
    }
}

fn tile_pen(rom: &[u8], size: usize, code: usize, x: usize, y: usize) -> usize {
    let stride = if size == 32 { 16 } else { 8 };
    let offset = (code * size + y) * stride + (x / 8) * 4;
    let Some(planes) = rom.get(offset..offset + 4) else {
        return 15;
    };
    let shift = 7 - (x & 7);
    planes.iter().enumerate().fold(0, |pen, (plane, byte)| {
        pen | (usize::from((byte >> shift) & 1) << plane)
    })
}

fn base(bus: &Bus, register: usize, alignment: usize) -> usize {
    (usize::from(word(&bus.registers, register)) << 8) & (0x40000 - alignment)
}

fn vram_word(bus: &Bus, address: usize) -> u16 {
    word(&bus.graphics, address & 0x3ffff)
}

fn word(memory: &[u8], address: usize) -> u16 {
    memory
        .get(address..address + 2)
        .map_or(0xffff, |bytes| u16::from_be_bytes([bytes[0], bytes[1]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_bitplanes_and_scroll1_halves() {
        let mut rom = [0; 64];
        rom[..8].copy_from_slice(&[0x80, 0x01, 0, 0x81, 0, 0, 0x80, 0]);
        assert_eq!(tile_pen(&rom, 8, 0, 0, 0), 9);
        assert_eq!(tile_pen(&rom, 8, 0, 7, 0), 10);
        assert_eq!(tile_pen(&rom, 8, 0, 8, 0), 4);
        assert_eq!(tile_pen(&rom, 8, 1, 0, 0), 15);
    }
}
