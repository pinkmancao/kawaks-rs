use std::{fs, path::PathBuf};

use super::*;

const WINDOW_STRIDE: usize = 896;

#[test]
#[ignore = "requires WinKawaks 1.65 dumps from the reference capture"]
fn compare_win165_frames_and_pcm() {
    let capture = std::env::var_os("KAWAKS_REFERENCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("outputs/win165"));
    let fixture = std::env::var_os("KAWAKS_FIXTURE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".."));
    let roms = DinoRoms::load(fixture.join("roms/cps1/dino.zip")).unwrap();
    for slot in 0..4 {
        let dir = capture.join(format!("slot{slot}"));
        let kwk =
            Kwk4::read(&fs::read(fixture.join(format!("saves/dino.ks{slot}"))).unwrap()).unwrap();
        assert_eq!(fs::read(dir.join("pre.voices")).unwrap(), kwk.channels);
        let mut replay = qsound::QSound::default();
        replay.restore(&kwk.channels);
        let mut machine = Dino::from_kwk4(roms.clone(), kwk).unwrap();
        let mut aligned = Dino::from_kwk4(
            roms.clone(),
            Kwk4::read(&fs::read(fixture.join(format!("saves/dino.ks{slot}"))).unwrap()).unwrap(),
        )
        .unwrap();
        align_legacy_sound_session(&mut aligned, &dir);
        let mut reference_commands = Vec::new();
        let log = fs::read_to_string(dir.join("capture.log")).unwrap();
        let mut commands = Vec::new();
        for line in log.lines() {
            if line.starts_with("MIX_FRAME ") {
                reference_commands.push(std::mem::take(&mut commands));
                continue;
            }
            let mut fields = line.split_whitespace();
            while let Some(token) = fields.next() {
                if token == "QS" {
                    commands.push((
                        u8::from_str_radix(fields.next().unwrap(), 16).unwrap(),
                        u16::from_str_radix(fields.next().unwrap(), 16).unwrap(),
                    ));
                }
            }
        }
        let mut totals = [0usize; 8];
        let mut command_mismatches = 0;
        for frame in 1.. {
            let read = |ext| fs::read(dir.join(format!("f{frame:03}.{ext}"))).unwrap();
            if !dir.join(format!("f{frame:03}.rgb16")).exists() {
                assert!(frame > 1, "no reference frames for slot {slot}");
                break;
            }
            machine.step_frame(FrameInput::default()).unwrap();
            aligned.main.mem.qsound_trace.clear();
            aligned.step_frame(FrameInput::default()).unwrap();
            let expected_left = read("left");
            let expected_right = read("right");
            assert_eq!(expected_left.len(), AUDIO_SAMPLES_PER_FRAME * 2);
            assert_eq!(expected_right.len(), expected_left.len());
            let expected_audio: Vec<_> = expected_left
                .chunks_exact(2)
                .zip(expected_right.chunks_exact(2))
                .map(|(l, r)| {
                    [
                        i16::from_le_bytes(l.try_into().unwrap()),
                        i16::from_le_bytes(r.try_into().unwrap()),
                    ]
                })
                .collect();
            let audio_diff = audio_difference(machine.frame_audio(), &expected_audio);
            let phase_aligned_diff = audio_difference(aligned.frame_audio(), &expected_audio);

            let mix_snapshot: [u8; 0x300] = read("mix-in").try_into().unwrap();
            let mut reference_mixer = qsound::QSound::default();
            reference_mixer.restore(&mix_snapshot);
            reference_mixer.mix_frame(&roms.0.samples);
            let isolated_audio_diff = audio_difference(&reference_mixer.pcm, &expected_audio);

            let expected_commands = &reference_commands[frame - 1];
            for &(register, data) in expected_commands {
                replay.write(register, data);
            }
            replay.mix_frame(&roms.0.samples);
            let replay_diff = audio_difference(&replay.pcm, &expected_audio);

            let raw_frame = read("rgb16");
            let pixel_diff = pixel_difference(&machine.render_rgb(), &raw_frame);
            let mut vram = read("vram");
            let mut regs = read("regs");
            let mut objects = read("objects");
            swap_words(&mut vram);
            swap_words(&mut regs);
            swap_words(&mut objects);
            let vram_diff = difference(&machine.main.mem.graphics, &vram);
            let obj_diff = difference(machine.video.objects(), &objects);
            let regs_diff = difference(
                &machine.main.mem.registers[0x100..0x150],
                &regs[0x100..0x150],
            );

            let [pixel_count, audio_count, mixer_count] =
                [pixel_diff.0, audio_diff.0, isolated_audio_diff.0];
            for (total, diff) in totals.iter_mut().zip([
                pixel_count,
                audio_count,
                mixer_count,
                vram_diff,
                obj_diff,
                regs_diff,
                phase_aligned_diff.0,
                replay_diff.0,
            ]) {
                *total += diff;
            }
            let phase = u32::from_le_bytes(read("phase").try_into().unwrap());
            let legacy_pc = le16(&read("z80"), 0x50);
            let current_phase = aligned.irq_phase;
            let current_pc = aligned.sound.get_pc();
            let expected = expected_commands;
            let actual = &aligned.main.mem.qsound_trace;
            if expected != actual {
                command_mismatches += 1;
                let first = expected.iter().zip(actual).find(|(a, b)| a != b);
                println!(
                    "slot={slot} frame={frame:03} QS native={} Rust={} first={first:04x?}",
                    expected.len(),
                    actual.len()
                );
            }
            println!("slot={slot} frame={frame:03} pixel={pixel_diff:?} pcm={audio_diff:?} aligned={phase_aligned_diff:?} mixer={isolated_audio_diff:?} replay={replay_diff:?} vram={vram_diff} obj={obj_diff} regs={regs_diff} phase={phase}/{current_phase} z80={legacy_pc:04x}/{current_pc:04x}");
        }
        assert_eq!(
            command_mismatches, 0,
            "slot {slot} QSound commands differ after session alignment"
        );
        assert_eq!(
            totals[6], 0,
            "slot {slot} PCM differs after session alignment"
        );
        assert_eq!(totals[0], 0, "slot {slot} rendered pixels differ");
        assert_eq!(
            totals[2], 0,
            "slot {slot} PCM mixer differs from its reference snapshot"
        );
        assert_eq!(
            totals[7], 0,
            "slot {slot} PCM differs when replaying the original command stream"
        );
        println!("slot={slot} totals [pixel, pcm-value, isolated-mixer, vram, objects, video-registers, aligned-pcm, replay-pcm] {totals:?}");
    }
}

fn align_legacy_sound_session(machine: &mut Dino, dir: &std::path::Path) {
    let read = |file| u32::from_le_bytes(fs::read(dir.join(file)).unwrap().try_into().unwrap());
    machine.irq_phase = u64::from(read("pre.phase"));
    machine.main.mem.qsound_data = read("pre.latch") as u16;
    if dir.join("pre.irq-cycles").exists() {
        machine.sound_irq_cycles = u64::from(read("pre.irq-cycles"));
    }
}

fn difference(a: &[u8], b: &[u8]) -> usize {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b).filter(|(a, b)| a != b).count()
}

fn pixel_difference(rgb: &[u8], reference: &[u8]) -> (usize, Option<(usize, usize, u16, u16)>) {
    assert_eq!(reference.len(), WINDOW_STRIDE * FRAME_HEIGHT);
    let mut mismatch = 0;
    let mut first = None;
    for (index, pixel) in rgb.chunks_exact(3).enumerate() {
        let (y, x) = (index / FRAME_WIDTH, index % FRAME_WIDTH);
        let offset = y * WINDOW_STRIDE + x * 2;
        let legacy = u16::from_le_bytes(reference[offset..offset + 2].try_into().unwrap());
        let [r, g, b] = [pixel[0], pixel[1], pixel[2]].map(u16::from);
        let current = ((r >> 3) << 11) | ((g >> 3) << 6) | (b >> 3);
        if legacy != current {
            mismatch += 1;
            first.get_or_insert((x, y, legacy, current));
        }
    }
    (mismatch, first)
}

fn audio_difference(
    actual: &[[i16; 2]],
    reference: &[[i16; 2]],
) -> (usize, Option<(usize, [i16; 2], [i16; 2])>) {
    assert_eq!(actual.len(), reference.len());
    let mut mismatch = 0;
    let mut first = None;
    for (index, (&current, &legacy)) in actual.iter().zip(reference).enumerate() {
        mismatch += usize::from(current[0] != legacy[0]) + usize::from(current[1] != legacy[1]);
        if current != legacy {
            first.get_or_insert((index, legacy, current));
        }
    }
    (mismatch, first)
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct SoundSlice {
    pub pc: u16,
    pub cycles: u64,
    pub after: u16,
    pub phase: u64,
    pub pending: bool,
    pub irqs: Vec<(bool, u16, u64)>,
}

#[test]
#[ignore = "requires CDB sound slice trace"]
fn compare_win165_sound_slices() {
    let capture = std::env::var_os("KAWAKS_REFERENCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("outputs/win165-slices"));
    let mut examined = 0;
    let fixture = std::env::var_os("KAWAKS_FIXTURE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".."));
    let roms = DinoRoms::load(fixture.join("roms/cps1/dino.zip")).unwrap();
    for slot in 0..4 {
        let dir = capture.join(format!("slot{slot}"));
        if !dir.is_dir() {
            continue;
        }
        let state =
            Kwk4::read(&fs::read(fixture.join(format!("saves/dino.ks{slot}"))).unwrap()).unwrap();
        let mut machine = Dino::from_kwk4(roms.clone(), state).unwrap();
        align_legacy_sound_session(&mut machine, &dir);
        let log = fs::read_to_string(dir.join("capture.log")).unwrap();
        let mut input = log
            .lines()
            .filter(|line| !line.contains("> .echo MIX_FRAME "))
            .flat_map(str::split_whitespace);
        let hex = |s: &str| u64::from_str_radix(s, 16).unwrap();
        let mut irqs = Vec::new();
        let mut slices = Vec::new();
        let mut differences = 0;
        let mut compared = 0;
        let mut accepted_irqs = 0;
        while let Some(token) = input.next() {
            match token {
                "IRQ" => {
                    let mode = input.next().unwrap();
                    if mode == "I" || mode == "D" {
                        let pc = hex(input.next().unwrap()) as u16;
                        let cycles = hex(input.next().unwrap());
                        irqs.push((mode == "I", pc, cycles));
                    }
                }
                "SL" => {
                    let Ok(index) = u8::from_str_radix(input.next().unwrap(), 16) else {
                        continue;
                    };
                    assert_eq!(usize::from(index), SOUND_SLICES as usize - slices.len());
                    let pc = hex(input.next().unwrap()) as u16;
                    let cycles = hex(input.next().unwrap());
                    let _phase_with_cycles = input.next().unwrap();
                    slices.push(SoundSlice {
                        pc,
                        cycles,
                        after: 0,
                        phase: 0,
                        pending: false,
                        irqs: Vec::new(),
                    });
                }
                "END" => {
                    let Some(last) = slices.last_mut() else {
                        continue;
                    };
                    last.after = hex(input.next().unwrap()) as u16;
                    last.phase = hex(input.next().unwrap());
                    last.pending = hex(input.next().unwrap()) != 0;
                    for (immediate, _, cycles) in &mut irqs {
                        if *immediate {
                            *cycles = last.cycles;
                        }
                    }
                    last.irqs = std::mem::take(&mut irqs);
                }
                "MIX_FRAME" => {
                    let Ok(frame) = input.next().unwrap().parse::<usize>() else {
                        continue;
                    };
                    if slices.is_empty() {
                        continue;
                    }
                    assert_eq!(slices.len(), SOUND_SLICES as usize);
                    compared += slices.len();
                    accepted_irqs += slices.iter().map(|slice| slice.irqs.len()).sum::<usize>();
                    machine.step_frame(FrameInput::default()).unwrap();
                    for (i, (native, rust)) in slices.iter().zip(&machine.sound_slices).enumerate()
                    {
                        if native != rust {
                            if differences < 8 {
                                println!("slot={slot} frame={frame} slice={} native={native:?} rust={rust:?}",i+1);
                            }
                            differences += 1;
                        }
                    }
                    slices.clear();
                }
                _ => {}
            }
        }
        assert!(compared > 0, "slot {slot} has no sound slice trace");
        println!("slot={slot} compared {compared} slices, {accepted_irqs} IRQ accepts, {differences} differences");
        assert_eq!(
            differences, 0,
            "slot {slot} differs in per-slice Z80 timing"
        );
        examined += compared;
    }
    assert!(
        examined > 0,
        "no sound slice traces in {}",
        capture.display()
    );
}

#[test]
#[ignore = "requires local ROMs, KWK4 saves and WinKawaks frame captures"]
fn compare_platform_win165_frames() {
    let project = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture = std::env::var_os("KAWAKS_FIXTURE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| project.join(".."));
    for (game, slot, directory, width) in [
        ("ssf2", 3, "ssf2-coin-reference", 384),
        ("kof97", 0, "kof97-animation-reference", 304),
    ] {
        let capture = project.join("outputs").join(directory);
        let saves = capture.join("saves");
        let state = fs::read(saves.join(format!("{game}.ks{slot}"))).unwrap();
        let snapshots = capture.join(format!("slot{slot}"));
        let mut machine: Box<dyn ArcadeMachine> = if game == "ssf2" {
            let roms = DinoRoms::load_game(game, fixture.join("roms/cps2/ssf2.zip")).unwrap();
            let mut cps2 = Dino::from_kwk4(roms, Kwk4::read_cps2(&state).unwrap()).unwrap();
            align_legacy_sound_session(&mut cps2, &snapshots);
            Box::new(cps2)
        } else {
            let roms = NeoRoms::load_kof97(
                fixture.join("roms/neogeo/kof97.zip"),
                fixture.join("roms/neogeo/neogeo.zip"),
            )
            .unwrap();
            Box::new(NeoGeo::from_kwk4(roms, NeoKwk4::read(&state).unwrap()).unwrap())
        };
        let (mut pixels, mut samples, mut active_pixels, mut active_samples, mut rust_samples) =
            (0, 0, 0, 0, 0);
        for frame in 1..=24 {
            machine.step_frame(FrameInput::default()).unwrap();
            let read = |ext| fs::read(snapshots.join(format!("f{frame:03}.{ext}"))).unwrap();
            let legacy = read("rgb16");
            let rgb = machine.render_rgb();
            assert_eq!(legacy.len(), WINDOW_STRIDE * FRAME_HEIGHT);
            assert_eq!(rgb.len(), FRAME_WIDTH * FRAME_HEIGHT * 3);
            let mut different_pixels = 0;
            let (mut min_x, mut min_y, mut max_x, mut max_y) = (width, FRAME_HEIGHT, 0, 0);
            for y in 0..FRAME_HEIGHT {
                for x in 0..width {
                    let offset = y * WINDOW_STRIDE + x * 2;
                    let expected =
                        u16::from_le_bytes(legacy[offset..offset + 2].try_into().unwrap());
                    let offset = (y * FRAME_WIDTH + x + (FRAME_WIDTH - width) / 2) * 3;
                    let [r, g, b] = [rgb[offset], rgb[offset + 1], rgb[offset + 2]].map(u16::from);
                    let actual = ((r >> 3) << 11) | ((g >> 3) << 6) | (b >> 3);
                    active_pixels += usize::from(expected != 0);
                    if actual != expected {
                        different_pixels += 1;
                        min_x = min_x.min(x);
                        min_y = min_y.min(y);
                        max_x = max_x.max(x);
                        max_y = max_y.max(y);
                    }
                }
            }
            let (left, right) = (read("left"), read("right"));
            assert_eq!(left.len(), right.len());
            assert_eq!(left.len(), 2 * AUDIO_SAMPLES_PER_FRAME);
            let pcm: Vec<[i16; 2]> = left
                .chunks_exact(2)
                .zip(right.chunks_exact(2))
                .map(|(l, r)| {
                    [
                        i16::from_le_bytes(l.try_into().unwrap()),
                        i16::from_le_bytes(r.try_into().unwrap()),
                    ]
                })
                .collect();
            active_samples += pcm.iter().flatten().filter(|&&value| value != 0).count();
            rust_samples += machine
                .frame_audio()
                .iter()
                .flatten()
                .filter(|&&value| value != 0)
                .count();
            let (different_samples, _) = audio_difference(machine.frame_audio(), &pcm);
            println!("{game} frame={frame:02} pixels_different={different_pixels} pcm_channels_different={different_samples} bounds={min_x},{min_y}-{max_x},{max_y}");
            pixels += different_pixels;
            samples += different_samples;
        }
        assert!(
            active_pixels > 0 && active_samples > 0,
            "{game} reference must contain picture and sound"
        );
        assert_eq!(pixels, 0, "{game} picture differs from WinKawaks");
        assert_eq!(samples, 0, "{game} PCM differs from WinKawaks");
        println!("{game} totals pixels={pixels}/{} pcm={samples}/{} reference_nonzero_pixels={active_pixels} reference_nonzero_pcm={active_samples} rust_nonzero_pcm={rust_samples}",
            24 * width * FRAME_HEIGHT, 24 * 2 * AUDIO_SAMPLES_PER_FRAME);
    }
}
