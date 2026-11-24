use std::io;

#[cfg(unix)]
pub(crate) fn spawn_service() -> io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    Command::new(std::env::current_exe()?)
        .arg("--serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
}

#[cfg(windows)]
pub(crate) use win::spawn_service;

#[cfg(windows)]
mod win {
    use super::io;
    use std::os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    };
    use std::ptr::null;
    use windows_sys::Win32::{
        Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::Threading::{
            CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, CreateProcessW, GetExitCodeProcess,
            PROCESS_INFORMATION, STARTUPINFOW, TerminateProcess, WaitForSingleObject,
        },
    };

    pub(crate) struct ServiceProcess(OwnedHandle);

    impl ServiceProcess {
        pub(crate) fn kill(&mut self) -> io::Result<()> {
            let handle = self.0.as_raw_handle();
            if unsafe { TerminateProcess(handle, 1) } == 0 && self.try_wait()?.is_none() {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }

        pub(crate) fn try_wait(&self) -> io::Result<Option<u32>> {
            let process = self.0.as_raw_handle();
            match unsafe { WaitForSingleObject(process, 0) } {
                WAIT_TIMEOUT => Ok(None),
                WAIT_OBJECT_0 => {
                    let mut code = 0;
                    if unsafe { GetExitCodeProcess(process, &mut code) } == 0 {
                        Err(io::Error::last_os_error())
                    } else {
                        Ok(Some(code))
                    }
                }
                _ => Err(io::Error::last_os_error()),
            }
        }
    }

    pub(crate) fn spawn_service() -> io::Result<ServiceProcess> {
        let exe: Vec<u16> = std::env::current_exe()?
            .as_os_str()
            .encode_wide()
            .chain([0])
            .collect();
        let mut args: Vec<u16> = "bastion-bridge-gpui --serve\0".encode_utf16().collect();
        let startup = STARTUPINFOW {
            cb: size_of::<STARTUPINFOW>() as u32,
            ..Default::default()
        };
        let mut process = PROCESS_INFORMATION::default();
        // Inherit the environment (including the per-user data directory), but no handles.
        // An inherited client pipe, even an unused one, prevents the MCP host from seeing EOF.
        let ok = unsafe {
            CreateProcessW(
                exe.as_ptr(),
                args.as_mut_ptr(),
                null(),
                null(),
                0,
                CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW,
                null(),
                null(),
                &startup,
                &mut process,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        // CreateProcessW transfers ownership of both handles. Only the process is needed
        // to detect startup failure. Dropping these handles does not terminate the service.
        unsafe {
            drop(OwnedHandle::from_raw_handle(process.hThread));
            Ok(ServiceProcess(OwnedHandle::from_raw_handle(
                process.hProcess,
            )))
        }
    }
}
