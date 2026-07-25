use aero_cpu_core::assist::AssistContext;
use aero_cpu_core::interp::tier0::exec::{
    run_batch, run_batch_cpu_core_with_assists, step, BatchExit, StepExit,
};
use aero_cpu_core::interp::tier0::Tier0Config;
use aero_cpu_core::mem::{CpuBus, FlatTestBus};
use aero_cpu_core::state::{CpuMode, CpuState, FLAG_CF, FLAG_ZF, RFLAGS_IF};
use aero_cpu_core::AssistReason;
use aero_cpu_core::CpuCore;
use aero_x86::Register;

fn run_to_halt(state: &mut CpuState, bus: &mut FlatTestBus, max: u64) {
    let mut steps = 0;
    while steps < max {
        let res = run_batch(state, bus, 1024);
        steps += res.executed;
        match res.exit {
            BatchExit::Completed | BatchExit::Branch => continue,
            BatchExit::Halted => return,
            BatchExit::BiosInterrupt(vector) => panic!("unexpected BIOS interrupt: {vector:#x}"),
            BatchExit::Assist(r) => panic!("unexpected assist: {r:?}"),
            BatchExit::Exception(e) => panic!("unexpected exception: {e:?}"),
            BatchExit::CpuExit(e) => panic!("unexpected cpu exit: {e:?}"),
        }
    }
    panic!("program did not halt");
}

#[test]
fn mov_add_sub_flags() {
    // mov eax,1; add eax,-1; hlt
    let code = [
        0xB8, 0x01, 0x00, 0x00, 0x00, // mov eax,1
        0x83, 0xC0, 0xFF, // add eax,-1 (imm8 sign-extended)
        0xF4, // hlt
    ];
    let mut bus = FlatTestBus::new(0x1000);
    bus.load(0, &code);
    let mut state = CpuState::new(CpuMode::Bit32);
    state.set_rip(0);
    run_to_halt(&mut state, &mut bus, 100);
    assert_eq!(state.read_reg(aero_x86::Register::EAX), 0);
    assert!(state.get_flag(FLAG_ZF));
}

#[test]
fn popf_restores_interrupt_flag_in_real_mode() {
    // push 0x0202; popf; hlt
    let code = [
        0x68, 0x02, 0x02, // push 0x0202 (IF=1, reserved1=1)
        0x9D, // popf
        0xF4, // hlt
    ];
    let mut bus = FlatTestBus::new(0x1000);
    bus.load(0, &code);
    let mut state = CpuState::new(CpuMode::Bit16);
    state.set_rip(0);
    state.write_reg(Register::SS, 0);
    state.write_reg(Register::SP, 0x800);
    run_to_halt(&mut state, &mut bus, 20);
    assert_ne!(state.rflags() & RFLAGS_IF, 0);
}

#[test]
fn pushfd_popfd_use_operand_override_width_in_16_bit_mode() {
    let code = [0x66, 0x9C, 0x66, 0x9D];
    let mut bus = FlatTestBus::new(0x1000);
    bus.load(0, &code);
    let mut state = CpuState::new(CpuMode::Bit16);
    state.write_reg(Register::SS, 0);
    state.write_reg(Register::SP, 0x800);
    state.set_rflags(0x0020_0202);

    assert_eq!(step(&mut state, &mut bus).unwrap(), StepExit::Continue);
    assert_eq!(state.read_reg(Register::SP), 0x7FC);
    assert_eq!(bus.read_u32(0x7FC).unwrap(), 0x0020_0202);

    assert_eq!(step(&mut state, &mut bus).unwrap(), StepExit::Continue);
    assert_eq!(state.read_reg(Register::SP), 0x800);
    assert_eq!(state.rflags() & 0x0020_0202, 0x0020_0202);
}

#[test]
fn call_ret_stack() {
    // 0: mov eax, 5
    // 5: call 0x0B
    // A: hlt
    // B: inc eax
    // C: ret
    let code = [
        0xB8, 0x05, 0x00, 0x00, 0x00, // mov eax,5
        0xE8, 0x01, 0x00, 0x00, 0x00, // call +1 (to 0x0B)
        0xF4, // hlt
        0x40, // inc eax
        0xC3, // ret
    ];
    let mut bus = FlatTestBus::new(0x1000);
    bus.load(0, &code);
    let mut state = CpuState::new(CpuMode::Bit32);
    state.set_rip(0);
    state.write_reg(aero_x86::Register::ESP, 0x800);
    run_to_halt(&mut state, &mut bus, 100);
    assert_eq!(state.read_reg(aero_x86::Register::EAX), 6);
}

