//! 平台侧进程存活性探测（轮 41，V7 §7「startup orphan sweep」的探测腿）。
//!
//! 用途只有一个：跨宿主重启后，对持久化回收台账里的 pid 做**只读**定性——
//! `Gone` 用来把"终止失败、进程其实早已退出"的历史条目销账；`Alive` / `Unknown`
//! 只留证据，**绝不据此杀进程**（跨重启无法验明进程身份，盲杀可能命中复用同一
//! 号的新进程，见 `RuntimeTable::sweep_restart_reaps` 的安全封口）。
//!
//! 探测实现留在宿主核心而不是 `tauron-proc`：`tauron-host` 已有平台腿先例
//! （`local_host_reference` 的 `SO_PEERCRED` / named-pipe SID），且本探测**只读**、
//! 不认识进程宿主语义；`tauron-proc` 的终止能力仍由装配层经 `LeaseReaper` 注入。

/// 一次存活性探测的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PidLiveness {
    /// 平台确认该 pid 当前存在（不代表是本宿主的进程，也不代表有权终止它）。
    Alive,
    /// 平台确认该 pid 当前不存在。
    Gone,
    /// 探测不可用或结论不可靠（含 `pid == 0` 的哨兵形态）。
    Unknown,
}

/// 存活性探测能力（装配层注入真实实现；测试注入确定性的替身）。
///
/// 与 `LeaseReaper` 同形：宿主核心不直接调平台 API，由 `crate::liveness::SystemPidProbe`
/// （或测试替身）经 [`crate::runtime::RuntimeTable::set_pid_probe`] 注入。
pub trait PidProbe: Send + Sync {
    /// 探测 `pid` 的存活性。**只读**：实现方不得产生任何副作用。
    fn probe(&self, pid: u32) -> PidLiveness;
}

/// 平台真实探测：`libc::kill(pid, 0)`（Unix）/ `OpenProcess` + `GetExitCodeProcess`
/// （Windows）。不支持的其他平台一律返回 `Unknown`（诚实降级，不猜）。
pub struct SystemPidProbe;

impl PidProbe for SystemPidProbe {
    fn probe(&self, pid: u32) -> PidLiveness {
        system_pid_liveness(pid)
    }
}

/// `pid == 0` 是哨兵而不是普通进程号：Unix 的 `kill(0, 0)` 探的是"当前进程组"，
/// Windows 的 pid 0 打不开——两边语义对"某个插件进程"都无意义，直接 `Unknown`。
pub(crate) fn system_pid_liveness(pid: u32) -> PidLiveness {
    if pid == 0 {
        return PidLiveness::Unknown;
    }
    imp::probe(pid)
}

#[cfg(unix)]
mod imp {
    use super::PidLiveness;

    pub(super) fn probe(pid: u32) -> PidLiveness {
        // SAFETY: signal 0 不投递任何信号，只做存在性/权限检查（POSIX kill 的经典用法）。
        let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if rc == 0 {
            return PidLiveness::Alive;
        }
        match std::io::Error::last_os_error().raw_os_error() {
            Some(libc::ESRCH) => PidLiveness::Gone,
            // EPERM = 进程存在、只是没权限向它发信号。
            Some(libc::EPERM) => PidLiveness::Alive,
            _ => PidLiveness::Unknown,
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::PidLiveness;

    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    pub(super) fn probe(pid: u32) -> PidLiveness {
        // SAFETY: 句柄在函数内闭合；指针参数均为栈上可写位置。
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return match std::io::Error::last_os_error().raw_os_error() {
                    // 无效 pid 参数 = 该 pid 不存在（其余如 ERROR_ACCESS_DENIED = 存在但不可开）。
                    Some(code) if code as u32 == ERROR_INVALID_PARAMETER => PidLiveness::Gone,
                    _ => PidLiveness::Unknown,
                };
            }
            let mut exit_code: u32 = 0;
            let ok = GetExitCodeProcess(handle, &mut exit_code);
            let _ = CloseHandle(handle);
            if ok == 0 {
                return PidLiveness::Unknown;
            }
            if exit_code == STILL_ACTIVE as u32 {
                PidLiveness::Alive
            } else {
                // 有退出码 = 进程已终止但句柄短暂仍可开（退出与回收之间的窗口）。
                PidLiveness::Gone
            }
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use super::PidLiveness;

    pub(super) fn probe(_pid: u32) -> PidLiveness {
        PidLiveness::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_process_probes_alive_and_zero_is_unknown() {
        // 宿主自己的 pid 一定存在。
        assert_eq!(system_pid_liveness(std::process::id()), PidLiveness::Alive);
        // 0 是哨兵语义（Unix 进程组 / Windows 打不开），两种平台都必须是 Unknown。
        assert_eq!(system_pid_liveness(0), PidLiveness::Unknown);
    }

    #[test]
    fn a_pid_above_the_platform_maximum_probes_gone() {
        // pid 上界远低于 u32::MAX（Unix pid_max 默认 4194304；Windows 实际也远小于）。
        // 这个号不是 0（哨兵）且不可能有实进程——平台必须给 Gone，而不是 Unknown。
        assert_eq!(system_pid_liveness(u32::MAX - 1), PidLiveness::Gone);
    }

    #[test]
    fn probe_trait_impl_delegates_to_the_platform() {
        let probe = SystemPidProbe;
        assert_eq!(PidProbe::probe(&probe, std::process::id()), PidLiveness::Alive);
    }
}
