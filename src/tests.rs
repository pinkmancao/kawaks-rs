use std::{io::Write, sync::Arc};

use flate2::{write::ZlibEncoder, Compression};
use r68k::{
    interrupts::InterruptController,
    ram::{AddressBus, SUPERVISOR_DATA},
};
use z80emu::Memory;

use crate::{bus::Bus, rom::Regions, Dino, DinoRoms, FrameInput, Kwk4};

fn test_roms() -> DinoRoms {
    let mut program = vec![0; 0x400000];
    program[..8].copy_from_slice(&[0x00, 0xff, 0xff, 0xfe, 0, 0, 1, 0]);
    program[0x68..0x6c].copy_from_slice(&0x120_u32.to_be_bytes());
    program[0x100..0x10e].copy_from_slice(&[
        0x46, 0xfc, 0x20, 0, // MOVE #$2000,SR
        0x13, 0xfc, 0, 0x5a, 0, 0xff, 0, 0, // MOVE.B #$5A,$FF0000
        0x60, 0xfe, // BRA to itself
    ]);
    program[0x120..0x128].copy_from_slice(&[
        0x52, 0x39, 0, 0xff, 0, 1, // ADDQ.B #1,$FF0001
        0x4e, 0x73, // RTE
    ]);
    let mut sound = vec![0; 0x20000];
    sound[..17].copy_from_slice(&[
        0x3e, 0x12, 0x32, 0, 0xd0, // LD A,$12 / LD ($D000),A
        0x3e, 0x34, 0x32, 1, 0xd0, // LD A,$34 / LD ($D001),A
        0x3e, 0x56, 0x32, 2, 0xd0, // LD A,$56 / LD ($D002),A
        0x18, 0xfe, // JR to itself
    ]);
    sound[0x8000] = 0x11;
    sound[0xc000] = 0x22;
    DinoRoms(Arc::new(Regions {
        game: crate::rom::Game::Dino,
        main_opcodes: None,
        program,
        graphics: vec![0; 0x400000],
        sound_data: sound[..0x8000].to_vec(),
        sound_opcodes: sound[..0x8000].to_vec(),
        sound,
        samples: vec![0; 0x200000],
    }))
}

#[test]
fn frame_step_runs_both_cpus_and_dispatches_irq2() {
    let mut dino = Dino::new(test_roms());
    let mut twin = Dino::new(test_roms());
    assert_eq!(dino.report().main_pc, 0x100);
    for frame in 1..=3 {
        let report = dino.step_frame(FrameInput::default()).unwrap();
        assert_eq!(report, twin.step_frame(FrameInput::default()).unwrap());
        assert_eq!(report.frame, frame);
        assert_eq!(report.qsound_commands, 1);
        assert!(report.main_cycles >= 167_680 * frame);
        assert!(report.sound_cycles >= 133_120 * frame);
        assert_eq!(dino.read_main_byte(0xff0000), 0x5a);
        assert_eq!(dino.read_main_byte(0xff0001), frame as u8);
        assert_eq!(dino.main.mem.qsound_registers[0x56], 0x1234);
        assert_eq!(dino.main.int_ctrl.highest_priority(), 0);
    }
}

#[test]
fn mapped_cpu_accesses_preserve_byte_lanes_and_banking() {
    let mut bus = Bus::new(test_roms());
    bus.write_word(SUPERVISOR_DATA, 0xf18020, 0x1234);
    assert_eq!(bus.read_debug(0xc010), 0x34);
    assert_eq!(bus.read_word(SUPERVISOR_DATA, 0xf18020), 0xff34);
    bus.write_mem(0xf004, 0x5a, 0);
    assert_eq!(bus.read_word(SUPERVISOR_DATA, 0xf1e008), 0xff5a);
    assert_eq!(bus.read_debug(0x8000), 0x11);
    bus.write_mem(0xd003, 1, 0);
    assert_eq!(bus.read_debug(0x8000), 0x22);
    bus.write_long(SUPERVISOR_DATA, 0x900002, 0x12345678);
    assert_eq!(bus.read_long(SUPERVISOR_DATA, 0x900002), 0x12345678);
    assert_eq!(bus.main_read8(0x900005), 0x78);
    assert_eq!(bus.main_read8(0x00a00000), 0xff);
}

