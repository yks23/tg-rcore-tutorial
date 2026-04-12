//! VirtIO-GPU 驱动模块
//!
//! 为 ch6 提供 VirtIO-GPU framebuffer 支持。
//! 内核维护一个全局 GPU 驱动实例，用户程序通过自定义 syscall 622/623 读写帧缓冲。
//!
//! ## syscall 接口
//! - `622` `draw_framebuffer(buf_ptr, w, h)`：将用户态帧缓冲数据刷新到屏幕
//! - `623` `get_fb_info(info_ptr)`：获取屏幕分辨率 `[width: u32, height: u32]`

use crate::{build_flags, Sv39, KERNEL_SPACE};
use alloc::{
    alloc::{alloc_zeroed, dealloc},
    sync::Arc,
};
use core::{alloc::Layout, ptr::NonNull};
use spin::{Lazy, Mutex};
use tg_kernel_vm::page_table::{MmuMeta, VAddr, VmFlags};
use virtio_drivers::{Hal, MmioTransport, PhysAddr, VirtAddr, VirtIOGpu, VirtIOHeader};

/// VirtIO GPU 设备 MMIO 基地址（QEMU virt 平台，第一个 virtio-gpu 设备在最高 slot）
pub const VIRTIO_GPU_BASE: usize = 0x1000_8000;

/// VirtIO GPU 内存映射区域（需加入内核地址空间）
pub const GPU_MMIO: (usize, usize) = (VIRTIO_GPU_BASE, 0x1000);

/// 全局 GPU 驱动封装（延迟初始化）
pub static GPU_DEVICE: Lazy<Arc<Mutex<GpuDriver>>> = Lazy::new(|| {
    Arc::new(Mutex::new(GpuDriver::new().expect("VirtIO-GPU init failed")))
});

/// VirtIO HAL 实现（复用 ch6 的堆分配器，恒等映射）
pub struct VirtioHal;

impl Hal for VirtioHal {
    fn dma_alloc(pages: usize) -> PhysAddr {
        unsafe {
            alloc_zeroed(Layout::from_size_align_unchecked(
                pages << Sv39::PAGE_BITS,
                1 << Sv39::PAGE_BITS,
            )) as _
        }
    }

    fn dma_dealloc(paddr: PhysAddr, pages: usize) -> i32 {
        unsafe {
            dealloc(
                paddr as _,
                Layout::from_size_align_unchecked(pages << Sv39::PAGE_BITS, 1 << Sv39::PAGE_BITS),
            );
        }
        0
    }

    fn phys_to_virt(paddr: PhysAddr) -> VirtAddr {
        paddr
    }

    fn virt_to_phys(vaddr: VirtAddr) -> PhysAddr {
        const VALID: VmFlags<Sv39> = build_flags("__V");
        let ptr: NonNull<u8> = unsafe {
            KERNEL_SPACE
                .assume_init_ref()
                .translate(VAddr::new(vaddr), VALID)
                .unwrap_or_else(|| NonNull::new(vaddr as *mut u8).unwrap())
        };
        ptr.as_ptr() as usize
    }
}

/// GPU 驱动封装（持有 VirtIOGpu 实例和帧缓冲信息）
pub struct GpuDriver {
    gpu: VirtIOGpu<'static, VirtioHal, MmioTransport>,
    pub width: u32,
    pub height: u32,
}

// Safety: 单核内核（无并发），Mutex 保证互斥访问
unsafe impl Send for GpuDriver {}
unsafe impl Sync for GpuDriver {}

impl GpuDriver {
    /// 初始化 VirtIO-GPU 驱动
    pub fn new() -> Option<Self> {
        let header = NonNull::new(VIRTIO_GPU_BASE as *mut VirtIOHeader)?;
        let transport = unsafe { MmioTransport::new(header).ok()? };
        let mut gpu = VirtIOGpu::<VirtioHal, MmioTransport>::new(transport).ok()?;
        let (width, height) = gpu.resolution().ok()?;
        let _fb = gpu.setup_framebuffer().ok()?;
        Some(GpuDriver { gpu, width, height })
    }

    /// 获取内核侧帧缓冲（setup_framebuffer 返回的 &mut [u8]）
    ///
    /// 注意：virtio-drivers 0.1.0 的 setup_framebuffer 返回 &'static mut [u8]，
    /// 需要在初始化时保存。这里通过重新访问 DMA 区域实现帧缓冲写入。
    pub fn write_fb_from_user(&mut self, src: *const u8, size: usize) -> bool {
        let fb = match self.gpu.setup_framebuffer() {
            Ok(fb) => fb,
            Err(_) => return false,
        };
        if size > fb.len() {
            return false;
        }
        unsafe {
            core::ptr::copy_nonoverlapping(src, fb.as_mut_ptr(), size.min(fb.len()));
        }
        self.gpu.flush().is_ok()
    }
}