#[test]
fn enter_leave_build_and_destroy_stack_frames() {
    for (mode, prefix, sp_reg, bp_reg, size) in [
        (CpuMode::Bit16, &[][..], Register::SP, Register::BP, 2u64),
        (CpuMode::Bit32, &[][..], Register::ESP, Register::EBP, 4u64),
        (CpuMode::Bit64, &[][..], Register::RSP, Register::RBP, 8u64),
    ] {
        let mut code = prefix.to_vec();
        code.extend_from_slice(&[0xC8, 0x20, 0x00, 0x00, 0xC9]);
        let mut bus = FlatTestBus::new(0x1000);
        bus.load(0, &code);
        let mut state = CpuState::new(mode);
        state.write_reg(Register::SS, 0);
        state.write_reg(sp_reg, 0x800);
        state.write_reg(bp_reg, 0x900);

        assert_eq!(step(&mut state, &mut bus).unwrap(), StepExit::Continue);
        assert_eq!(state.read_reg(bp_reg), 0x800 - size);
        assert_eq!(state.read_reg(sp_reg), 0x800 - size - 0x20);
        match size {
            2 => assert_eq!(bus.read_u16(0x800 - size).unwrap(), 0x900),
            4 => assert_eq!(bus.read_u32(0x800 - size).unwrap(), 0x900),
            8 => assert_eq!(bus.read_u64(0x800 - size).unwrap(), 0x900),
            _ => unreachable!(),
        }

        assert_eq!(step(&mut state, &mut bus).unwrap(), StepExit::Continue);
        assert_eq!(state.read_reg(sp_reg), 0x800);
        assert_eq!(state.read_reg(bp_reg), 0x900);
    }
}

#[test]
fn enter_nested_frame_copies_ancestor_and_frame_temp() {
    // ENTER 0,2 pushes BP, copies [SS:BP-2], then pushes the new frame pointer.
    let code = [0xC8, 0x00, 0x00, 0x02];
    let mut bus = FlatTestBus::new(0x1000);
    bus.load(0, &code);
    bus.write_u16(0x8FE, 0x7777).unwrap();
    let mut state = CpuState::new(CpuMode::Bit16);
    state.write_reg(Register::SS, 0);
    state.write_reg(Register::SP, 0x800);
    state.write_reg(Register::BP, 0x900);

    assert_eq!(step(&mut state, &mut bus).unwrap(), StepExit::Continue);
    assert_eq!(state.read_reg(Register::BP), 0x7FE);
    assert_eq!(state.read_reg(Register::SP), 0x7FA);
    assert_eq!(bus.read_u16(0x7FE).unwrap(), 0x900);
    assert_eq!(bus.read_u16(0x7FC).unwrap(), 0x7777);
    assert_eq!(bus.read_u16(0x7FA).unwrap(), 0x7FE);
}

#[test]
fn real_mode_segment_mov_updates_base() {
    // mov ax,0x1000; mov ds,ax; mov byte ptr [0x20],0xAA; hlt
    let code = [
        0xB8, 0x00, 0x10, // mov ax,0x1000
        0x8E, 0xD8, // mov ds,ax
        0xC6, 0x06, 0x20, 0x00, 0xAA, // mov byte ptr [0x20],0xAA
        0xF4, // hlt
    ];
    let mut bus = FlatTestBus::new(0x20000);
    bus.load(0, &code);
    let mut state = CpuState::new(CpuMode::Bit16);
    state.set_rip(0);
    run_to_halt(&mut state, &mut bus, 100);
    assert_eq!(bus.read_u8(0x10000 + 0x20).unwrap(), 0xAA);
}

#[test]
fn real_mode_les_loads_offset_and_segment_base() {
    // les bx, far [0x00F2]
    let code = [0xC4, 0x1E, 0xF2, 0x00];
    let mut bus = FlatTestBus::new(0x30000);
    bus.load(0, &code);
    bus.write_u16(0xF2, 0x1234).unwrap();
    bus.write_u16(0xF4, 0x2000).unwrap();
    let mut state = CpuState::new(CpuMode::Bit16);

    assert_eq!(step(&mut state, &mut bus).unwrap(), StepExit::Continue);
    assert_eq!(state.read_reg(Register::BX), 0x1234);
    assert_eq!(state.read_reg(Register::ES), 0x2000);
    assert_eq!(state.segments.es.base, 0x20000);
}

#[test]
fn real_mode_stack_push_wraps_with_a20_disabled() {
    // push ax; hlt
    let code = [0x50, 0xF4];
    let mut bus = FlatTestBus::new(0x10000);
    bus.load(0x200, &code);

    let mut state = CpuState::new(CpuMode::Bit16);
    state.set_rip(0x200);
    state.a20_enabled = false;
    state.write_reg(Register::SS, 0xFFFF);
    state.write_reg(Register::SP, 0x0012);
    state.write_reg(Register::AX, 0xBEEF);

    run_to_halt(&mut state, &mut bus, 20);
    assert_eq!(bus.read_u16(0).unwrap(), 0xBEEF);
}

