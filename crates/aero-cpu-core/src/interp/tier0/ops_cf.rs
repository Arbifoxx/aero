use super::ops_data::{calc_ea, op_bits, read_op_sized};
use super::ExecOutcome;
use crate::exception::{AssistReason, Exception};
use crate::linear_mem::{
    read_u16_wrapped, read_u32_wrapped, read_u64_wrapped, write_u16_wrapped, write_u32_wrapped,
    write_u64_wrapped,
};
use crate::mem::CpuBus;
use crate::state::{mask_bits, CpuMode, CpuState, FLAG_ZF, RFLAGS_IF, RFLAGS_IOPL_MASK};
use aero_x86::{Code, DecodedInst, Instruction, Mnemonic, OpKind, Register};

pub fn handles_mnemonic(m: Mnemonic) -> bool {
    matches!(
        m,
        Mnemonic::Jmp
            | Mnemonic::Call
            | Mnemonic::Ret
            | Mnemonic::Retf
            | Mnemonic::Jo
            | Mnemonic::Jno
            | Mnemonic::Jb
            | Mnemonic::Jae
            | Mnemonic::Je
            | Mnemonic::Jne
            | Mnemonic::Jbe
            | Mnemonic::Ja
            | Mnemonic::Js
            | Mnemonic::Jns
            | Mnemonic::Jp
            | Mnemonic::Jnp
            | Mnemonic::Jl
            | Mnemonic::Jge
            | Mnemonic::Jle
            | Mnemonic::Jg
            | Mnemonic::Loop
            | Mnemonic::Loope
            | Mnemonic::Loopne
            | Mnemonic::Jcxz
            | Mnemonic::Jecxz
            | Mnemonic::Jrcxz
            | Mnemonic::Nop
            | Mnemonic::Pause
            | Mnemonic::Push
            | Mnemonic::Pop
            | Mnemonic::Pusha
            | Mnemonic::Popa
            | Mnemonic::Pushf
            | Mnemonic::Pushfd
            | Mnemonic::Pushfq
            | Mnemonic::Popf
            | Mnemonic::Popfd
            | Mnemonic::Popfq
            | Mnemonic::Enter
            | Mnemonic::Leave
    )
}

