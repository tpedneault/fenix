//! The control endpoint on Windows: a per-user, per-session named pipe.
//! See the parent module for why it is shaped this way.

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::ptr;

use windows_sys::Win32::Foundation::{
    CloseHandle, LocalFree, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY,
    ERROR_PIPE_CONNECTED, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SDDL_REVISION_1, SE_KERNEL_OBJECT,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenUser, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FlushFileBuffers, FILE_FLAG_FIRST_PIPE_INSTANCE, OPEN_EXISTING,
    PIPE_ACCESS_DUPLEX, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeServerProcessId, WaitNamedPipeW,
    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES,
    PIPE_WAIT,
};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

use super::{BindError, Endpoint};

const BUFFER: u32 = 64 * 1024;

/// The server's end: hands out one [`Stream`] per client.
pub struct Listener {
    name: Vec<u16>,
    security: Security,
    /// The instance the next client will connect to. Created ahead, so a
    /// client arriving between two `accept`s finds one waiting.
    pending: HANDLE,
}

// The handle is only used from whichever thread owns the listener.
unsafe impl Send for Listener {}

/// One connection, from either end.
pub struct Stream {
    file: File,
}

impl Listener {
    pub(super) fn bind(endpoint: &Endpoint) -> Result<Listener, BindError> {
        let user = UserIdentity::current()?;
        let name = wide(&pipe_name(endpoint, &user));
        // Full access for this user's SID and nobody else; `P` stops
        // inherited entries from widening it.
        let security = Security::from_sddl(&format!("D:P(A;;GA;;;{})", user.sid))?;
        let pending = match create_instance(&name, &security, true) {
            Ok(handle) => handle,
            // `FILE_FLAG_FIRST_PIPE_INSTANCE` fails with access denied
            // when an instance of the name exists: another Fenix, or
            // something squatting on the name, which we refuse to share.
            Err(e) if e.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) => {
                return Err(BindError::InUse)
            }
            Err(e) => return Err(BindError::Io(e)),
        };
        Ok(Listener {
            name,
            security,
            pending,
        })
    }

    /// Waits for the next client.
    pub fn accept(&mut self) -> io::Result<Stream> {
        if self.pending == INVALID_HANDLE_VALUE {
            self.pending = create_instance(&self.name, &self.security, false)?;
        }
        let handle = self.pending;
        // SAFETY: `handle` is a pipe instance this listener created and
        // still owns; a null OVERLAPPED makes this a blocking wait.
        let connected = unsafe { ConnectNamedPipe(handle, ptr::null_mut()) } != 0;
        if !connected {
            let error = io::Error::last_os_error();
            // A client that connected between creation and this call is
            // connected all the same.
            if error.raw_os_error() != Some(ERROR_PIPE_CONNECTED as i32) {
                return Err(error);
            }
        }
        self.pending = INVALID_HANDLE_VALUE;
        // SAFETY: ownership of the connected instance moves to the File.
        let file = unsafe { File::from_raw_handle(handle as _) };
        // Have the next instance ready before this client is served, so a
        // second client never finds the name missing and concludes there
        // is no Fenix.
        self.pending =
            create_instance(&self.name, &self.security, false).unwrap_or(INVALID_HANDLE_VALUE);
        Ok(Stream { file })
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        if self.pending != INVALID_HANDLE_VALUE {
            // SAFETY: an instance this listener created and never handed out.
            unsafe { CloseHandle(self.pending) };
        }
    }
}

