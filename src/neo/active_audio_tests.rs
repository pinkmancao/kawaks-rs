use super::*;
use std::{fs, path::PathBuf};

type Stereo = [i32; 2];
type Triple = [i32; 3];

fn components(log: &str) -> (Vec<Stereo>, Vec<Triple>) {
    let mut words = log
        .lines()
        .filter(|line| !line.contains(".printf"))
        .flat_map(str::split_whitespace);
    let mut fm = Vec::new();
    let mut ay = Vec::new();
    let mut delta_t = None;
    while let Some(tag) = words.next() {
        if !matches!(tag, "D" | "DT" | "FM" | "AY") {
            continue;
        }
        let pan: Triple =
            std::array::from_fn(|_| u32::from_str_radix(words.next().unwrap(), 16).unwrap() as i32);
        match tag {
            "D" | "DT" => delta_t = Some(pan),
            "FM" => {
                if let Some(delta_t) = delta_t.take() {
                    let contribution = |left: usize| {
                        (pan[left]
                            .wrapping_sub(delta_t[left])
                            .wrapping_add(pan[2].wrapping_sub(delta_t[2])))
                            >> 13
                    };
                    fm.push([contribution(0), contribution(1)]);
                }
            }
            _ => ay.push(pan.map(|sample| i32::from(sample as i16))),
        }
    }
    (fm, ay)
}

#[test]
#[ignore = "requires KOF97 active native FM and PSG diagnostic captures"]
fn compare_active_kof97_audio() {
    const FRAMES: usize = 24;
    let project = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture = std::env::var_os("KAWAKS_FIXTURE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| project.join(".."));
    let capture = std::env::var_os("KAWAKS_NEO_ACTIVE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| project.join("outputs/kof97-active-audio"));
    let roms = || {
        NeoRoms::load_kof97(
            fixture.join("roms/neogeo/kof97.zip"),
            fixture.join("roms/neogeo/neogeo.zip"),
        )
        .unwrap()
    };

    for slot in [1, 2] {
        let dir = capture.join(format!("slot{slot}"));
        let (reference_fm, reference_psg) =
            components(&fs::read_to_string(dir.join("capture.log")).unwrap());
        let samples = FRAMES * AUDIO_SAMPLES_PER_FRAME;
        assert_eq!(reference_fm.len(), samples, "FM trace size for slot {slot}");
        assert_eq!(
            reference_psg.len(),
            samples,
            "PSG trace size for slot {slot}"
        );
        let state =
            NeoKwk4::read(&fs::read(capture.join(format!("saves/kof97.ks{slot}"))).unwrap())
                .unwrap();
        let mut machine = NeoGeo::from_kwk4(roms(), state).unwrap();
        let (mut fm_diff, mut psg_diff, mut pcm_diff) = (0, 0, 0);
        let (mut fm_nonzero, mut psg_nonzero, mut fm_rust_nonzero) = (0, 0, 0);
        for frame in 1..=FRAMES {
            machine.step_frame(FrameInput::default()).unwrap();
            let audio = machine.main.mem.restored_audio.as_ref().unwrap();
            let read_channel = |ext| {
                let raw = fs::read(dir.join(format!("f{frame:03}.{ext}"))).unwrap();
                assert_eq!(raw.len(), AUDIO_SAMPLES_PER_FRAME * 2);
                raw.chunks_exact(2)
                    .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
                    .collect::<Vec<_>>()
            };
            let left = read_channel("left");
            let right = read_channel("right");
            assert_eq!(audio.source_trace.len(), AUDIO_SAMPLES_PER_FRAME);
            let snapshot = fs::read(dir.join(format!("f{frame:03}.fm-state"))).unwrap();
            let state_diff = audio.fm.compare_snapshot(&snapshot);
            let psg_snapshot = fs::read(dir.join(format!("f{frame:03}.psg-state"))).unwrap();
            let psg_state_diff = audio.psg.compare_phase(&psg_snapshot);
            assert_eq!(
                state_diff, [0; 4],
                "FM phase, increment, stage and envelope position, slot {slot}, frame {frame}"
            );
            assert_eq!(
                psg_state_diff, [0; 3],
                "PSG counters, tone outputs and envelope step, slot {slot}, frame {frame}"
            );
            for (i, (fm, psg)) in audio.source_trace.iter().enumerate() {
                let index = (frame - 1) * AUDIO_SAMPLES_PER_FRAME + i;
                let pcm = machine.frame_audio()[i];
                for side in 0..2 {
                    let expected = reference_fm[index][side];
                    fm_nonzero += usize::from(expected != 0);
                    fm_rust_nonzero += usize::from(fm[side] != 0);
                    fm_diff += usize::from(fm[side] != expected);
                    pcm_diff += usize::from(pcm[side] != [left[i], right[i]][side]);
                }
                for channel in 0..3 {
                    psg_nonzero += usize::from(reference_psg[index][channel] != 0);
                    psg_diff += usize::from(psg[channel] != reference_psg[index][channel]);
                }
            }
        }
        println!(
            "kof97 ks{slot}: fm={fm_diff}/{} psg={psg_diff}/{} pcm={pcm_diff}/{} \
             original_nonzero_fm={fm_nonzero} rust_nonzero_fm={fm_rust_nonzero} \
             original_nonzero_psg={psg_nonzero}",
            samples * 2,
            samples * 3,
            samples * 2,
        );
        assert!(
            fm_nonzero > 0 && fm_rust_nonzero > 0,
            "active FM slot {slot}"
        );
        if slot == 2 {
            assert!(psg_nonzero > 0, "audible PSG diagnostic");
        }
        assert_eq!(fm_diff, 0, "FM output for slot {slot}");
        assert_eq!(psg_diff, 0, "PSG output for slot {slot}");
        assert_eq!(pcm_diff, 0, "Stereo PCM for slot {slot}");
    }
}