#[test]
fn far_jump_executes_in_real_mode() {
    // ljmp 0x0000:0x0005 (EA 05 00 00 00)
    let code = [0xEA, 0x05, 0x00, 0x00, 0x00, 0xF4];
    let mut bus = FlatTestBus::new(0x1000);
    bus.load(0, &code);
    let mut state = CpuState::new(CpuMode::Bit16);
    state.set_rip(0);

    let res = run_batch(&mut state, &mut bus, 1);
    assert!(matches!(res.exit, BatchExit::Branch));
    assert_eq!(state.rip(), 5);
}

#[test]
fn indirect_far_jump_with_segment_override_executes_in_real_mode() {
    // jmp far ptr es:[bx-4]
    let code = [0x26, 0xFF, 0x6F, 0xFC];
    let mut bus = FlatTestBus::new(0x30000);
    bus.load(0, &code);
    bus.write_u16(0x100FC, 0x1234).unwrap();
    bus.write_u16(0x100FE, 0x2000).unwrap();
    let mut state = CpuState::new(CpuMode::Bit16);
    state.write_reg(Register::ES, 0x1000);
    state.write_reg(Register::BX, 0x0100);

    let res = run_batch(&mut state, &mut bus, 1);
    assert!(matches!(res.exit, BatchExit::Branch));
    assert_eq!(state.segments.cs.selector, 0x2000);
    assert_eq!(state.segments.cs.base, 0x20000);
    assert_eq!(state.rip(), 0x1234);
}

#[test]
fn indirect_near_jump_reads_dword_offset_from_jump_table() {
    // jmp dword ptr [eax*4+0x100]
    let code = [0xFF, 0x24, 0x85, 0x00, 0x01, 0x00, 0x00];
    let mut bus = FlatTestBus::new(0x1000);
    bus.load(0, &code);
    bus.write_u32(0x108, 0x345).unwrap();
    let mut state = CpuState::new(CpuMode::Bit32);
    state.write_reg(Register::EAX, 2);

    let res = run_batch(&mut state, &mut bus, 1);
    assert!(matches!(res.exit, BatchExit::Branch));
    assert_eq!(state.rip(), 0x345);
}

#[test]
fn retfd_uses_operand_override_width_in_16_bit_mode() {
    // retfd
    let code = [0x66, 0xCB];
    let mut bus = FlatTestBus::new(0x30000);
    bus.load(0, &code);
    bus.write_u32(0x800, 0x0000_1234).unwrap();
    bus.write_u32(0x804, 0x0000_2000).unwrap();
    let mut cpu = CpuCore::new(CpuMode::Bit16);
    cpu.state.write_reg(Register::SS, 0);
    cpu.state.write_reg(Register::SP, 0x800);

    let result = run_batch_cpu_core_with_assists(
        &Tier0Config::default(),
        &mut AssistContext::default(),
        &mut cpu,
        &mut bus,
        1,
    );
    assert!(matches!(result.exit, BatchExit::Branch));
    assert_eq!(cpu.state.segments.cs.selector, 0x2000);
    assert_eq!(cpu.state.segments.cs.base, 0x20000);
    assert_eq!(cpu.state.rip(), 0x1234);
    assert_eq!(cpu.state.read_reg(Register::SP), 0x808);
}

#[test]
fn mov_cr0_requests_assist() {
    // mov cr0,eax (0F 22 C0)
    let code = [0x0F, 0x22, 0xC0];
    let mut bus = FlatTestBus::new(0x1000);
    bus.load(0, &code);
    let mut state = CpuState::new(CpuMode::Bit32);
    state.set_rip(0);

    let res = run_batch(&mut state, &mut bus, 1);
    assert!(matches!(
        res.exit,
        BatchExit::Assist(AssistReason::Privileged)
    ));
    assert_eq!(state.rip(), 0);
}

#[test]
fn string_io_requests_io_assist() {
    // insb (6C)
    let code = [0x6C];
    let mut bus = FlatTestBus::new(0x1000);
    bus.load(0, &code);
    let mut state = CpuState::new(CpuMode::Bit32);
    state.set_rip(0);

    let res = run_batch(&mut state, &mut bus, 1);
    assert!(matches!(res.exit, BatchExit::Assist(AssistReason::Io)));
    assert_eq!(state.rip(), 0);

    // outsb (6E)
    let code = [0x6E];
    let mut bus = FlatTestBus::new(0x1000);
    bus.load(0, &code);
    let mut state = CpuState::new(CpuMode::Bit32);
    state.set_rip(0);

    let res = run_batch(&mut state, &mut bus, 1);
    assert!(matches!(res.exit, BatchExit::Assist(AssistReason::Io)));
    assert_eq!(state.rip(), 0);
}