impl Stream {
    pub(super) fn connect(endpoint: &Endpoint) -> io::Result<Option<Stream>> {
        let user = UserIdentity::current()?;
        let name = wide(&pipe_name(endpoint, &user));
        // A busy pipe means every instance is taken for a moment; the
        // server creates the next one right after accepting.
        for _ in 0..20 {
            // SAFETY: `name` is NUL-terminated. Identification-level QoS
            // stops the server from acting as us beyond knowing who we are.
            let handle = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    ptr::null(),
                    OPEN_EXISTING,
                    SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                    ptr::null_mut(),
                )
            };
            if handle != INVALID_HANDLE_VALUE {
                // SAFETY: a handle we just opened and own.
                let file = unsafe { File::from_raw_handle(handle as _) };
                let stream = Stream { file };
                stream.check_server_is(&user.sid)?;
                return Ok(Some(stream));
            }
            let error = io::Error::last_os_error();
            match error.raw_os_error().map(|c| c as u32) {
                Some(ERROR_FILE_NOT_FOUND) => return Ok(None),
                Some(ERROR_PIPE_BUSY) => {
                    // SAFETY: `name` is NUL-terminated.
                    unsafe { WaitNamedPipeW(name.as_ptr(), 250) };
                }
                _ => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Fenix's control pipe stayed busy",
        ))
    }

    /// Refuses a pipe someone else made. Pipe names are machine-wide, so
    /// another user could create ours before our Fenix starts and read
    /// whatever we send; the DACL protects the server, this protects the
    /// client. The server process's user is checked first; when its
    /// token can't be read (an elevated Fenix, from an ordinary
    /// launch), the pipe's owner is.
    fn check_server_is(&self, sid: &str) -> io::Result<()> {
        let handle = self.file.as_raw_handle() as HANDLE;
        let server_user = server_process_user(handle);
        let owner = || pipe_owner(handle);
        if server_user.as_deref().ok() == Some(sid) || owner().as_deref().ok() == Some(sid) {
            return Ok(());
        }
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Fenix's control pipe belongs to another user",
        ))
    }

    /// A second handle to the same connection, for reading on one side
    /// and writing on the other.
    pub fn try_clone(&self) -> io::Result<Stream> {
        Ok(Stream {
            file: self.file.try_clone()?,
        })
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // `std` already reads a broken pipe as end of file.
        self.file.read(buf)
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file.write(buf)
    }

    /// Waits until the other side has read everything written. Without
    /// it, a connection closed right after answering could take the
    /// answer with it; `File::flush` is a no-op on Windows.
    fn flush(&mut self) -> io::Result<()> {
        // SAFETY: the handle is owned by `self.file` and open.
        if unsafe { FlushFileBuffers(self.file.as_raw_handle() as HANDLE) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

fn create_instance(name: &[u16], security: &Security, first: bool) -> io::Result<HANDLE> {
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: security.0,
        bInheritHandle: 0,
    };
    let open_mode = PIPE_ACCESS_DUPLEX
        | if first {
            FILE_FLAG_FIRST_PIPE_INSTANCE
        } else {
            0
        };
    // SAFETY: `name` is NUL-terminated and `attributes` outlives the call.
    let handle = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            BUFFER,
            BUFFER,
            0,
            &attributes,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(handle)
}

pub(super) fn address(endpoint: &Endpoint) -> io::Result<String> {
    Ok(pipe_name(endpoint, &UserIdentity::current()?))
}

/// `\\.\pipe\fenix-<SID>-<session>[-<instance>]`.
fn pipe_name(endpoint: &Endpoint, user: &UserIdentity) -> String {
    format!(
        r"\\.\pipe\fenix-{}-{}{}",
        user.sid,
        user.session,
        endpoint.suffix()
    )
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// Who we are: the user's SID as a string, and the login session.
struct UserIdentity {
    sid: String,
    session: u32,
}

impl UserIdentity {
    fn current() -> io::Result<Self> {
        let mut session = 0;
        // SAFETY: plain out-parameter call.
        if unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the pseudo-handle for this process needs no closing.
        Ok(UserIdentity {
            sid: process_user(unsafe { GetCurrentProcess() })?,
            session,
        })
    }
}

/// The user a process runs as, as a string SID.
fn process_user(process: HANDLE) -> io::Result<String> {
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: `process` is a process handle with query access.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = OwnedHandle(token);
    let mut needed = 0;
    // SAFETY: a size query; failing with "insufficient buffer" is expected.
    unsafe { GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut needed) };
    if needed == 0 {
        return Err(io::Error::last_os_error());
    }
    // `u64`s keep the buffer aligned for `TOKEN_USER`.
    let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
    // SAFETY: `buffer` has at least `needed` bytes.
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call filled `buffer` with a TOKEN_USER, whose SID
    // points into `buffer`, alive for this call.
    sid_string(unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid })
}

/// The user of the process at the server end of a connected pipe.
fn server_process_user(pipe: HANDLE) -> io::Result<String> {
    let mut pid = 0;
    // SAFETY: `pipe` is a connected client handle; plain out-parameter.
    if unsafe { GetNamedPipeServerProcessId(pipe, &mut pid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: plain call; the handle is closed by `OwnedHandle`.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return Err(io::Error::last_os_error());
    }
    let process = OwnedHandle(process);
    process_user(process.0)
}

/// The owner of a kernel object (here, a pipe), as a string SID.
fn pipe_owner(handle: HANDLE) -> io::Result<String> {
    let mut owner: PSID = ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: `handle` is open with READ_CONTROL (part of GENERIC_READ);
    // `owner` points into `descriptor`, freed below.
    let status = unsafe {
        GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let sid = sid_string(owner);
    // SAFETY: allocated by GetSecurityInfo.
    unsafe { LocalFree(descriptor) };
    sid
}

fn sid_string(sid: PSID) -> io::Result<String> {
    let mut text: *mut u16 = ptr::null_mut();
    // SAFETY: `sid` is a valid SID for the duration of the call.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `text` is a NUL-terminated string the call allocated with
    // LocalAlloc; it is read once and freed.
    Ok(unsafe { take_local_string(text) })
}

/// Copies and frees a NUL-terminated wide string allocated with
/// `LocalAlloc`.
///
/// # Safety
/// `text` must be such a string, not used afterwards.
unsafe fn take_local_string(text: *mut u16) -> String {
    // SAFETY: per this function's contract.
    unsafe {
        let len = (0..).take_while(|&i| *text.add(i) != 0).count();
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
        LocalFree(text.cast());
        s
    }
}

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: a handle we opened.
        unsafe { CloseHandle(self.0) };
    }
}