pub fn exec<B: CpuBus>(
    state: &mut CpuState,
    bus: &mut B,
    decoded: &DecodedInst,
    next_ip: u64,
) -> Result<ExecOutcome, Exception> {
    let instr = &decoded.instr;
    match instr.mnemonic() {
        Mnemonic::Nop | Mnemonic::Pause => Ok(ExecOutcome::Continue),
        Mnemonic::Jmp => {
            if is_far_branch(instr) {
                if matches!(state.mode, CpuMode::Real | CpuMode::Vm86) {
                    let (selector, offset) = read_real_mode_far_target(state, bus, instr, next_ip)?;
                    state.write_reg(Register::CS, selector as u64);
                    state.set_rip(offset);
                    return Ok(ExecOutcome::Branch);
                }

                return Ok(ExecOutcome::Assist(AssistReason::Privileged));
            }
            let target = branch_target(state, bus, instr, next_ip)?;
            state.set_rip(target);
            Ok(ExecOutcome::Branch)
        }
        Mnemonic::Call => {
            if is_far_branch(instr) {
                return Ok(ExecOutcome::Assist(AssistReason::Privileged));
            }
            let ret_size = state.bitness() / 8;
            push(state, bus, next_ip, ret_size)?;
            let target = branch_target(state, bus, instr, next_ip)?;
            state.set_rip(target);
            Ok(ExecOutcome::Branch)
        }
        Mnemonic::Ret => {
            let pop_imm = if instr.op_count() == 1 && instr.op_kind(0) == OpKind::Immediate16 {
                instr.immediate16() as u32
            } else {
                0
            };
            let ret_size = state.bitness() / 8;
            let target = pop(state, bus, ret_size)? & mask_bits(state.bitness());
            let sp = state.stack_ptr().wrapping_add(pop_imm as u64);
            state.set_stack_ptr(sp);
            state.set_rip(target);
            Ok(ExecOutcome::Branch)
        }
        Mnemonic::Retf => Ok(ExecOutcome::Assist(AssistReason::Privileged)),
        Mnemonic::Push => {
            let bits = op_bits(state, instr, 0)?;
            let v = read_op_sized(state, bus, instr, 0, bits, next_ip)?;
            push(state, bus, v, bits / 8)?;
            Ok(ExecOutcome::Continue)
        }
        Mnemonic::Pop => {
            if state.mode != crate::state::CpuMode::Bit16
                && instr.op_kind(0) == OpKind::Register
                && is_seg_reg(instr.op0_register())
            {
                return Ok(ExecOutcome::Assist(AssistReason::Privileged));
            }
            let bits = op_bits(state, instr, 0)?;
            let v = pop(state, bus, bits / 8)?;
            super::ops_data::write_op_sized(state, bus, instr, 0, v, bits, next_ip)?;
            // `POP SS` creates an interrupt shadow for the following instruction.
            if instr.op_kind(0) == OpKind::Register && instr.op0_register() == Register::SS {
                Ok(ExecOutcome::ContinueInhibitInterrupts)
            } else {
                Ok(ExecOutcome::Continue)
            }
        }
        Mnemonic::Pusha => {
            let bits = state.bitness();
            if bits != 16 && bits != 32 {
                return Err(Exception::InvalidOpcode);
            }
            let sz = bits / 8;
            let sp_before = state.stack_ptr();
            let regs = if bits == 16 {
                [
                    Register::AX,
                    Register::CX,
                    Register::DX,
                    Register::BX,
                    Register::SP,
                    Register::BP,
                    Register::SI,
                    Register::DI,
                ]
                .to_vec()
            } else {
                [
                    Register::EAX,
                    Register::ECX,
                    Register::EDX,
                    Register::EBX,
                    Register::ESP,
                    Register::EBP,
                    Register::ESI,
                    Register::EDI,
                ]
                .to_vec()
            };
            for r in regs {
                let v = if (bits == 16 && r == Register::SP) || (bits == 32 && r == Register::ESP) {
                    sp_before
                } else {
                    state.read_reg(r)
                };
                push(state, bus, v, sz)?;
            }
            Ok(ExecOutcome::Continue)
        }
        Mnemonic::Popa => {
            let bits = state.bitness();
            if bits != 16 && bits != 32 {
                return Err(Exception::InvalidOpcode);
            }
            let sz = bits / 8;
            let regs = if bits == 16 {
                [
                    Register::DI,
                    Register::SI,
                    Register::BP,
                    Register::SP,
                    Register::BX,
                    Register::DX,
                    Register::CX,
                    Register::AX,
                ]
                .to_vec()
            } else {
                [
                    Register::EDI,
                    Register::ESI,
                    Register::EBP,
                    Register::ESP,
                    Register::EBX,
                    Register::EDX,
                    Register::ECX,
                    Register::EAX,
                ]
                .to_vec()
            };
            for r in regs {
                let v = pop(state, bus, sz)?;
                // POPA ignores the value popped into SP/ESP.
                if (bits == 16 && r == Register::SP) || (bits == 32 && r == Register::ESP) {
                    continue;
                }
                state.write_reg(r, v);
            }
            Ok(ExecOutcome::Continue)
        }
        Mnemonic::Pushf | Mnemonic::Pushfd | Mnemonic::Pushfq => {
            let bits = flags_stack_operand_bits(instr)?;
            let v = state.rflags() & mask_bits(bits);
            push(state, bus, v, bits / 8)?;
            Ok(ExecOutcome::Continue)
        }
        Mnemonic::Popf | Mnemonic::Popfd | Mnemonic::Popfq => {
            let bits = flags_stack_operand_bits(instr)?;
            let v = pop(state, bus, bits / 8)? & mask_bits(bits);
            let old = state.rflags();

            // POPF can update more than the arithmetic status flags. In particular, real-mode
            // firmware and OS kernels frequently use `pushf; cli; ...; popf` to restore IF.
            //
            // In protected/long mode, updates to IOPL/IF are privilege gated:
            // - IOPL can only be modified at CPL0.
            // - IF can only be modified when CPL <= IOPL.
            let mut write_mask = crate::state::FLAG_CF
                | crate::state::FLAG_PF
                | crate::state::FLAG_AF
                | crate::state::FLAG_ZF
                | crate::state::FLAG_SF
                | crate::state::RFLAGS_TF
                | crate::state::FLAG_DF
                | crate::state::FLAG_OF
                | RFLAGS_IF
                | RFLAGS_IOPL_MASK
                | crate::state::RFLAGS_AC
                | crate::state::RFLAGS_ID;
            write_mask &= mask_bits(bits);

            // Do not allow POPF to toggle virtualization/virtual interrupt bits in this model.
            write_mask &=
                !(crate::state::RFLAGS_VM | crate::state::RFLAGS_VIF | crate::state::RFLAGS_VIP);

            if !matches!(state.mode, CpuMode::Real | CpuMode::Vm86) {
                let cpl = state.cpl();
                if cpl != 0 {
                    write_mask &= !RFLAGS_IOPL_MASK;
                }

                let iopl = ((old & RFLAGS_IOPL_MASK) >> 12) as u8;
                if cpl > iopl {
                    write_mask &= !RFLAGS_IF;
                }
            }

            let new = (old & !write_mask) | (v & write_mask);
            state.set_rflags(new);
            Ok(ExecOutcome::Continue)
        }
        Mnemonic::Enter => {
            let bits = enter_leave_operand_bits(instr)?;
            let size = bits / 8;
            let bp_reg = frame_pointer_reg(bits)?;
            let allocation_size = u64::from(instr.immediate16());
            let nesting_level = instr.immediate8_2nd() & 0x1f;

            let old_frame_pointer = state.read_reg(bp_reg) & mask_bits(bits);
            push(state, bus, old_frame_pointer, size)?;
            let frame_temp = state.stack_ptr();

            if nesting_level != 0 {
                let mut frame_pointer = old_frame_pointer;
                for _ in 1..nesting_level {
                    frame_pointer = frame_pointer.wrapping_sub(u64::from(size)) & mask_bits(bits);
                    let addr = state
                        .apply_a20(state.seg_base_reg(Register::SS).wrapping_add(frame_pointer));
                    let value = read_stack_value(state, bus, addr, size)?;
                    push(state, bus, value, size)?;
                }
                push(state, bus, frame_temp, size)?;
            }

            state.write_reg(bp_reg, frame_temp & mask_bits(bits));
            let sp =
                state.stack_ptr().wrapping_sub(allocation_size) & mask_bits(state.stack_ptr_bits());
            state.set_stack_ptr(sp);
            Ok(ExecOutcome::Continue)
        }
        Mnemonic::Leave => {
            let bits = enter_leave_operand_bits(instr)?;
            let size = bits / 8;
            let bp_reg = frame_pointer_reg(bits)?;
            state.set_stack_ptr(state.read_reg(bp_reg));
            let restored_frame_pointer = pop(state, bus, size)?;
            state.write_reg(bp_reg, restored_frame_pointer);
            Ok(ExecOutcome::Continue)
        }
        Mnemonic::Loop | Mnemonic::Loope | Mnemonic::Loopne => {
            let addr_bits = state.bitness();
            let reg = match addr_bits {
                16 => Register::CX,
                32 => Register::ECX,
                64 => Register::RCX,
                _ => Register::ECX,
            };
            let mut count = state.read_reg(reg) & mask_bits(addr_bits);
            count = count.wrapping_sub(1) & mask_bits(addr_bits);
            state.write_reg(reg, count);
            let zf = state.get_flag(FLAG_ZF);
            let cond = match instr.mnemonic() {
                Mnemonic::Loop => count != 0,
                Mnemonic::Loope => count != 0 && zf,
                Mnemonic::Loopne => count != 0 && !zf,
                _ => false,
            };
            if cond {
                let target = instr.near_branch_target();
                state.set_rip(target);
            } else {
                state.set_rip(next_ip);
            }
            Ok(ExecOutcome::Branch)
        }
        Mnemonic::Jcxz | Mnemonic::Jecxz | Mnemonic::Jrcxz => {
            let (reg, bits) = match instr.mnemonic() {
                Mnemonic::Jcxz => (Register::CX, 16),
                Mnemonic::Jecxz => (Register::ECX, 32),
                Mnemonic::Jrcxz => (Register::RCX, 64),
                _ => (Register::ECX, 32),
            };
            let v = state.read_reg(reg) & mask_bits(bits);
            if v == 0 {
                state.set_rip(instr.near_branch_target());
            } else {
                state.set_rip(next_ip);
            }
            Ok(ExecOutcome::Branch)
        }
        m if is_jcc(m) => {
            if super::ops_data::eval_cond(state, m) {
                state.set_rip(instr.near_branch_target());
            } else {
                state.set_rip(next_ip);
            }
            Ok(ExecOutcome::Branch)
        }
        _ => Err(Exception::InvalidOpcode),
    }
}