#[test]
fn fast_syscall_instructions_request_privileged_assist() {
    // sysenter (0F 34)
    let code = [0x0F, 0x34];
    let mut bus = FlatTestBus::new(0x1000);
    bus.load(0, &code);
    let mut state = CpuState::new(CpuMode::Bit32);
    state.set_rip(0);

    let res = run_batch(&mut state, &mut bus, 1);
    assert!(matches!(
        res.exit,
        BatchExit::Assist(AssistReason::Privileged)
    ));
    assert_eq!(state.rip(), 0);

    // syscall (0F 05) - only valid in long mode but should still be decoded and surfaced as an assist.
    let code = [0x0F, 0x05];
    let mut bus = FlatTestBus::new(0x1000);
    bus.load(0, &code);
    let mut state = CpuState::new(CpuMode::Bit64);
    state.set_rip(0);

    let res = run_batch(&mut state, &mut bus, 1);
    assert!(matches!(
        res.exit,
        BatchExit::Assist(AssistReason::Privileged)
    ));
    assert_eq!(state.rip(), 0);
}

#[test]
fn bsf_bsr_bt() {
    // mov eax,0x10; bsf ecx,eax; bsr edx,eax; bt eax,4; hlt
    let code = [
        0xB8, 0x10, 0x00, 0x00, 0x00, // mov eax,0x10
        0x0F, 0xBC, 0xC8, // bsf ecx,eax
        0x0F, 0xBD, 0xD0, // bsr edx,eax
        0x0F, 0xA3, 0xE0, // bt eax,esp? actually modrm E0 => /4? fix below
        0xF4,
    ];
    let mut bus = FlatTestBus::new(0x1000);
    // Fix bt eax,4 using imm8 form: 0F BA /4 ib; modrm E0 means mod=11 reg=4 rm=0 (EAX)
    let mut prog = Vec::new();
    prog.extend_from_slice(&code[..11]);
    prog.extend_from_slice(&[0x0F, 0xBA, 0xE0, 0x04]); // bt eax,4
    prog.push(0xF4);
    bus.load(0, &prog);
    let mut state = CpuState::new(CpuMode::Bit32);
    state.set_rip(0);
    run_to_halt(&mut state, &mut bus, 100);
    assert_eq!(state.read_reg(aero_x86::Register::ECX), 4);
    assert_eq!(state.read_reg(aero_x86::Register::EDX), 4);
    assert!(state.get_flag(FLAG_CF));
}

#[test]
fn fuzz_lite_no_panics() {
    // Build a small random-ish program made of safe encodings and make sure
    // the interpreter never panics and always makes forward progress.
    let mut seed = 0x1234_5678_u64;
    let mut code: Vec<u8> = Vec::new();
    for _ in 0..200 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let choice = (seed >> 32) % 5;
        let reg = ((seed >> 40) & 7) as u8;
        match choice {
            0 => {
                // mov r32, imm32
                code.push(0xB8 + reg);
                code.extend_from_slice(&(seed as u32).to_le_bytes());
            }
            1 => {
                // add r32, imm8  (83 /0 ib)
                code.extend_from_slice(&[0x83, 0xC0 | reg, (seed >> 8) as u8]);
            }
            2 => {
                // sub r32, imm8  (83 /5 ib)
                code.extend_from_slice(&[0x83, 0xE8 | reg, (seed >> 8) as u8]);
            }
            3 => {
                // xor r32, r32 (31 /r)
                code.extend_from_slice(&[0x31, 0xC0 | (reg << 3) | reg]);
            }
            _ => {
                // jz +0 (always in range)
                code.extend_from_slice(&[0x74, 0x00]);
            }
        }
    }
    code.push(0xF4); // hlt

    let mut bus = FlatTestBus::new(0x4000);
    bus.load(0, &code);
    let mut state = CpuState::new(CpuMode::Bit32);
    state.set_rip(0);
    state.write_reg(aero_x86::Register::ESP, 0x2000);

    let mut prev_ip = state.rip();
    for _ in 0..5000 {
        let res = run_batch(&mut state, &mut bus, 1);
        if matches!(res.exit, BatchExit::Exception(_)) {
            panic!("unexpected exception");
        }
        let ip = state.rip();
        assert_ne!(ip, prev_ip, "IP did not progress");
        prev_ip = ip;
        if matches!(res.exit, BatchExit::Halted) {
            return;
        }
    }
    panic!("did not halt");
}