#[test]
fn kwk4_restores_registers_and_ram_and_rejects_bad_payloads() {
    let mut file = b"KWK4".to_vec();
    let mut work = vec![0; 0x10000];
    work[..2].copy_from_slice(&[0xad, 0xde]);
    chunk(&mut file, &work);
    chunk(&mut file, &[0; 0x200]);
    chunk(&mut file, &vec![0; 0x30000]);
    let mut main = [0; 0x80];
    main[0x44] = 0x20;
    main[0x50..0x54].copy_from_slice(&0x100_u32.to_le_bytes());
    file.extend(main);
    chunk(&mut file, &[0; 0x4000]);
    let mut sound = [0; 0x64];
    sound[0x04..0x08].copy_from_slice(&0xdeadbeef_u32.to_le_bytes());
    sound[0x50..0x52].copy_from_slice(&0x1234_u16.to_le_bytes());
    file.extend(sound);
    file.extend(1_u32.to_le_bytes());
    file.extend([0; 1 + 0x80 + 0x300 + 0x2c + 0x20]);
    let dino = Dino::from_kwk4(test_roms(), Kwk4::read(&file).unwrap()).unwrap();
    assert_eq!(dino.report().main_pc, 0x100);
    assert_eq!(dino.report().sound_pc, 0x1234);
    assert_eq!(dino.read_main_byte(0xff0000), 0xde);
    assert_eq!(dino.read_main_byte(0xff0001), 0xad);
    assert_eq!(dino.main.mem.read_debug(0x8000), 0x22);
    assert!(Kwk4::read(&file[..file.len() - 1]).is_err());
    let mut bad = file.clone();
    bad[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(Kwk4::read(&bad).is_err());
    let mut bad = file;
    bad.push(0);
    assert!(Kwk4::read(&bad).is_err());
}

fn chunk(file: &mut Vec<u8>, raw: &[u8]) {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(raw).unwrap();
    let bytes = encoder.finish().unwrap();
    file.extend((bytes.len() as u32).to_le_bytes());
    file.extend(bytes);
}

#[test]
fn sound_clock_delivers_periodic_irq_to_halted_z80() {
    let mut roms = test_roms();
    let image = Arc::get_mut(&mut roms.0).unwrap();
    let mut fixed = vec![0; 0x8000];
    fixed[..9].copy_from_slice(&[0xed, 0x56, 0x31, 0xf0, 0xff, 0xfb, 0x76, 0x18, 0xfd]);
    fixed[0x38..0x44].copy_from_slice(&[
        0x3a, 0x20, 0xc0, 0x3c, 0x32, 0x20, 0xc0, 0xfb, 0xed, 0x4d, 0, 0,
    ]);
    image.sound_data = fixed.clone();
    image.sound_opcodes = fixed;
    let mut dino = Dino::new(roms);
    dino.step_frame(FrameInput::default()).unwrap();
    assert_eq!(dino.main.mem.read_debug(0xc020), 4);
}

#[test]
fn kabuki_uses_separate_data_and_opcode_views() {
    let raw = [0x00, 0x35, 0xa7, 0xff];
    let (data, opcodes) = crate::kabuki::decode_fixed_rom(&raw, crate::rom::Game::Dino);
    assert_eq!(opcodes, [0x07, 0x91, 0xb2, 0xf8]);
    assert_eq!(data, [0x07, 0xd5, 0xb2, 0xf8]);
}

#[test]
fn sound_slice_completes_indexed_instruction_before_accepting_irq() {
    use z80emu::{Cpu, InterruptMode};

    for prefix in [0xdd, 0xfd] {
        let mut roms = test_roms();
        let image = Arc::get_mut(&mut roms.0).unwrap();
        let mut fixed = vec![0; 0x8000];
        // 512 NOPs leave exactly zero cycles before the indexed LD.
        fixed[0x200..0x204].copy_from_slice(&[prefix, 0x21, 0x34, 0x12]);
        image.sound_data = fixed.clone();
        image.sound_opcodes = fixed;
        let mut dino = Dino::new(roms);
        dino.sound.set_im(InterruptMode::Mode1);
        dino.sound.set_iffs(true, true);
        dino.sound.set_sp(0xffff);
        dino.irq_phase = crate::SOUND_IRQ_PERIOD - crate::SOUND_CYCLES_PER_SLICE - 2;

        dino.step_frame(FrameInput::default()).unwrap();
        let first = &dino.sound_slices[0];
        assert_eq!((first.pc, first.cycles, first.after), (0x204, 2062, 0x38));
        assert_eq!(first.irqs, [(true, 0x204, 2062)]);
    }
}

#[test]
fn sound_block_transfers_charge_sixteen_cycles_per_byte() {
    use z80emu::{Cpu, StkReg16};

    for (opcode, first) in [(0xb0, 0xc100), (0xb8, 0xc101)] {
        let mut roms = test_roms();
        let image = Arc::get_mut(&mut roms.0).unwrap();
        let mut fixed = vec![0; 0x8000];
        fixed[0x200..0x203].copy_from_slice(&[0xed, opcode, 0x76]);
        image.sound_data = fixed.clone();
        image.sound_opcodes = fixed;
        let mut dino = Dino::new(roms);
        dino.main.mem.sound_ram[0x100..0x102].copy_from_slice(&[0x4a, 0x7b]);
        dino.sound.set_reg16(StkReg16::BC, 2);
        dino.sound.set_reg16(StkReg16::HL, first);
        dino.sound.set_reg16(StkReg16::DE, first + 0x100);

        dino.step_frame(FrameInput::default()).unwrap();
        let slice = &dino.sound_slices[0];
        assert_eq!((slice.pc, slice.cycles), (0x200, 2064));
        assert_eq!(&dino.main.mem.sound_ram[0x200..0x202], &[0x4a, 0x7b]);
        assert_eq!(dino.sound.get_reg16(StkReg16::BC), 0);
    }
}

#[test]
fn player_three_coin_and_start_use_the_extra_controller_port() {
    let mut dino = Dino::new(test_roms());
    dino.step_frame(FrameInput {
        players: [0, 0, 0x3008, 0],
    })
    .unwrap();
    assert_eq!(dino.read_main_byte(0xf1c001), 0x3e);
    assert_eq!(dino.read_main_byte(0x800018), 0xff);
}
