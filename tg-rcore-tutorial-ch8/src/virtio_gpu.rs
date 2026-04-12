//! VirtIO-GPU 驱动模块
//!
//! 为 ch6 提供 VirtIO-GPU framebuffer 支持。
//! 使用 `virtio-drivers` crate 的 `VirtIOGpu` 支持 legacy/modern 两种 MMIO transport。
//! 帧缓冲使用静态 BSS 内存（FB_BUF），通过自定义 HAL 将大块 DMA 分配重定向到静态数组。
//!
//! ## syscall 接口
//! - `622` `draw_framebuffer(buf_ptr, w, h)`：将用户态帧缓冲数据刷新到屏幕
//! - `623` `get_fb_info(info_ptr)`：获取屏幕分辨率 `[width: u32, height: u32]`

use crate::{build_flags, Sv39, KERNEL_SPACE};
use alloc::{alloc::alloc_zeroed, sync::Arc};
use core::{alloc::Layout, ptr::NonNull, sync::atomic::{AtomicUsize, Ordering}};
use spin::{Lazy, Mutex};
use tg_kernel_vm::page_table::{VAddr, VmFlags};
use virtio_drivers::{Hal, MmioTransport, PhysAddr, VirtAddr, VirtIOGpu, VirtIOHeader};

/// VirtIO GPU MMIO 区域（覆盖全部 8 个 VirtIO slot，含块设备和 GPU）
pub const GPU_MMIO: (usize, usize) = (0x1000_1000, 0x8000);

/// 全局 GPU 驱动封装（延迟初始化）
pub static GPU_DEVICE: Lazy<Arc<Mutex<GpuDriver>>> = Lazy::new(|| {
    Arc::new(Mutex::new(GpuDriver::new().expect("VirtIO-GPU init failed")))
});

// ─── 静态帧缓冲（BSS）────────────────────────────────────────────────────────
// 最大支持 1280×800×4 = 4,096,000 字节，避免大块堆分配
const FB_MAX_SIZE: usize = 1280 * 800 * 4;
const PAGE_SIZE: usize = 4096;

#[repr(C, align(4096))]
struct FrameBuf([u8; FB_MAX_SIZE]);
static mut FRAME_BUF: FrameBuf = FrameBuf([0u8; FB_MAX_SIZE]);

// ─── 小 DMA 池（virtqueue 用，约 32 KiB）────────────────────────────────────
const DMA_POOL_PAGES: usize = 16; // 64 KiB

#[repr(C, align(4096))]
struct DmaPool([u8; DMA_POOL_PAGES * PAGE_SIZE]);
static mut DMA_POOL: DmaPool = DmaPool([0u8; DMA_POOL_PAGES * PAGE_SIZE]);
static DMA_NEXT: AtomicUsize = AtomicUsize::new(0);

/// VirtIO HAL 实现：
/// - 小块（virtqueue）→ 静态 DMA 池
/// - 大块（帧缓冲）   → 静态 FRAME_BUF
pub struct VirtioHal;

impl Hal for VirtioHal {
    fn dma_alloc(pages: usize) -> PhysAddr {
        let size = pages * PAGE_SIZE;
        if size > FB_MAX_SIZE / 2 {
            // 大块分配：直接返回 FRAME_BUF 的地址（静态 BSS，无需堆分配）
            (&raw mut FRAME_BUF) as PhysAddr
        } else {
            // 小块分配：从静态 DMA 池分配（bump allocator）
            let off = DMA_NEXT.fetch_add(pages, Ordering::Relaxed);
            if off + pages <= DMA_POOL_PAGES {
                unsafe { ((&raw mut DMA_POOL) as *mut u8).add(off * PAGE_SIZE) as PhysAddr }
            } else {
                // 池用完，退化到堆分配
                unsafe {
                    alloc_zeroed(Layout::from_size_align_unchecked(size, PAGE_SIZE)) as PhysAddr
                }
            }
        }
    }

    fn dma_dealloc(_paddr: PhysAddr, _pages: usize) -> i32 {
        0 // 静态池不释放
    }

    fn phys_to_virt(paddr: PhysAddr) -> VirtAddr {
        paddr // 恒等映射
    }

    fn virt_to_phys(vaddr: VirtAddr) -> PhysAddr {
        const VALID: VmFlags<Sv39> = build_flags("__V");
        let ptr: NonNull<u8> = unsafe {
            KERNEL_SPACE
                .assume_init_ref()
                .translate(VAddr::new(vaddr), VALID)
                .unwrap_or_else(|| NonNull::new(vaddr as *mut u8).unwrap())
        };
        ptr.as_ptr() as PhysAddr
    }
}

/// GPU 驱动封装
pub struct GpuDriver {
    gpu: VirtIOGpu<'static, VirtioHal, MmioTransport>,
    pub width: u32,
    pub height: u32,
    fb_ptr: *mut u8,
    fb_len: usize,
}

// Safety: 单核内核，Mutex 保证互斥
unsafe impl Send for GpuDriver {}
unsafe impl Sync for GpuDriver {}

impl GpuDriver {
    /// 扫描所有 VirtIO MMIO slot，找到 GPU 并初始化
    pub fn new() -> Option<Self> {
        // 找 GPU 设备（device_id=16）
        let gpu_base = (0x1000_1000usize..=0x1000_8000)
            .step_by(0x1000)
            .find(|&addr| {
                let magic = unsafe { core::ptr::read_volatile(addr as *const u32) };
                let dev_id = unsafe { core::ptr::read_volatile((addr + 8) as *const u32) };
                magic == 0x7472_6976 && dev_id == 16
            })?;

        let header = NonNull::new(gpu_base as *mut VirtIOHeader)?;
        let transport = unsafe { MmioTransport::new(header).ok()? };
        let mut gpu = VirtIOGpu::<VirtioHal, MmioTransport>::new(transport).ok()?;
        let (width, height) = gpu.resolution().ok()?;
        // setup_framebuffer 通过 dma_alloc 分配，大块分配走 FRAME_BUF
        let fb = gpu.setup_framebuffer().ok()?;
        let fb_ptr = fb.as_mut_ptr();
        let fb_len = fb.len();
        Some(GpuDriver { gpu, width, height, fb_ptr, fb_len })
    }

    /// 从用户态 src 拷贝到帧缓冲并刷新到屏幕
    pub fn write_fb_from_user(&mut self, src: *const u8, size: usize) -> bool {
        let copy_len = size.min(self.fb_len);
        unsafe { core::ptr::copy_nonoverlapping(src, self.fb_ptr, copy_len); }
        self.gpu.flush().is_ok()
    }
}
