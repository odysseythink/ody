//! Windows process-tree cleanup via Job Objects (E4). The service process
//! and every descendant it spawns are bound to a job with
//! KILL_ON_JOB_CLOSE: TerminateJobObject kills the whole tree, and even a
//! leaked handle (Runtime crash) is reclaimed by the kernel on close.
//! taskkill /T remains as fallback when job creation/assignment fails.

use std::io;

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
use windows_sys::Win32::System::JobObjects::CreateJobObjectW;
use windows_sys::Win32::System::JobObjects::JobObjectExtendedLimitInformation;
use windows_sys::Win32::System::JobObjects::SetInformationJobObject;
use windows_sys::Win32::System::JobObjects::TerminateJobObject;
use windows_sys::Win32::System::JobObjects::JOBOBJECT_EXTENDED_LIMIT_INFORMATION;
use windows_sys::Win32::System::JobObjects::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
use windows_sys::Win32::System::Threading::OpenProcess;
use windows_sys::Win32::System::Threading::PROCESS_SET_QUOTA;
use windows_sys::Win32::System::Threading::PROCESS_TERMINATE;

pub(crate) struct ServiceJob {
    handle: HANDLE,
}

// HANDLE is a kernel-owned pointer token; the job outlives thread moves.
unsafe impl Send for ServiceJob {}
unsafe impl Sync for ServiceJob {}

impl ServiceJob {
    pub(crate) fn create() -> io::Result<Self> {
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
                BasicLimitInformation: windows_sys::Win32::System::JobObjects::JOBOBJECT_BASIC_LIMIT_INFORMATION {
                    LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                    ..Default::default()
                },
                ..Default::default()
            };
            let ok = SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                let err = io::Error::last_os_error();
                CloseHandle(handle);
                return Err(err);
            }
            Ok(Self { handle })
        }
    }

    /// Bind a (just-spawned) pid. Callers invoke this immediately after
    /// spawn; assignment races are covered by KILL_ON_JOB_CLOSE on drop.
    pub(crate) fn assign_pid(&self, pid: u32) -> io::Result<()> {
        unsafe {
            let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
            if process.is_null() {
                return Err(io::Error::last_os_error());
            }
            let ok = AssignProcessToJobObject(self.handle, process);
            CloseHandle(process);
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    }

    /// Kill the whole tree now (exit code 1). Idempotent enough for our
    /// use: callers invoke once from stop/Drop.
    pub(crate) fn kill(&self) -> io::Result<()> {
        unsafe {
            if TerminateJobObject(self.handle, 1) == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    }
}

impl Drop for ServiceJob {
    fn drop(&mut self) {
        unsafe {
            // KILL_ON_JOB_CLOSE: any surviving tree members die here even
            // if TerminateJobObject was never called (Runtime crash path).
            CloseHandle(self.handle);
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::io;
    use std::process::Command;
    use std::process::Stdio;
    use std::time::Duration;
    use std::time::Instant;

    use windows_sys::Win32::System::Threading::GetExitCodeProcess;
    use windows_sys::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION;
    use windows_sys::Win32::System::Threading::STILL_ACTIVE;

    /// True while the pid refers to a live process (STILL_ACTIVE).
    fn pid_exists(pid: u32) -> bool {
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                return false;
            }
            let mut code: u32 = 0;
            let ok = GetExitCodeProcess(process, &mut code);
            CloseHandle(process);
            ok != 0 && code == STILL_ACTIVE
        }
    }

    fn wait_pid_gone(pid: u32, timeout: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if !pid_exists(pid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        !pid_exists(pid)
    }

    /// Number of live `ping.exe` processes — the grandchild probe: the
    /// fixture trees are built from `ping -t` (present on every Windows).
    fn ping_process_count() -> usize {
        let output = Command::new("tasklist")
            .args(["/FI", "IMAGENAME eq ping.exe", "/NH"])
            .output();
        match output {
            Ok(output) => {
                let text = String::from_utf8_lossy(&output.stdout);
                text.lines()
                    .filter(|line| line.to_lowercase().contains("ping.exe"))
                    .count()
            }
            Err(_) => 0,
        }
    }

    fn spawn_ping_tree() -> io::Result<u32> {
        // cmd spawns a background ping (start /b) then a foreground one:
        // the background ping is the grandchild that a taskkill /T tree
        // snapshot can miss when it spawns inside the race window.
        let child = Command::new("cmd")
            .args(["/c", "start /b ping -t 127.0.0.1 & ping -t 127.0.0.1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(child.id())
    }

    #[test]
    fn job_object_kills_grandchild_tree() {
        let root_pid = spawn_ping_tree().expect("spawn ping tree");
        std::thread::sleep(Duration::from_millis(500)); // let the tree establish
        let job = ServiceJob::create().expect("CreateJobObject");
        job.assign_pid(root_pid).expect("assign root to job");
        job.kill().expect("TerminateJobObject");
        assert!(
            wait_pid_gone(root_pid, Duration::from_secs(5)),
            "root pid {root_pid} survived TerminateJobObject"
        );
        let start = Instant::now();
        while ping_process_count() > 0 && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(
            ping_process_count(),
            0,
            "grandchild ping survived job kill — the exact scenario taskkill /T can miss"
        );
    }

    #[test]
    fn drop_without_kill_leaves_no_survivors_via_kill_on_job_close() {
        let root_pid = spawn_ping_tree().expect("spawn ping tree");
        std::thread::sleep(Duration::from_millis(500));
        {
            let job = ServiceJob::create().expect("CreateJobObject");
            job.assign_pid(root_pid).expect("assign root to job");
            drop(job); // no kill(): the kernel must reap on handle close
        }
        assert!(
            wait_pid_gone(root_pid, Duration::from_secs(5)),
            "pid {root_pid} survived job handle close (KILL_ON_JOB_CLOSE)"
        );
        let start = Instant::now();
        while ping_process_count() > 0 && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(ping_process_count(), 0, "tree survivor after handle close");
    }
}
