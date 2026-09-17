//! 单实例守卫（takeover 语义）：同一 ODY_HOME 只允许一个 app-server 存活。
//!
//! 背景（odyBox 工程模式验收实证，2026-09-17）：dev 热重启或异常残留会
//! 让多个 app-server 进程持有同一批 JSON store（workspace-project /
//! workspace-source）的内存快照，而它们的 persist 都是「全量重写」——
//! 后写者覆盖先写者，表现为绑定记录、暂存 changeset 随机「丢失」
//! （v1.json 在两次读取之间整体换人）。守卫在启动早期对
//! `$ODY_HOME/app-server.lock` 加 advisory flock：
//!
//! 1. 加锁成功 → 写 PID 文件，继续启动；
//! 2. 失败 → 读 PID 文件，向旧进程发 SIGTERM，轮询等待其释放（≤2s，
//!    dev 热重启的常规路径，旧进程收到 TERM 即退出、锁自动释放）；
//! 3. 仍失败 → 明确报错退出——宁可启动失败，也不两个进程静默互写。
//!
//! 非 Unix 平台没有 flock，退化为「只写 PID 文件、跳过强制互斥」
//! （odyBox 桌面端交付目标为 macOS/Unix；Windows 下依赖启动器保证单例）。

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub(crate) struct SingleInstanceGuard {
    // 持有锁的 fd：进程退出（含被 TERM）时内核自动释放。
    _lock_file: File,
    pid_path: PathBuf,
}

const LOCK_FILE_NAME: &str = "app-server.lock";
const PID_FILE_NAME: &str = "app-server.pid";
const TAKEOVER_TIMEOUT: Duration = Duration::from_secs(2);
const TAKEOVER_POLL_INTERVAL: Duration = Duration::from_millis(100);

impl SingleInstanceGuard {
    pub(crate) fn acquire(ody_home: &Path) -> io::Result<Self> {
        fs::create_dir_all(ody_home)?;
        let lock_path = ody_home.join(LOCK_FILE_NAME);
        let pid_path = ody_home.join(PID_FILE_NAME);
        let lock_file = OpenOptions::new().create(true).append(true).open(&lock_path)?;
        if try_lock_exclusive(&lock_file) {
            write_pid(&pid_path)?;
            return Ok(Self {
                _lock_file: lock_file,
                pid_path,
            });
        }

        // 已被占用：尝试 takeover——终止旧持有者后抢锁。
        terminate_previous_holder(&pid_path);
        let deadline = Instant::now() + TAKEOVER_TIMEOUT;
        loop {
            if try_lock_exclusive(&lock_file) {
                write_pid(&pid_path)?;
                return Ok(Self {
                    _lock_file: lock_file,
                    pid_path,
                });
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!(
                        "another app-server process holds {} (see PID file {}); \
                         refused to start a second instance because both would \
                         silently overwrite each other's workspace stores",
                        lock_path.display(),
                        pid_path.display()
                    ),
                ));
            }
            std::thread::sleep(TAKEOVER_POLL_INTERVAL);
        }
    }
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        // 尽力清理；失败无碍——锁释放后 PID 文件只是残留信息。
        let _ = fs::remove_file(&self.pid_path);
    }
}

fn write_pid(pid_path: &Path) -> io::Result<()> {
    let mut file = File::create(pid_path)?;
    writeln!(file, "{}", std::process::id())
}

/// 向 PID 文件指向的旧进程发 SIGTERM。跳过非法 PID 与自身（同进程内
/// 测试/重复初始化场景防自杀）。
#[cfg(unix)]
fn terminate_previous_holder(pid_path: &Path) {
    let Ok(raw) = fs::read_to_string(pid_path) else {
        return;
    };
    let Ok(pid) = raw.trim().parse::<libc::pid_t>() else {
        return;
    };
    if pid <= 1 || pid as u32 == std::process::id() {
        return;
    }
    // 旧进程没有任何 graceful-shutdown 义务：直接 TERM，锁随进程退出释放。
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
}

#[cfg(not(unix))]
fn terminate_previous_holder(_pid_path: &Path) {}

#[cfg(unix)]
fn try_lock_exclusive(file: &File) -> bool {
    use std::os::unix::io::AsRawFd;
    // advisory flock：同一进程内不同 fd 也会互斥，适合跨进程判定。
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
}

#[cfg(not(unix))]
fn try_lock_exclusive(_file: &File) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::io::AsRawFd;

    #[test]
    fn acquire_writes_pid_file_and_holds_lock() {
        let home = tempfile::tempdir().expect("temp ody home");
        let guard = SingleInstanceGuard::acquire(home.path()).expect("acquire");
        let raw = fs::read_to_string(home.path().join(PID_FILE_NAME)).expect("pid file");
        assert_eq!(raw.trim(), std::process::id().to_string());
        drop(guard);
        assert!(!home.path().join(PID_FILE_NAME).exists());
    }

    #[test]
    fn lock_is_actually_exclusive_across_fds() {
        let home = tempfile::tempdir().expect("temp ody home");
        let _guard = SingleInstanceGuard::acquire(home.path()).expect("acquire");
        // 模拟第二个进程：独立 fd 打开同一锁文件，非阻塞加锁必须失败。
        let second = OpenOptions::new()
            .append(true)
            .open(home.path().join(LOCK_FILE_NAME))
            .expect("open second fd");
        let rc = unsafe { libc::flock(second.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        assert_ne!(rc, 0, "guard must hold an exclusive flock");
    }

    #[test]
    fn terminate_skips_own_pid_and_garbage() {
        let home = tempfile::tempdir().expect("temp ody home");
        let pid_path = home.path().join(PID_FILE_NAME);
        // 自身 PID：不得发信号（防自杀）。
        write_pid(&pid_path).expect("write own pid");
        terminate_previous_holder(&pid_path);
        // 垃圾内容：静默忽略。
        fs::write(&pid_path, "not-a-pid").expect("write garbage");
        terminate_previous_holder(&pid_path);
    }
}
