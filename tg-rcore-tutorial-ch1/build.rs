//! 构建脚本：为 RISC-V64 目标自动生成链接脚本。
//!
//! 链接脚本控制程序各段在内存中的布局，确保：
//! - M-mode 代码（tg-sbi）从 0x80000000 开始
//! - S-mode 代码（_start 入口）从 0x80200000 开始

fn main() {
    use std::{env, fs, path::PathBuf};

    write_qemu_smp();

    // 仅在交叉编译到 RISC-V64 时生成链接脚本
    if env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default() == "riscv64" {
        let ld = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("linker.ld");
        fs::write(&ld, LINKER_SCRIPT).unwrap();
        // 告诉 rustc 使用此链接脚本
        println!("cargo:rustc-link-arg=-T{}", ld.display());
    }
}

/// 写入 `OUT_DIR/qemu_smp.rs`，供 `main.rs` `include!`；与 QEMU 脚本 `RCORE_SMP` 对齐（验收单核/多核开关）。
fn write_qemu_smp() {
    use std::{env, fs, path::PathBuf};

    println!("cargo:rerun-if-env-changed=RCORE_SMP");
    let n = env::var("RCORE_SMP").unwrap_or_else(|_| "8".to_string());
    let n: usize = n.parse().unwrap_or(8).clamp(1, 8);
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("qemu_smp.rs");
    fs::write(
        &out,
        format!(
            "/// QEMU `-smp` 核数（构建时 `RCORE_SMP`，默认 8，与 `scripts/qemu-virt-kernel.sh` 一致）。\npub const QEMU_SMP: usize = {n};\n"
        ),
    )
    .unwrap();
}

/// 链接脚本内容。
///
/// 内存布局：
///
/// ```text
/// 0x80000000  M-mode 区域（tg-sbi 提供）
///   .text.m_entry   M-mode 入口代码
///   .text.m_trap    M-mode 中断处理
///   .bss.m_stack    M-mode 栈空间
///   .bss.m_data     M-mode 数据
///
/// 0x80200000  S-mode 区域（本程序）
///   .text           代码段（含 .text.entry 入口）
///   .rodata         只读数据段
///   .data           可读写数据段
///   .bss            未初始化数据段（含栈空间）
/// ```
///
/// 注意：链接脚本是字节字符串，不能包含非 ASCII 字符，
/// 因此脚本内注释使用英文。
const LINKER_SCRIPT: &[u8] = b"
OUTPUT_ARCH(riscv)
ENTRY(_m_start)

/* M-mode code base address: start of RAM on QEMU virt platform */
M_BASE_ADDRESS = 0x80000000;
/* S-mode code base address: M-mode jumps here after init */
S_BASE_ADDRESS = 0x80200000;

SECTIONS {
    /* ===== M-mode region (provided by tg-sbi) ===== */
    . = M_BASE_ADDRESS;
    .text.m_entry : { *(.text.m_entry) }
    .text.m_trap  : { *(.text.m_trap)  }
    .bss.m_stack  : { *(.bss.m_stack)  }
    .bss.m_data   : { *(.bss.m_data)   }

    /* ===== S-mode region (this program) ===== */
    . = S_BASE_ADDRESS;
    .text   : {
        *(.text.entry)          /* _start entry, must come first */
        *(.text .text.*)        /* other code */
    }
    .rodata : {
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
    }
    .data   : {
        *(.data .data.*)
        *(.sdata .sdata.*)
    }
    .bss    : {
        *(.bss.uninit)          /* stack space */
        *(.bss .bss.*)
        *(.sbss .sbss.*)
    }
}";
