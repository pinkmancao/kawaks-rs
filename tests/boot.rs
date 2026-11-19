use kawaks_rs::{
    Dino, DinoRoms, FrameInput, Kwk4, AUDIO_SAMPLES_PER_FRAME, FRAME_HEIGHT, FRAME_WIDTH,
};
use std::collections::HashSet;

#[test]
#[ignore = "requires local dino.zip and dino.ks0 through dino.ks3"]
fn local_dino_executes_and_imports_all_four_slots() {
    let root = std::env::var_os("KAWAKS_FIXTURE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".."));
    let roms = DinoRoms::load(root.join("roms/cps1/dino.zip")).unwrap();
    assert_eq!(
        roms.region_crc32(),
        [0xec644a1e, 0x80d21717, 0x605fdb0b, 0xa9c6f1de]
    );

    let mut machine = Dino::new(roms.clone());
    let initial = machine.report();
    assert_eq!(initial.main_pc, 0x41a);
    for _ in 0..120 {
        machine.step_frame(FrameInput::default()).unwrap();
    }
    let booted = machine.report();
    assert_eq!(booted.frame, 120);
    assert!(booted.main_cycles >= 167_680 * 120);
    assert!(booted.sound_cycles >= 133_120 * 120);
    assert_ne!(booted.graphics_ram_crc32, initial.graphics_ram_crc32);
    assert!(
        booted.qsound_commands > 0,
        "real sound firmware did not reach the QSound port"
    );
    assert!(booted.main_pc >= 0x400);

    let mut pictures = HashSet::new();
    let mut soundtracks = HashSet::new();
    for (slot, (main_pc, sound_pc)) in
        [(0x602, 0x29fc), (0x5f0, 0xc1), (0x602, 0xc4), (0x606, 0xc1)]
            .into_iter()
            .enumerate()
    {
        let path = root.join(format!("saves/dino.ks{slot}"));
        let state = Kwk4::read(&std::fs::read(path).unwrap()).unwrap();
        let mut machine = Dino::from_kwk4(roms.clone(), state).unwrap();
        assert_eq!(machine.report().main_pc, main_pc);
        assert_eq!(machine.report().sound_pc, sound_pc);
        let restored = machine.render_rgb();
        assert_eq!(restored.len(), FRAME_WIDTH * FRAME_HEIGHT * 3);
        assert!(
            restored.chunks_exact(3).collect::<HashSet<_>>().len() > 64,
            "slot {slot} lacks visible tile and sprite colors"
        );
        assert!(pictures.insert(crc32fast::hash(&restored)));

        let mut sample_count = 0;
        let mut stereo_difference = false;
        let mut pcm_crc = crc32fast::Hasher::new();
        for step in 0..60 {
            machine.step_frame(FrameInput::default()).unwrap();
            let audio = machine.frame_audio();
            assert_eq!(audio.len(), AUDIO_SAMPLES_PER_FRAME);
            if step == 0 {
                assert!(
                    audio[..32].iter().any(|&sample| sample != [0, 0]),
                    "slot {slot} lost the saved PCM voices"
                );
            }
            sample_count += audio.len();
            for sample in audio {
                stereo_difference |= sample[0] != sample[1];
                for channel in sample {
                    pcm_crc.update(&channel.to_le_bytes());
                }
            }
        }
        assert_eq!(sample_count, 60 * AUDIO_SAMPLES_PER_FRAME);
        assert!(stereo_difference, "slot {slot} has no stereo separation");
        assert!(soundtracks.insert(pcm_crc.finalize()));
        assert_ne!(
            machine.render_rgb(),
            restored,
            "slot {slot} did not update the rendered picture"
        );
        let frame = machine.report();
        assert_eq!(frame.frame, 60);
        assert!(
            frame.main_pc >= 0x400,
            "slot {slot} entered an early exception vector"
        );
    }
}

#[test]
#[ignore = "requires local WOF/Punisher ROMs and two WOFA slots"]
fn more_cps1_qsound_games_boot_and_wofa_restores_with_live_input() {
    let root = std::env::var_os("KAWAKS_FIXTURE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".."));
    let wof = root.join("roms/cps1/wof.zip");
    for (name, path, expected) in [
        (
            "wof",
            wof.clone(),
            [0x393b6a7f, 0x665f3bf4, 0xc9183a0d, 0xffcb8e54],
        ),
        (
            "punisher",
            root.join("roms/cps1/punisher.zip"),
            [0x551a46a1, 0xe9edae13, 0x49ff4446, 0x93ef60a7],
        ),
    ] {
        let roms = DinoRoms::load_game(name, path).unwrap();
        assert_eq!(roms.region_crc32(), expected);
        let mut game = Dino::new(roms);
        for _ in 0..900 {
            game.step_frame(FrameInput::default()).unwrap();
        }
        assert!(game.report().qsound_commands > 0);
        assert!(
            game.render_rgb()
                .chunks_exact(3)
                .collect::<HashSet<_>>()
                .len()
                > 20
        );
    }
    let roms = DinoRoms::load_game("wofa", wof).unwrap();
    assert_eq!(
        roms.region_crc32(),
        [0xb87a4725, 0x665f3bf4, 0xc9183a0d, 0xffcb8e54]
    );
    for slot in 0..2 {
        let bytes = std::fs::read(root.join(format!("saves/wofa.ks{slot}"))).unwrap();
        let boot = || Dino::from_kwk4(roms.clone(), Kwk4::read(&bytes).unwrap()).unwrap();
        let (mut idle, mut directed) = (boot(), boot());
        let before = idle.render_rgb();
        let mut heard_samples = false;
        for frame in 0..60 {
            idle.step_frame(FrameInput::default()).unwrap();
            directed
                .step_frame(FrameInput {
                    players: [if frame < 25 { 0x0018 } else { 0 }, 0, 0, 0],
                })
                .unwrap();
            heard_samples |= directed
                .frame_audio()
                .iter()
                .any(|&sample| sample != [0, 0]);
        }
        assert!(heard_samples, "slot {slot} produced only silence");
        assert_ne!(idle.render_rgb(), before);
        assert_ne!(
            directed.render_rgb(),
            idle.render_rgb(),
            "slot {slot} did not respond to P1 input"
        );
    }
}

#[test]
#[ignore = "requires local SSF2, KOF97 and Neo Geo BIOS ROMs"]
fn cps2_and_neo_geo_cold_boot_accept_credit_and_start() {
    use kawaks_rs::{write_png, ArcadeMachine, NeoGeo, NeoRoms};
    let root = std::env::var_os("KAWAKS_FIXTURE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".."));
    let cps2 = Dino::new(DinoRoms::load_game("ssf2", root.join("roms/cps2/ssf2.zip")).unwrap());
    let neo = NeoGeo::new(
        NeoRoms::load_kof97(
            root.join("roms/neogeo/kof97.zip"),
            root.join("roms/neogeo/neogeo.zip"),
        )
        .unwrap(),
    );
    for (game, mut machine, battle_frame, battle_crc) in [
        (
            "ssf2",
            Box::new(cps2) as Box<dyn ArcadeMachine>,
            2861,
            0xa157acbd,
        ),
        (
            "kof97",
            Box::new(neo) as Box<dyn ArcadeMachine>,
            4061,
            0xe7d7648b,
        ),
    ] {
        let mut peak = 0;
        for frame in 0..battle_frame {
            let mut input = FrameInput::default();
            if frame >= 1750 {
                input.players[0] = match frame % 240 {
                    0..=7 => 0x2000,
                    24..=31 => 0x1000,
                    90..=96 | 120..=126 => 0x10,
                    _ => 0,
                };
            }
            machine.step_frame(input).unwrap();
            peak = peak.max(
                machine
                    .frame_audio()
                    .iter()
                    .flatten()
                    .map(|x| x.unsigned_abs())
                    .max()
                    .unwrap(),
            );
        }
        let pixels = machine.render_rgb();
        assert_eq!(
            crc32fast::hash(&pixels),
            battle_crc,
            "{game} live battle frame"
        );
        assert!(peak > 0, "{game} cold boot should produce audio");
        if let Some(dir) = std::env::var_os("KAWAKS_BOOT_IMAGES") {
            let path = std::path::PathBuf::from(dir).join(format!("{game}-cold-battle.png"));
            write_png(std::fs::File::create(path).unwrap(), &pixels).unwrap();
        }
        println!(
            "{game} cold boot frame={} pc={:06x} pcm_peak={peak}",
            machine.report().frame,
            machine.report().main_pc
        );
    }
}

#[test]
#[ignore = "requires the local merged WOF ROM archive"]
fn wof_regions_start_each_native_stage_after_character_select() {
    let root = std::env::var_os("KAWAKS_FIXTURE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".."));
    for game in ["wof", "wofa"] {
        let archive = root.join("roms/cps1/wof.zip");
        let roms = DinoRoms::load_game(game, archive).unwrap();
        let count = Dino::starting_stages(game).len();
        assert!(Dino::new_at_stage(roms.clone(), count as u8).is_err());
        let mut scenes = HashSet::new();
        for stage in 0..count as u8 {
            let mut machine = Dino::new_at_stage(roms.clone(), stage).unwrap();
            for frame in 0..1150 {
                let buttons = if frame >= 400 {
                    match frame % 120 {
                        0..=6 => 0x2000,
                        20..=26 => 0x1000,
                        48..=54 | 80..=86 => 0x10,
                        _ => 0,
                    }
                } else {
                    0
                };
                machine
                    .step_frame(FrameInput {
                        players: [buttons, 0, 0, 0],
                    })
                    .unwrap();
                if frame == 1060 {
                    assert_eq!(
                        machine.read_main_byte(0xff7f88),
                        0,
                        "{game} changed before the match"
                    );
                }
                if frame == 1090 {
                    assert_eq!(
                        machine.read_main_byte(0xff7f88),
                        stage,
                        "{game} initial stage"
                    );
                }
            }
            assert!(machine.report().qsound_commands > 0);
            let pixels = machine.render_rgb();
            assert!(pixels.chunks_exact(3).collect::<HashSet<_>>().len() > 20);
            scenes.insert(crc32fast::hash(&pixels));
        }
        assert_eq!(scenes.len(), count, "{game} distinct stage scenes");
    }
}