fn flags_stack_operand_bits(instr: &Instruction) -> Result<u32, Exception> {
    match instr.code() {
        Code::Pushfw | Code::Popfw => Ok(16),
        Code::Pushfd | Code::Popfd => Ok(32),
        Code::Pushfq | Code::Popfq => Ok(64),
        _ => Err(Exception::InvalidOpcode),
    }
}

fn enter_leave_operand_bits(instr: &Instruction) -> Result<u32, Exception> {
    match instr.code() {
        Code::Enterw_imm16_imm8 | Code::Leavew => Ok(16),
        Code::Enterd_imm16_imm8 | Code::Leaved => Ok(32),
        Code::Enterq_imm16_imm8 | Code::Leaveq => Ok(64),
        _ => Err(Exception::InvalidOpcode),
    }
}

fn frame_pointer_reg(bits: u32) -> Result<Register, Exception> {
    match bits {
        16 => Ok(Register::BP),
        32 => Ok(Register::EBP),
        64 => Ok(Register::RBP),
        _ => Err(Exception::InvalidOpcode),
    }
}

fn read_stack_value<B: CpuBus>(
    state: &CpuState,
    bus: &mut B,
    addr: u64,
    size: u32,
) -> Result<u64, Exception> {
    match size {
        2 => Ok(read_u16_wrapped(state, bus, addr)? as u64),
        4 => Ok(read_u32_wrapped(state, bus, addr)? as u64),
        8 => read_u64_wrapped(state, bus, addr),
        _ => Err(Exception::InvalidOpcode),
    }
}