/// A security descriptor from SDDL, freed on drop.
struct Security(PSECURITY_DESCRIPTOR);

impl Security {
    fn from_sddl(sddl: &str) -> io::Result<Self> {
        let sddl = wide(sddl);
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        // SAFETY: `sddl` is NUL-terminated; the descriptor is allocated by
        // the call and freed in `drop`.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Security(descriptor))
    }
}

impl Drop for Security {
    fn drop(&mut self) {
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
        unsafe { LocalFree(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::unique;
    use super::*;
    use std::ffi::OsStr;
    use windows_sys::Win32::Security::Authorization::ConvertSecurityDescriptorToStringSecurityDescriptorW;
    use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;

    #[test]
    fn the_name_holds_the_user_the_session_and_the_home() {
        let user = UserIdentity::current().unwrap();
        assert!(user.sid.starts_with("S-1-"), "{}", user.sid);
        let own = pipe_name(&Endpoint::for_home(None), &user);
        assert_eq!(
            own,
            format!(r"\\.\pipe\fenix-{}-{}", user.sid, user.session)
        );
        let home = pipe_name(&Endpoint::for_home(Some(OsStr::new("foobar"))), &user);
        assert_eq!(
            home,
            format!(
                r"\\.\pipe\fenix-{}-{}-85944171f73967e8",
                user.sid, user.session
            )
        );
        assert_eq!(Endpoint::for_home(None).address().unwrap(), own);
    }

    #[test]
    fn the_session_is_this_processs_terminal_services_session() {
        let mut session = u32::MAX;
        // SAFETY: plain out-parameter call.
        assert_ne!(
            unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) },
            0
        );
        assert_eq!(UserIdentity::current().unwrap().session, session);
    }

    /// The pipe's DACL, read back from the object Windows created.
    fn dacl_of(handle: HANDLE) -> String {
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        // SAFETY: `handle` is a pipe instance we own; the descriptor is freed below.
        let status = unsafe {
            GetSecurityInfo(
                handle,
                SE_KERNEL_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut descriptor,
            )
        };
        assert_eq!(status, 0);
        let mut text: *mut u16 = ptr::null_mut();
        // SAFETY: `descriptor` is valid; `text` is taken and freed below.
        let ok = unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor,
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &mut text,
                ptr::null_mut(),
            )
        };
        assert_ne!(ok, 0);
        // SAFETY: allocated by the calls above.
        unsafe {
            LocalFree(descriptor);
            take_local_string(text)
        }
    }

    #[test]
    fn only_this_user_may_open_the_pipe() {
        let listener = unique().bind().unwrap();
        let sid = UserIdentity::current().unwrap().sid;
        let dacl = dacl_of(listener.pending);
        // Protected (nothing inherited) and a single allow entry, for us.
        // Windows reports GENERIC_ALL mapped to the pipe's own rights.
        assert!(dacl.starts_with("D:P("), "{dacl}");
        assert_eq!(dacl.matches('(').count(), 1, "{dacl}");
        assert!(
            dacl.starts_with("D:P(A;;") && dacl.ends_with(&format!(";;;{sid})")),
            "{dacl}"
        );
    }

    #[test]
    fn a_name_someone_else_created_first_is_not_shared() {
        // Squatting on the name with a pipe open to everyone: Fenix must
        // not add its own instance to it and receive clients beside it.
        let endpoint = unique();
        let user = UserIdentity::current().unwrap();
        let name = wide(&pipe_name(&endpoint, &user));
        let everyone = Security::from_sddl("D:(A;;GA;;;WD)").unwrap();
        let squatter = create_instance(&name, &everyone, false).unwrap();
        assert!(matches!(endpoint.bind(), Err(BindError::InUse)));
        // SAFETY: the instance created above.
        unsafe { CloseHandle(squatter) };
    }

    #[test]
    fn a_client_checks_who_is_serving() {
        let endpoint = unique();
        let _listener = endpoint.bind().unwrap();
        let stream = endpoint.connect().unwrap().expect("listening");
        let sid = UserIdentity::current().unwrap().sid;
        let handle = stream.file.as_raw_handle() as HANDLE;
        assert_eq!(server_process_user(handle).unwrap(), sid);
        assert!(stream.check_server_is(&sid).is_ok());
        assert!(stream.check_server_is("S-1-5-21-1-2-3-500").is_err());
    }
}
