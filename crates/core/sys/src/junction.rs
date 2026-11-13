use std::{
    fs::{self, File, OpenOptions},
    io, mem,
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle},
    path::Path,
    ptr,
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, ERROR_NOT_ALL_ASSIGNED, FALSE, GENERIC_READ, GENERIC_WRITE, GetLastError,
        HANDLE, LUID,
    },
    Security::{
        AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW,
        SE_CREATE_SYMBOLIC_LINK_NAME, SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES,
        TOKEN_PRIVILEGES,
    },
    Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, GetFullPathNameW,
        MAXIMUM_REPARSE_DATA_BUFFER_SIZE,
    },
    System::{
        IO::DeviceIoControl,
        SystemServices::IO_REPARSE_TAG_MOUNT_POINT,
        Threading::{GetCurrentProcess, OpenProcessToken},
    },
};

const FSCTL_SET_REPARSE_POINT: u32 = 0x0009_00A4;
const NT_PREFIX: [u16; 4] = [b'\\' as u16, b'?' as u16, b'?' as u16, b'\\' as u16];
const VERBATIM_PREFIX: [u16; 4] = [b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];
const UNIT_BYTES: usize = mem::size_of::<u16>();
const REPARSE_DATA_HEADER_BYTES: usize = 8;
const MOUNT_POINT_HEADER_BYTES: usize = 8;
const FAILED: i32 = 0;

pub fn create(target: &Path, junction: &Path) -> io::Result<()> {
    let target = full_path(target)?;
    let buffer = mount_point_buffer(&target)?;
    fs::create_dir(junction)?;
    let file = open_for_writing(junction)?;
    let length = u32::try_from(buffer.len()).expect("a reparse buffer fits u32");
    let mut returned: u32 = 0;
    let set = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            FSCTL_SET_REPARSE_POINT,
            buffer.as_ptr().cast(),
            length,
            ptr::null_mut(),
            0,
            &mut returned,
            ptr::null_mut(),
        )
    };
    if set == FAILED {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn mount_point_buffer(full_target: &[u16]) -> io::Result<Vec<u8>> {
    let target = full_target
        .strip_prefix(VERBATIM_PREFIX.as_slice())
        .unwrap_or(full_target);
    let substitute_bytes = (NT_PREFIX.len() + target.len()) * UNIT_BYTES;
    let print_bytes = target.len() * UNIT_BYTES;
    let names_bytes = substitute_bytes + UNIT_BYTES + print_bytes + UNIT_BYTES;
    let path_bytes_max = usize::try_from(MAXIMUM_REPARSE_DATA_BUFFER_SIZE)
        .expect("a u32 size fits usize")
        - REPARSE_DATA_HEADER_BYTES
        - MOUNT_POINT_HEADER_BYTES;
    if names_bytes > path_bytes_max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "`target` is too long",
        ));
    }
    let field =
        |bytes: usize| u16::try_from(bytes).expect("a name within the reparse limit fits u16");
    let data_bytes = MOUNT_POINT_HEADER_BYTES + names_bytes;
    let mut buffer = Vec::with_capacity(REPARSE_DATA_HEADER_BYTES + data_bytes);
    buffer.extend_from_slice(&IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
    buffer.extend_from_slice(&field(data_bytes).to_le_bytes());
    buffer.extend_from_slice(&0_u16.to_le_bytes());
    buffer.extend_from_slice(&0_u16.to_le_bytes());
    buffer.extend_from_slice(&field(substitute_bytes).to_le_bytes());
    buffer.extend_from_slice(&field(substitute_bytes + UNIT_BYTES).to_le_bytes());
    buffer.extend_from_slice(&field(print_bytes).to_le_bytes());
    let names = NT_PREFIX
        .iter()
        .chain(target)
        .chain(&[0])
        .chain(target)
        .chain(&[0]);
    for unit in names {
        buffer.extend_from_slice(&unit.to_le_bytes());
    }
    assert_eq!(buffer.len(), REPARSE_DATA_HEADER_BYTES + data_bytes);
    Ok(buffer)
}

fn full_path(path: &Path) -> io::Result<Vec<u16>> {
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a junction target cannot contain a NUL",
        ));
    }
    wide.push(0);
    let needed = unsafe { GetFullPathNameW(wide.as_ptr(), 0, ptr::null_mut(), ptr::null_mut()) };
    if needed == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut full = vec![0_u16; usize::try_from(needed).expect("a u32 length fits usize")];
    let written =
        unsafe { GetFullPathNameW(wide.as_ptr(), needed, full.as_mut_ptr(), ptr::null_mut()) };
    if written == 0 {
        return Err(io::Error::last_os_error());
    }
    if written >= needed {
        return Err(io::Error::other(
            "the current directory changed while the junction target was resolved",
        ));
    }
    full.truncate(usize::try_from(written).expect("a u32 length fits usize"));
    Ok(full)
}

fn open_for_writing(junction: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options
        .access_mode(GENERIC_READ | GENERIC_WRITE)
        .share_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS);
    match options.open(junction) {
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            enable_symbolic_link_privilege()?;
            options.open(junction)
        }
        opened => opened,
    }
}

struct Token(HANDLE);

impl Drop for Token {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

fn enable_symbolic_link_privilege() -> io::Result<()> {
    let mut handle: HANDLE = ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES, &mut handle) }
        == FAILED
    {
        return Err(io::Error::last_os_error());
    }
    let token = Token(handle);
    let mut luid = LUID {
        LowPart: 0,
        HighPart: 0,
    };
    if unsafe { LookupPrivilegeValueW(ptr::null(), SE_CREATE_SYMBOLIC_LINK_NAME, &mut luid) }
        == FAILED
    {
        return Err(io::Error::last_os_error());
    }
    let privileges = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [LUID_AND_ATTRIBUTES {
            Luid: luid,
            Attributes: SE_PRIVILEGE_ENABLED,
        }],
    };
    let size =
        u32::try_from(mem::size_of::<TOKEN_PRIVILEGES>()).expect("TOKEN_PRIVILEGES fits u32");
    let adjusted = unsafe {
        AdjustTokenPrivileges(
            token.0,
            FALSE,
            &privileges,
            size,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    if adjusted == FAILED {
        return Err(io::Error::last_os_error());
    }
    let status = unsafe { GetLastError() };
    if status == ERROR_NOT_ALL_ASSIGNED {
        return Err(io::Error::from_raw_os_error(
            i32::try_from(status).expect("a Win32 error code fits i32"),
        ));
    }
    Ok(())
}