fn is_far_branch(instr: &Instruction) -> bool {
    matches!(instr.op_kind(0), OpKind::FarBranch16 | OpKind::FarBranch32)
        || matches!(
            instr.code(),
            Code::Call_m1616
                | Code::Call_m1632
                | Code::Call_m1664
                | Code::Jmp_m1616
                | Code::Jmp_m1632
                | Code::Jmp_m1664
        )
}

fn read_real_mode_far_target<B: CpuBus>(
    state: &mut CpuState,
    bus: &mut B,
    instr: &Instruction,
    next_ip: u64,
) -> Result<(u16, u64), Exception> {
    match instr.op_kind(0) {
        OpKind::FarBranch16 => Ok((instr.far_branch_selector(), u64::from(instr.far_branch16()))),
        OpKind::FarBranch32 => Ok((instr.far_branch_selector(), u64::from(instr.far_branch32()))),
        OpKind::Memory => {
            let offset_bits = match instr.code() {
                Code::Call_m1616 | Code::Jmp_m1616 => 16,
                Code::Call_m1632 | Code::Jmp_m1632 => 32,
                _ => return Err(Exception::InvalidOpcode),
            };
            let addr = calc_ea(state, instr, next_ip, true)?;
            let offset = super::ops_data::read_mem(state, bus, addr, offset_bits)?;
            let selector_addr = addr.wrapping_add(u64::from(offset_bits / 8));
            let selector = read_u16_wrapped(state, bus, selector_addr)?;
            Ok((selector, offset))
        }
        _ => Err(Exception::InvalidOpcode),
    }
}

