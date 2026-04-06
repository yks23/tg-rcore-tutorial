//! 死锁预判：与 **crates.io** 上 `tg-rcore-tutorial-sync` 0.4.8 对齐的桩实现。
//!
//! 该版本 sync 无 `deadlock_snapshot`；此处恒为「安全」，保证 `cargo publish` 校验可通过。
//! 若使用本地扩展的 `tg-rcore-tutorial-sync`（含快照 API），可恢复银行家算法实现。

use crate::process::Process;
use tg_task_manage::ThreadId;

/// `mutex_lock` / `semaphore_down` 判定将不安全时返回的用户态约定值
pub const DEADLOCK_RET: isize = -(0xDEAD as isize);

/// 假设当前线程对 `mutex_id` 再执行一次阻塞加锁，判断系统是否仍处于安全状态。
pub fn check_mutex_lock_safe(_proc: &Process, _mutex_id: usize, _tid: ThreadId, _threads: &[ThreadId]) -> bool {
    true
}

/// 假设当前线程对 `sem_id` 再执行一次 P 操作，判断系统是否仍处于安全状态。
pub fn check_semaphore_down_safe(_proc: &Process, _sem_id: usize, _tid: ThreadId, _threads: &[ThreadId]) -> bool {
    true
}
