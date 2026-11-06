//! RAII process-tree lifetime, including future cancellation/drop.
use duga_types::error::ToolError;

pub(crate) fn configure(command: &mut tokio::process::Command) {
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    command.creation_flags(0x00000004); // CREATE_SUSPENDED: assign job before executing user code.
}

pub(crate) struct ProcessTree {
    #[cfg(unix)]
    pid: i32,
    #[cfg(windows)]
    job: windows_sys::Win32::Foundation::HANDLE,
}
// The job HANDLE is owned exclusively by this guard and may be closed on any thread.
#[cfg(windows)]
unsafe impl Send for ProcessTree {}

impl ProcessTree {
    pub(crate) fn attach(child: &tokio::process::Child) -> Result<Self, ToolError> {
        #[cfg(unix)]
        {
            let pid = child
                .id()
                .ok_or_else(|| ToolError::Io("missing process id".into()))?;
            Ok(Self { pid: pid as i32 })
        }
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::{Foundation::CloseHandle, System::JobObjects::*};
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(ToolError::Io(std::io::Error::last_os_error().to_string()));
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let handle = child
                .raw_handle()
                .ok_or_else(|| ToolError::Io("missing process handle".into()))?;
            if SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of_val(&info) as u32,
            ) == 0
                || AssignProcessToJobObject(job, handle as _) == 0
            {
                let error = std::io::Error::last_os_error();
                CloseHandle(job);
                return Err(ToolError::Io(error.to_string()));
            }
            // NtResumeProcess resumes the suspended process without opening its primary thread.
            #[link(name = "ntdll")]
            unsafe extern "system" {
                fn NtResumeProcess(process: windows_sys::Win32::Foundation::HANDLE) -> i32;
            }
            if NtResumeProcess(handle as _) < 0 {
                CloseHandle(job);
                return Err(ToolError::Io("cannot resume job-contained process".into()));
            }
            Ok(Self { job })
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = child;
            Err(ToolError::Denied(
                "process-tree isolation is unavailable on this platform".into(),
            ))
        }
    }
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::kill(-self.pid, libc::SIGKILL);
        }
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.job);
        }
    }
}

/// Tokio detaches tasks when a JoinHandle is dropped; readers must be aborted instead.
pub(crate) struct AbortOnDrop<T>(pub tokio::task::JoinHandle<T>);
impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}