fn is_seg_reg(reg: Register) -> bool {
    matches!(
        reg,
        Register::ES | Register::CS | Register::SS | Register::DS | Register::FS | Register::GS
    )
}

fn is_jcc(m: Mnemonic) -> bool {
    matches!(
        m,
        Mnemonic::Jo
            | Mnemonic::Jno
            | Mnemonic::Jb
            | Mnemonic::Jae
            | Mnemonic::Je
            | Mnemonic::Jne
            | Mnemonic::Jbe
            | Mnemonic::Ja
            | Mnemonic::Js
            | Mnemonic::Jns
            | Mnemonic::Jp
            | Mnemonic::Jnp
            | Mnemonic::Jl
            | Mnemonic::Jge
            | Mnemonic::Jle
            | Mnemonic::Jg
    )
}

fn branch_target<B: CpuBus>(
    state: &mut CpuState,
    bus: &mut B,
    instr: &Instruction,
    next_ip: u64,
) -> Result<u64, Exception> {
    match instr.op_kind(0) {
        OpKind::NearBranch16 | OpKind::NearBranch32 | OpKind::NearBranch64 => {
            Ok(instr.near_branch_target())
        }
        OpKind::Register => Ok(state.read_reg(instr.op0_register()) & mask_bits(state.bitness())),
        OpKind::Memory => {
            let bits = op_bits(state, instr, 0)?;
            let addr = calc_ea(state, instr, next_ip, true)?;
            let v = super::ops_data::read_mem(state, bus, addr, bits)?;
            Ok(v & mask_bits(state.bitness()))
        }
        _ => Err(Exception::InvalidOpcode),
    }
}

fn push<B: CpuBus>(
    state: &mut CpuState,
    bus: &mut B,
    val: u64,
    size: u32,
) -> Result<(), Exception> {
    let sp_bits = state.stack_ptr_bits();
    let mut sp = state.stack_ptr();
    sp = sp.wrapping_sub(size as u64) & mask_bits(sp_bits);
    state.set_stack_ptr(sp);
    let addr = state.apply_a20(state.seg_base_reg(Register::SS).wrapping_add(sp));
    match size {
        2 => write_u16_wrapped(state, bus, addr, val as u16),
        4 => write_u32_wrapped(state, bus, addr, val as u32),
        8 => write_u64_wrapped(state, bus, addr, val),
        _ => Err(Exception::InvalidOpcode),
    }
}

fn pop<B: CpuBus>(state: &mut CpuState, bus: &mut B, size: u32) -> Result<u64, Exception> {
    let sp_bits = state.stack_ptr_bits();
    let sp = state.stack_ptr();
    let addr = state.apply_a20(state.seg_base_reg(Register::SS).wrapping_add(sp));
    let v = match size {
        2 => read_u16_wrapped(state, bus, addr)? as u64,
        4 => read_u32_wrapped(state, bus, addr)? as u64,
        8 => read_u64_wrapped(state, bus, addr)?,
        _ => return Err(Exception::InvalidOpcode),
    };
    let new_sp = sp.wrapping_add(size as u64) & mask_bits(sp_bits);
    state.set_stack_ptr(new_sp);
    Ok(v)
}
