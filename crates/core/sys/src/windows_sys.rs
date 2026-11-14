pub mod Win32 {
    pub mod Foundation {
        pub type DUPLICATE_HANDLE_OPTIONS = u32;
        pub type FARPROC = Option<unsafe extern "system" fn() -> isize>;
        #[derive(Clone, Copy, Default)]
        #[repr(C)]
        pub struct FILETIME {
            pub dwLowDateTime: u32,
            pub dwHighDateTime: u32,
        }
        pub type GENERIC_ACCESS_RIGHTS = u32;
        pub type HANDLE = *mut core::ffi::c_void;
        pub type HANDLE_FLAGS = u32;
        pub type HINSTANCE = *mut core::ffi::c_void;
        pub type HLOCAL = *mut core::ffi::c_void;
        pub type HMODULE = *mut core::ffi::c_void;
        pub type HWND = *mut core::ffi::c_void;
        pub type LPARAM = isize;
        #[derive(Clone, Copy, Default)]
        #[repr(C)]
        pub struct LUID {
            pub LowPart: u32,
            pub HighPart: i32,
        }
        pub type NTSTATUS = i32;
        #[derive(Clone, Copy, Default)]
        #[repr(C)]
        pub struct SYSTEMTIME {
            pub wYear: u16,
            pub wMonth: u16,
            pub wDayOfWeek: u16,
            pub wDay: u16,
            pub wHour: u16,
            pub wMinute: u16,
            pub wSecond: u16,
            pub wMilliseconds: u16,
        }
        #[derive(Clone, Copy)]
        #[repr(C)]
        pub struct UNICODE_STRING {
            pub Length: u16,
            pub MaximumLength: u16,
            pub Buffer: crate::windows_sys::core::PWSTR,
        }
        impl Default for UNICODE_STRING {
            fn default() -> Self {
                unsafe { core::mem::zeroed() }
            }
        }
        pub type WAIT_EVENT = u32;
        pub type WIN32_ERROR = u32;
        pub const CERT_E_CN_NO_MATCH: crate::windows_sys::core::HRESULT = 0x800B010F_u32 as _;
        pub const CERT_E_EXPIRED: crate::windows_sys::core::HRESULT = 0x800B0101_u32 as _;
        pub const CERT_E_INVALID_NAME: crate::windows_sys::core::HRESULT = 0x800B0114_u32 as _;
        pub const CERT_E_UNTRUSTEDROOT: crate::windows_sys::core::HRESULT = 0x800B0109_u32 as _;
        pub const CERT_E_WRONG_USAGE: crate::windows_sys::core::HRESULT = 0x800B0110_u32 as _;
        pub const CRYPT_E_NOT_FOUND: crate::windows_sys::core::HRESULT = 0x80092004_u32 as _;
        pub const CRYPT_E_REVOKED: crate::windows_sys::core::HRESULT = 0x80092010_u32 as _;
        pub const DUPLICATE_CLOSE_SOURCE: DUPLICATE_HANDLE_OPTIONS = 1u32;
        pub const DUPLICATE_SAME_ACCESS: DUPLICATE_HANDLE_OPTIONS = 2u32;
        pub const ERROR_ABANDONED_WAIT_0: WIN32_ERROR = 735u32;
        pub const ERROR_ACCESS_DENIED: WIN32_ERROR = 5u32;
        pub const ERROR_ALREADY_EXISTS: WIN32_ERROR = 183u32;
        pub const ERROR_ARENA_TRASHED: WIN32_ERROR = 7u32;
        pub const ERROR_AUTODATASEG_EXCEEDS_64k: WIN32_ERROR = 199u32;
        pub const ERROR_BAD_COMMAND: WIN32_ERROR = 22u32;
        pub const ERROR_BAD_ENVIRONMENT: WIN32_ERROR = 10u32;
        pub const ERROR_BAD_EXE_FORMAT: WIN32_ERROR = 193u32;
        pub const ERROR_BAD_FORMAT: WIN32_ERROR = 11u32;
        pub const ERROR_BAD_LENGTH: WIN32_ERROR = 24u32;
        pub const ERROR_BAD_NETPATH: WIN32_ERROR = 53u32;
        pub const ERROR_BAD_NET_NAME: WIN32_ERROR = 67u32;
        pub const ERROR_BAD_PATHNAME: WIN32_ERROR = 161u32;
        pub const ERROR_BAD_UNIT: WIN32_ERROR = 20u32;
        pub const ERROR_BROKEN_PIPE: WIN32_ERROR = 109u32;
        pub const ERROR_CANNOT_MAKE: WIN32_ERROR = 82u32;
        pub const ERROR_CANT_ACCESS_FILE: WIN32_ERROR = 1920u32;
        pub const ERROR_CHILD_NOT_COMPLETE: WIN32_ERROR = 129u32;
        pub const ERROR_CONNECTION_ABORTED: WIN32_ERROR = 1236u32;
        pub const ERROR_CONNECTION_REFUSED: WIN32_ERROR = 1225u32;
        pub const ERROR_CONTROL_C_EXIT: WIN32_ERROR = 572u32;
        pub const ERROR_CRC: WIN32_ERROR = 23u32;
        pub const ERROR_CURRENT_DIRECTORY: WIN32_ERROR = 16u32;
        pub const ERROR_DIRECTORY: WIN32_ERROR = 267u32;
        pub const ERROR_DIRECT_ACCESS_HANDLE: WIN32_ERROR = 130u32;
        pub const ERROR_DIR_NOT_EMPTY: WIN32_ERROR = 145u32;
        pub const ERROR_DISK_FULL: WIN32_ERROR = 112u32;
        pub const ERROR_DRIVE_LOCKED: WIN32_ERROR = 108u32;
        pub const ERROR_DYNLINK_FROM_INVALID_RING: WIN32_ERROR = 196u32;
        pub const ERROR_EXE_MARKED_INVALID: WIN32_ERROR = 192u32;
        pub const ERROR_FAIL_I24: WIN32_ERROR = 83u32;
        pub const ERROR_FILENAME_EXCED_RANGE: WIN32_ERROR = 206u32;
        pub const ERROR_FILE_EXISTS: WIN32_ERROR = 80u32;
        pub const ERROR_FILE_NOT_FOUND: WIN32_ERROR = 2u32;
        pub const ERROR_GEN_FAILURE: WIN32_ERROR = 31u32;
        pub const ERROR_HANDLE_EOF: WIN32_ERROR = 38u32;
        pub const ERROR_INFLOOP_IN_RELOC_CHAIN: WIN32_ERROR = 202u32;
        pub const ERROR_INSUFFICIENT_BUFFER: WIN32_ERROR = 122u32;
        pub const ERROR_INVALID_ACCESS: WIN32_ERROR = 12u32;
        pub const ERROR_INVALID_BLOCK: WIN32_ERROR = 9u32;
        pub const ERROR_INVALID_DATA: WIN32_ERROR = 13u32;
        pub const ERROR_INVALID_DRIVE: WIN32_ERROR = 15u32;
        pub const ERROR_INVALID_EXE_SIGNATURE: WIN32_ERROR = 191u32;
        pub const ERROR_INVALID_FLAGS: WIN32_ERROR = 1004u32;
        pub const ERROR_INVALID_FUNCTION: WIN32_ERROR = 1u32;
        pub const ERROR_INVALID_HANDLE: WIN32_ERROR = 6u32;
        pub const ERROR_INVALID_MINALLOCSIZE: WIN32_ERROR = 195u32;
        pub const ERROR_INVALID_MODULETYPE: WIN32_ERROR = 190u32;
        pub const ERROR_INVALID_NAME: WIN32_ERROR = 123u32;
        pub const ERROR_INVALID_PARAMETER: WIN32_ERROR = 87u32;
        pub const ERROR_INVALID_SEGDPL: WIN32_ERROR = 198u32;
        pub const ERROR_INVALID_STACKSEG: WIN32_ERROR = 189u32;
        pub const ERROR_INVALID_STARTING_CODESEG: WIN32_ERROR = 188u32;
        pub const ERROR_INVALID_TARGET_HANDLE: WIN32_ERROR = 114u32;
        pub const ERROR_IOPL_NOT_ENABLED: WIN32_ERROR = 197u32;
        pub const ERROR_IO_INCOMPLETE: WIN32_ERROR = 996u32;
        pub const ERROR_IO_PENDING: WIN32_ERROR = 997u32;
        pub const ERROR_ITERATED_DATA_EXCEEDS_64k: WIN32_ERROR = 194u32;
        pub const ERROR_LOCK_FAILED: WIN32_ERROR = 167u32;
        pub const ERROR_LOCK_VIOLATION: WIN32_ERROR = 33u32;
        pub const ERROR_MAX_THRDS_REACHED: WIN32_ERROR = 164u32;
        pub const ERROR_MORE_DATA: WIN32_ERROR = 234u32;
        pub const ERROR_NEGATIVE_SEEK: WIN32_ERROR = 131u32;
        pub const ERROR_NESTING_NOT_ALLOWED: WIN32_ERROR = 215u32;
        pub const ERROR_NETNAME_DELETED: WIN32_ERROR = 64u32;
        pub const ERROR_NETWORK_ACCESS_DENIED: WIN32_ERROR = 65u32;
        pub const ERROR_NOT_ALL_ASSIGNED: WIN32_ERROR = 1300u32;
        pub const ERROR_NOT_DOS_DISK: WIN32_ERROR = 26u32;
        pub const ERROR_NOT_ENOUGH_MEMORY: WIN32_ERROR = 8u32;
        pub const ERROR_NOT_ENOUGH_QUOTA: WIN32_ERROR = 1816u32;
        pub const ERROR_NOT_FOUND: WIN32_ERROR = 1168u32;
        pub const ERROR_NOT_LOCKED: WIN32_ERROR = 158u32;
        pub const ERROR_NOT_READY: WIN32_ERROR = 21u32;
        pub const ERROR_NOT_SAME_DEVICE: WIN32_ERROR = 17u32;
        pub const ERROR_NOT_SUPPORTED: WIN32_ERROR = 50u32;
        pub const ERROR_NO_DATA: WIN32_ERROR = 232u32;
        pub const ERROR_NO_MORE_FILES: WIN32_ERROR = 18u32;
        pub const ERROR_NO_MORE_ITEMS: WIN32_ERROR = 259u32;
        pub const ERROR_NO_PROC_SLOTS: WIN32_ERROR = 89u32;
        pub const ERROR_NO_SYSTEM_RESOURCES: WIN32_ERROR = 1450u32;
        pub const ERROR_NO_UNICODE_TRANSLATION: WIN32_ERROR = 1113u32;
        pub const ERROR_OPERATION_ABORTED: WIN32_ERROR = 995u32;
        pub const ERROR_OUT_OF_PAPER: WIN32_ERROR = 28u32;
        pub const ERROR_PATH_NOT_FOUND: WIN32_ERROR = 3u32;
        pub const ERROR_PIPE_BUSY: WIN32_ERROR = 231u32;
        pub const ERROR_PIPE_CONNECTED: WIN32_ERROR = 535u32;
        pub const ERROR_PORT_UNREACHABLE: WIN32_ERROR = 1234u32;
        pub const ERROR_PRIVILEGE_NOT_HELD: WIN32_ERROR = 1314u32;
        pub const ERROR_READ_FAULT: WIN32_ERROR = 30u32;
        pub const ERROR_RELOC_CHAIN_XEEDS_SEGLIM: WIN32_ERROR = 201u32;
        pub const ERROR_RING2SEG_MUST_BE_MOVABLE: WIN32_ERROR = 200u32;
        pub const ERROR_SECTOR_NOT_FOUND: WIN32_ERROR = 27u32;
        pub const ERROR_SEEK: WIN32_ERROR = 25u32;
        pub const ERROR_SEEK_ON_DEVICE: WIN32_ERROR = 132u32;
        pub const ERROR_SEM_TIMEOUT: WIN32_ERROR = 121u32;
        pub const ERROR_SHARING_BUFFER_EXCEEDED: WIN32_ERROR = 36u32;
        pub const ERROR_SHARING_VIOLATION: WIN32_ERROR = 32u32;
        pub const ERROR_SUCCESS: WIN32_ERROR = 0u32;
        pub const ERROR_TOO_MANY_OPEN_FILES: WIN32_ERROR = 4u32;
        pub const ERROR_WAIT_NO_CHILDREN: WIN32_ERROR = 128u32;
        pub const ERROR_WRITE_FAULT: WIN32_ERROR = 29u32;
        pub const ERROR_WRITE_PROTECT: WIN32_ERROR = 19u32;
        pub const ERROR_WRONG_DISK: WIN32_ERROR = 34u32;
        pub const E_POINTER: crate::windows_sys::core::HRESULT = 0x80004003_u32 as _;
        pub const FALSE: crate::windows_sys::core::BOOL = 0i32;
        pub const GENERIC_READ: GENERIC_ACCESS_RIGHTS = 2147483648u32;
        pub const GENERIC_WRITE: GENERIC_ACCESS_RIGHTS = 1073741824u32;
        pub const HANDLE_FLAG_INHERIT: HANDLE_FLAGS = 1u32;
        pub const INVALID_HANDLE_VALUE: HANDLE = -1i32 as _;
        pub const MAX_PATH: u32 = 260u32;
        pub const STATUS_PENDING: NTSTATUS = 0x103_u32 as _;
        pub const STILL_ACTIVE: NTSTATUS = 0x103_u32 as _;
        pub const S_OK: crate::windows_sys::core::HRESULT = 0x0_u32 as _;
        pub const TRUE: crate::windows_sys::core::BOOL = 1i32;
        pub const WAIT_ABANDONED_0: WAIT_EVENT = 128u32;
        pub const WAIT_FAILED: WAIT_EVENT = 4294967295u32;
        pub const WAIT_OBJECT_0: WAIT_EVENT = 0u32;
        pub const WAIT_TIMEOUT: WAIT_EVENT = 258u32;
        #[link(name = "kernel32.dll", kind = "raw-dylib", modifiers = "+verbatim")]
        unsafe extern "system" {
            pub fn CloseHandle(hobject: HANDLE) -> crate::windows_sys::core::BOOL;
            pub fn DuplicateHandle(
                hsourceprocesshandle: HANDLE,
                hsourcehandle: HANDLE,
                htargetprocesshandle: HANDLE,
                lptargethandle: *mut HANDLE,
                dwdesiredaccess: u32,
                binherithandle: crate::windows_sys::core::BOOL,
                dwoptions: DUPLICATE_HANDLE_OPTIONS,
            ) -> crate::windows_sys::core::BOOL;
            pub fn FreeLibrary(hlibmodule: HMODULE) -> crate::windows_sys::core::BOOL;
            pub fn GetHandleInformation(
                hobject: HANDLE,
                lpdwflags: *mut u32,
            ) -> crate::windows_sys::core::BOOL;
            pub fn GetLastError() -> WIN32_ERROR;
            pub fn LocalFree(hmem: HLOCAL) -> HLOCAL;
            pub fn SetHandleInformation(
                hobject: HANDLE,
                dwmask: u32,
                dwflags: HANDLE_FLAGS,
            ) -> crate::windows_sys::core::BOOL;
            pub fn SetLastError(dwerrcode: WIN32_ERROR);
        }
        #[link(name = "oleaut32.dll", kind = "raw-dylib", modifiers = "+verbatim")]
        unsafe extern "system" {
            pub fn SysAllocStringLen(
                strin: crate::windows_sys::core::PCWSTR,
                ui: u32,
            ) -> crate::windows_sys::core::BSTR;
            pub fn SysFreeString(bstrstring: crate::windows_sys::core::BSTR);
            pub fn SysStringLen(pbstr: crate::windows_sys::core::BSTR) -> u32;
        }
    }
    pub mod Globalization {
        pub type COMPARESTRING_RESULT = i32;
        pub type MULTI_BYTE_TO_WIDE_CHAR_FLAGS = u32;
        #[derive(Clone, Copy, Default)]
        #[repr(C)]
        pub struct NLSVERSIONINFO {
            pub dwNLSVersionInfoSize: u32,
            pub dwNLSVersion: u32,
            pub dwDefinedVersion: u32,
            pub dwEffectiveId: u32,
            pub guidCustomVersion: crate::windows_sys::core::GUID,
        }
        pub const CP_ACP: u32 = 0u32;
        pub const CP_OEMCP: u32 = 1u32;
        pub const CP_UTF7: u32 = 65000u32;
        pub const CP_UTF8: u32 = 65001u32;
        pub const CSTR_EQUAL: COMPARESTRING_RESULT = 2i32;
        pub const LCMAP_BYTEREV: u32 = 2048u32;
        pub const LCMAP_FULLWIDTH: u32 = 8388608u32;
        pub const LCMAP_HALFWIDTH: u32 = 4194304u32;
        pub const LCMAP_HASH: u32 = 262144u32;
        pub const LCMAP_HIRAGANA: u32 = 1048576u32;
        pub const LCMAP_KATAKANA: u32 = 2097152u32;
        pub const LCMAP_LINGUISTIC_CASING: u32 = 16777216u32;
        pub const LCMAP_LOWERCASE: u32 = 256u32;
        pub const LCMAP_SIMPLIFIED_CHINESE: u32 = 33554432u32;
        pub const LCMAP_SORTHANDLE: u32 = 536870912u32;
        pub const LCMAP_SORTKEY: u32 = 1024u32;
        pub const LCMAP_TITLECASE: u32 = 768u32;
        pub const LCMAP_TRADITIONAL_CHINESE: u32 = 67108864u32;
        pub const LCMAP_UPPERCASE: u32 = 512u32;
        pub const MB_ERR_INVALID_CHARS: MULTI_BYTE_TO_WIDE_CHAR_FLAGS = 8u32;
        pub const WC_ERR_INVALID_CHARS: u32 = 128u32;
        pub const WC_NO_BEST_FIT_CHARS: u32 = 1024u32;
        #[link(name = "kernel32.dll", kind = "raw-dylib", modifiers = "+verbatim")]
        unsafe extern "system" {
            pub fn CompareStringOrdinal(
                lpstring1: crate::windows_sys::core::PCWSTR,
                cchcount1: i32,
                lpstring2: crate::windows_sys::core::PCWSTR,
                cchcount2: i32,
                bignorecase: crate::windows_sys::core::BOOL,
            ) -> COMPARESTRING_RESULT;
            pub fn GetACP() -> u32;
            pub fn GetLocaleInfoW(
                locale: u32,
                lctype: u32,
                lplcdata: crate::windows_sys::core::PWSTR,
                cchdata: i32,
            ) -> i32;
            pub fn GetUserDefaultLCID() -> u32;
            pub fn LCMapStringEx(
                lplocalename: crate::windows_sys::core::PCWSTR,
                dwmapflags: u32,
                lpsrcstr: crate::windows_sys::core::PCWSTR,
                cchsrc: i32,
                lpdeststr: crate::windows_sys::core::PWSTR,
                cchdest: i32,
                lpversioninformation: *const NLSVERSIONINFO,
                lpreserved: *const core::ffi::c_void,
                sorthandle: super::Foundation::LPARAM,
            ) -> i32;
            pub fn MultiByteToWideChar(
                codepage: u32,
                dwflags: MULTI_BYTE_TO_WIDE_CHAR_FLAGS,
                lpmultibytestr: crate::windows_sys::core::PCSTR,
                cbmultibyte: i32,
                lpwidecharstr: crate::windows_sys::core::PWSTR,
                cchwidechar: i32,
            ) -> i32;
            pub fn WideCharToMultiByte(
                codepage: u32,
                dwflags: u32,
                lpwidecharstr: crate::windows_sys::core::PCWSTR,
                cchwidechar: i32,
                lpmultibytestr: crate::windows_sys::core::PSTR,
                cbmultibyte: i32,
                lpdefaultchar: crate::windows_sys::core::PCSTR,
                lpuseddefaultchar: *mut crate::windows_sys::core::BOOL,
            ) -> i32;
        }
    }
    pub mod Media {
        pub mod Audio {
            pub type SND_FLAGS = u32;
            pub const SND_ALIAS: SND_FLAGS = 65536u32;
            pub const SND_APPLICATION: SND_FLAGS = 128u32;
            pub const SND_ASYNC: SND_FLAGS = 1u32;
            pub const SND_FILENAME: SND_FLAGS = 131072u32;
            pub const SND_LOOP: SND_FLAGS = 8u32;
            pub const SND_MEMORY: SND_FLAGS = 4u32;
            pub const SND_NODEFAULT: SND_FLAGS = 2u32;
            pub const SND_NOSTOP: SND_FLAGS = 16u32;
            pub const SND_NOWAIT: SND_FLAGS = 8192u32;
            pub const SND_PURGE: SND_FLAGS = 64u32;
            pub const SND_SENTRY: SND_FLAGS = 524288u32;
            pub const SND_SYNC: SND_FLAGS = 0u32;
            pub const SND_SYSTEM: SND_FLAGS = 2097152u32;
        }
    }
    pub mod Networking {
        pub mod WinSock {
            pub type ADDRESS_FAMILY = u16;
            #[derive(Clone, Copy, Default)]
            #[repr(C)]
            pub struct FLOWSPEC {
                pub TokenRate: u32,
                pub TokenBucketSize: u32,
                pub PeakBandwidth: u32,
                pub Latency: u32,
                pub DelayVariation: u32,
                pub ServiceType: u32,
                pub MaxSduSize: u32,
                pub MinimumPolicedSize: u32,
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct IN6_ADDR {
                pub u: IN6_ADDR_0,
            }
            impl Default for IN6_ADDR {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub union IN6_ADDR_0 {
                pub Byte: [u8; 16],
                pub Word: [u16; 8],
            }
            impl Default for IN6_ADDR_0 {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct IN_ADDR {
                pub S_un: IN_ADDR_0,
            }
            impl Default for IN_ADDR {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub union IN_ADDR_0 {
                pub S_un_b: IN_ADDR_0_0,
                pub S_un_w: IN_ADDR_0_1,
                pub S_addr: u32,
            }
            impl Default for IN_ADDR_0 {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy, Default)]
            #[repr(C)]
            pub struct IN_ADDR_0_0 {
                pub s_b1: u8,
                pub s_b2: u8,
                pub s_b3: u8,
                pub s_b4: u8,
            }
            #[derive(Clone, Copy, Default)]
            #[repr(C)]
            pub struct IN_ADDR_0_1 {
                pub s_w1: u16,
                pub s_w2: u16,
            }
            pub type IPPROTO = i32;
            pub type LPWSAOVERLAPPED_COMPLETION_ROUTINE = Option<
                unsafe extern "system" fn(
                    dwerror: u32,
                    cbtransferred: u32,
                    lpoverlapped: *mut super::super::System::IO::OVERLAPPED,
                    dwflags: u32,
                ),
            >;
            #[derive(Clone, Copy, Default)]
            #[repr(C)]
            pub struct QOS {
                pub SendingFlowspec: FLOWSPEC,
                pub ReceivingFlowspec: FLOWSPEC,
                pub ProviderSpecific: WSABUF,
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct SCOPE_ID {
                pub Anonymous: SCOPE_ID_0,
            }
            impl Default for SCOPE_ID {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub union SCOPE_ID_0 {
                pub Anonymous: SCOPE_ID_0_0,
                pub Value: u32,
            }
            impl Default for SCOPE_ID_0 {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy, Default)]
            #[repr(C)]
            pub struct SCOPE_ID_0_0 {
                pub _bitfield: u32,
            }
            pub type SEND_RECV_FLAGS = i32;
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct SOCKADDR {
                pub sa_family: ADDRESS_FAMILY,
                pub sa_data: [i8; 14],
            }
            impl Default for SOCKADDR {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct SOCKADDR_IN {
                pub sin_family: ADDRESS_FAMILY,
                pub sin_port: u16,
                pub sin_addr: IN_ADDR,
                pub sin_zero: [i8; 8],
            }
            impl Default for SOCKADDR_IN {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct SOCKADDR_IN6 {
                pub sin6_family: ADDRESS_FAMILY,
                pub sin6_port: u16,
                pub sin6_flowinfo: u32,
                pub sin6_addr: IN6_ADDR,
                pub Anonymous: SOCKADDR_IN6_0,
            }
            impl Default for SOCKADDR_IN6 {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub union SOCKADDR_IN6_0 {
                pub sin6_scope_id: u32,
                pub sin6_scope_struct: SCOPE_ID,
            }
            impl Default for SOCKADDR_IN6_0 {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            pub type SOCKET = usize;
            pub type WINSOCK_SOCKET_TYPE = i32;
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct WSABUF {
                pub len: u32,
                pub buf: crate::windows_sys::core::PSTR,
            }
            impl Default for WSABUF {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct WSADATA {
                pub wVersion: u16,
                pub wHighVersion: u16,
                pub iMaxSockets: u16,
                pub iMaxUdpDg: u16,
                pub lpVendorInfo: crate::windows_sys::core::PSTR,
                pub szDescription: [i8; 257],
                pub szSystemStatus: [i8; 129],
            }
            impl Default for WSADATA {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct WSAPROTOCOLCHAIN {
                pub ChainLen: i32,
                pub ChainEntries: [u32; 7],
            }
            impl Default for WSAPROTOCOLCHAIN {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct WSAPROTOCOL_INFOW {
                pub dwServiceFlags1: u32,
                pub dwServiceFlags2: u32,
                pub dwServiceFlags3: u32,
                pub dwServiceFlags4: u32,
                pub dwProviderFlags: u32,
                pub ProviderId: crate::windows_sys::core::GUID,
                pub dwCatalogEntryId: u32,
                pub ProtocolChain: WSAPROTOCOLCHAIN,
                pub iVersion: i32,
                pub iAddressFamily: i32,
                pub iMaxSockAddr: i32,
                pub iMinSockAddr: i32,
                pub iSocketType: i32,
                pub iProtocol: i32,
                pub iProtocolMaxOffset: i32,
                pub iNetworkByteOrder: i32,
                pub iSecurityScheme: i32,
                pub dwMessageSize: u32,
                pub dwProviderReserved: u32,
                pub szProtocol: [u16; 256],
            }
            impl Default for WSAPROTOCOL_INFOW {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            pub type WSA_ERROR = i32;
            pub const AF_INET: ADDRESS_FAMILY = 2u16;
            pub const AF_INET6: ADDRESS_FAMILY = 23u16;
            pub const INADDR_ANY: u32 = 0u32;
            pub const INVALID_SOCKET: SOCKET = -1i32 as _;
            pub const IPPROTO_TCP: IPPROTO = 6i32;
            pub const SIO_GET_EXTENSION_FUNCTION_POINTER: u32 = 3355443206u32;
            pub const SOCKET_ERROR: i32 = -1i32;
            pub const SOCK_STREAM: WINSOCK_SOCKET_TYPE = 1i32;
            pub const SOL_SOCKET: i32 = 65535i32;
            pub const SO_ERROR: i32 = 4103i32;
            pub const SO_UPDATE_ACCEPT_CONTEXT: i32 = 28683i32;
            pub const SO_UPDATE_CONNECT_CONTEXT: i32 = 28688i32;
            pub const TF_REUSE_SOCKET: u32 = 2u32;
            pub const WSABASEERR: WSA_ERROR = 10000i32;
            pub const WSADESCRIPTION_LEN: u32 = 256u32;
            pub const WSAEACCES: WSA_ERROR = 10013i32;
            pub const WSAEADDRINUSE: WSA_ERROR = 10048i32;
            pub const WSAEADDRNOTAVAIL: WSA_ERROR = 10049i32;
            pub const WSAEAFNOSUPPORT: WSA_ERROR = 10047i32;
            pub const WSAEALREADY: WSA_ERROR = 10037i32;
            pub const WSAEBADF: WSA_ERROR = 10009i32;
            pub const WSAECANCELLED: WSA_ERROR = 10103i32;
            pub const WSAECONNABORTED: WSA_ERROR = 10053i32;
            pub const WSAECONNREFUSED: WSA_ERROR = 10061i32;
            pub const WSAECONNRESET: WSA_ERROR = 10054i32;
            pub const WSAEDESTADDRREQ: WSA_ERROR = 10039i32;
            pub const WSAEDISCON: WSA_ERROR = 10101i32;
            pub const WSAEDQUOT: WSA_ERROR = 10069i32;
            pub const WSAEFAULT: WSA_ERROR = 10014i32;
            pub const WSAEHOSTDOWN: WSA_ERROR = 10064i32;
            pub const WSAEHOSTUNREACH: WSA_ERROR = 10065i32;
            pub const WSAEINPROGRESS: WSA_ERROR = 10036i32;
            pub const WSAEINTR: WSA_ERROR = 10004i32;
            pub const WSAEINVAL: WSA_ERROR = 10022i32;
            pub const WSAEINVALIDPROCTABLE: WSA_ERROR = 10104i32;
            pub const WSAEINVALIDPROVIDER: WSA_ERROR = 10105i32;
            pub const WSAEISCONN: WSA_ERROR = 10056i32;
            pub const WSAELOOP: WSA_ERROR = 10062i32;
            pub const WSAEMFILE: WSA_ERROR = 10024i32;
            pub const WSAEMSGSIZE: WSA_ERROR = 10040i32;
            pub const WSAENAMETOOLONG: WSA_ERROR = 10063i32;
            pub const WSAENETDOWN: WSA_ERROR = 10050i32;
            pub const WSAENETRESET: WSA_ERROR = 10052i32;
            pub const WSAENETUNREACH: WSA_ERROR = 10051i32;
            pub const WSAENOBUFS: WSA_ERROR = 10055i32;
            pub const WSAENOMORE: WSA_ERROR = 10102i32;
            pub const WSAENOPROTOOPT: WSA_ERROR = 10042i32;
            pub const WSAENOTCONN: WSA_ERROR = 10057i32;
            pub const WSAENOTEMPTY: WSA_ERROR = 10066i32;
            pub const WSAENOTSOCK: WSA_ERROR = 10038i32;
            pub const WSAEOPNOTSUPP: WSA_ERROR = 10045i32;
            pub const WSAEPFNOSUPPORT: WSA_ERROR = 10046i32;
            pub const WSAEPROCLIM: WSA_ERROR = 10067i32;
            pub const WSAEPROTONOSUPPORT: WSA_ERROR = 10043i32;
            pub const WSAEPROTOTYPE: WSA_ERROR = 10041i32;
            pub const WSAEPROVIDERFAILEDINIT: WSA_ERROR = 10106i32;
            pub const WSAEREFUSED: WSA_ERROR = 10112i32;
            pub const WSAEREMOTE: WSA_ERROR = 10071i32;
            pub const WSAESHUTDOWN: WSA_ERROR = 10058i32;
            pub const WSAESOCKTNOSUPPORT: WSA_ERROR = 10044i32;
            pub const WSAESTALE: WSA_ERROR = 10070i32;
            pub const WSAETIMEDOUT: WSA_ERROR = 10060i32;
            pub const WSAETOOMANYREFS: WSA_ERROR = 10059i32;
            pub const WSAEUSERS: WSA_ERROR = 10068i32;
            pub const WSAEWOULDBLOCK: WSA_ERROR = 10035i32;
            pub const WSAID_ACCEPTEX: crate::windows_sys::core::GUID =
                crate::windows_sys::core::GUID::from_u128(0xb5367df1_cbac_11cf_95ca_00805f48a192);
            pub const WSAID_CONNECTEX: crate::windows_sys::core::GUID =
                crate::windows_sys::core::GUID::from_u128(0x25a207b9_ddf3_4660_8ee9_76e58c74063e);
            pub const WSAID_DISCONNECTEX: crate::windows_sys::core::GUID =
                crate::windows_sys::core::GUID::from_u128(0x7fda2e11_8630_436f_a031_f536a6eec157);
            pub const WSAID_GETACCEPTEXSOCKADDRS: crate::windows_sys::core::GUID =
                crate::windows_sys::core::GUID::from_u128(0xb5367df2_cbac_11cf_95ca_00805f48a192);
            pub const WSAID_TRANSMITFILE: crate::windows_sys::core::GUID =
                crate::windows_sys::core::GUID::from_u128(0xb5367df0_cbac_11cf_95ca_00805f48a192);
            pub const WSAID_TRANSMITPACKETS: crate::windows_sys::core::GUID =
                crate::windows_sys::core::GUID::from_u128(0xd9689da0_1f90_11d3_9971_00c04f68c876);
            pub const WSAID_WSAPOLL: crate::windows_sys::core::GUID =
                crate::windows_sys::core::GUID::from_u128(0x18c76f85_dc66_4964_972e_23c27238312b);
            pub const WSAID_WSARECVMSG: crate::windows_sys::core::GUID =
                crate::windows_sys::core::GUID::from_u128(0xf689d7c8_6f1f_436b_8a53_e54fe351c322);
            pub const WSANOTINITIALISED: WSA_ERROR = 10093i32;
            pub const WSANO_DATA: WSA_ERROR = 11004i32;
            pub const WSANO_RECOVERY: WSA_ERROR = 11003i32;
            pub const WSAPROTOCOL_LEN: u32 = 255u32;
            pub const WSASERVICE_NOT_FOUND: WSA_ERROR = 10108i32;
            pub const WSASYSCALLFAILURE: WSA_ERROR = 10107i32;
            pub const WSASYSNOTREADY: WSA_ERROR = 10091i32;
            pub const WSASYS_STATUS_LEN: u32 = 128u32;
            pub const WSATRY_AGAIN: WSA_ERROR = 11002i32;
            pub const WSATYPE_NOT_FOUND: WSA_ERROR = 10109i32;
            pub const WSAVERNOTSUPPORTED: WSA_ERROR = 10092i32;
            #[link(name = "ws2_32.dll", kind = "raw-dylib", modifiers = "+verbatim")]
            unsafe extern "system" {
                pub fn WSAConnect(
                    s: SOCKET,
                    name: *const SOCKADDR,
                    namelen: i32,
                    lpcallerdata: *const WSABUF,
                    lpcalleedata: *mut WSABUF,
                    lpsqos: *const QOS,
                    lpgqos: *const QOS,
                ) -> i32;
                pub fn WSAGetLastError() -> WSA_ERROR;
                pub fn WSAIoctl(
                    s: SOCKET,
                    dwiocontrolcode: u32,
                    lpvinbuffer: *const core::ffi::c_void,
                    cbinbuffer: u32,
                    lpvoutbuffer: *mut core::ffi::c_void,
                    cboutbuffer: u32,
                    lpcbbytesreturned: *mut u32,
                    lpoverlapped: *mut super::super::System::IO::OVERLAPPED,
                    lpcompletionroutine: LPWSAOVERLAPPED_COMPLETION_ROUTINE,
                ) -> i32;
                pub fn WSARecv(
                    s: SOCKET,
                    lpbuffers: *const WSABUF,
                    dwbuffercount: u32,
                    lpnumberofbytesrecvd: *mut u32,
                    lpflags: *mut u32,
                    lpoverlapped: *mut super::super::System::IO::OVERLAPPED,
                    lpcompletionroutine: LPWSAOVERLAPPED_COMPLETION_ROUTINE,
                ) -> i32;
                pub fn WSARecvFrom(
                    s: SOCKET,
                    lpbuffers: *const WSABUF,
                    dwbuffercount: u32,
                    lpnumberofbytesrecvd: *mut u32,
                    lpflags: *mut u32,
                    lpfrom: *mut SOCKADDR,
                    lpfromlen: *mut i32,
                    lpoverlapped: *mut super::super::System::IO::OVERLAPPED,
                    lpcompletionroutine: LPWSAOVERLAPPED_COMPLETION_ROUTINE,
                ) -> i32;
                pub fn WSASend(
                    s: SOCKET,
                    lpbuffers: *const WSABUF,
                    dwbuffercount: u32,
                    lpnumberofbytessent: *mut u32,
                    dwflags: u32,
                    lpoverlapped: *mut super::super::System::IO::OVERLAPPED,
                    lpcompletionroutine: LPWSAOVERLAPPED_COMPLETION_ROUTINE,
                ) -> i32;
                pub fn WSASendTo(
                    s: SOCKET,
                    lpbuffers: *const WSABUF,
                    dwbuffercount: u32,
                    lpnumberofbytessent: *mut u32,
                    dwflags: u32,
                    lpto: *const SOCKADDR,
                    itolen: i32,
                    lpoverlapped: *mut super::super::System::IO::OVERLAPPED,
                    lpcompletionroutine: LPWSAOVERLAPPED_COMPLETION_ROUTINE,
                ) -> i32;
                pub fn WSAStartup(wversionrequested: u16, lpwsadata: *mut WSADATA) -> i32;
                pub fn WSAStringToAddressW(
                    addressstring: crate::windows_sys::core::PCWSTR,
                    addressfamily: i32,
                    lpprotocolinfo: *const WSAPROTOCOL_INFOW,
                    lpaddress: *mut SOCKADDR,
                    lpaddresslength: *mut i32,
                ) -> i32;
                pub fn bind(s: SOCKET, name: *const SOCKADDR, namelen: i32) -> i32;
                pub fn closesocket(s: SOCKET) -> i32;
                pub fn getsockopt(
                    s: SOCKET,
                    level: i32,
                    optname: i32,
                    optval: crate::windows_sys::core::PSTR,
                    optlen: *mut i32,
                ) -> i32;
                pub fn send(
                    s: SOCKET,
                    buf: crate::windows_sys::core::PCSTR,
                    len: i32,
                    flags: SEND_RECV_FLAGS,
                ) -> i32;
                pub fn socket(af: i32, r#type: WINSOCK_SOCKET_TYPE, protocol: i32) -> SOCKET;
            }
        }
    }
    pub mod Security {
        #[derive(Clone, Copy, Default)]
        #[repr(C)]
        pub struct LUID_AND_ATTRIBUTES {
            pub Luid: super::Foundation::LUID,
            pub Attributes: TOKEN_PRIVILEGES_ATTRIBUTES,
        }
        pub type PSECURITY_DESCRIPTOR = *mut core::ffi::c_void;
        #[derive(Clone, Copy)]
        #[repr(C)]
        pub struct SECURITY_ATTRIBUTES {
            pub nLength: u32,
            pub lpSecurityDescriptor: *mut core::ffi::c_void,
            pub bInheritHandle: crate::windows_sys::core::BOOL,
        }
        impl Default for SECURITY_ATTRIBUTES {
            fn default() -> Self {
                unsafe { core::mem::zeroed() }
            }
        }
        pub type TOKEN_ACCESS_MASK = u32;
        #[derive(Clone, Copy)]
        #[repr(C)]
        pub struct TOKEN_PRIVILEGES {
            pub PrivilegeCount: u32,
            pub Privileges: [LUID_AND_ATTRIBUTES; 1],
        }
        impl Default for TOKEN_PRIVILEGES {
            fn default() -> Self {
                unsafe { core::mem::zeroed() }
            }
        }
        pub type TOKEN_PRIVILEGES_ATTRIBUTES = u32;
        pub const SE_CREATE_SYMBOLIC_LINK_NAME: crate::windows_sys::core::PCWSTR = {
            const INPUT: &[u8] = "SeCreateSymbolicLinkPrivilege".as_bytes();
            const OUTPUT_LEN: usize = crate::windows_sys::core::utf16_len(INPUT) + 1;
            const OUTPUT: &[u16; OUTPUT_LEN] = {
                let mut buffer = [0; OUTPUT_LEN];
                let mut input_pos = 0;
                let mut output_pos = 0;
                while let Some((mut code_point, new_pos)) =
                    crate::windows_sys::core::decode_utf8_char(INPUT, input_pos)
                {
                    input_pos = new_pos;
                    if code_point <= 0xffff {
                        buffer[output_pos] = code_point as u16;
                        output_pos += 1;
                    } else {
                        code_point -= 0x10000;
                        buffer[output_pos] = 0xd800 + (code_point >> 10) as u16;
                        output_pos += 1;
                        buffer[output_pos] = 0xdc00 + (code_point & 0x3ff) as u16;
                        output_pos += 1;
                    }
                }
                &{ buffer }
            };
            OUTPUT.as_ptr()
        };
        pub const SE_PRIVILEGE_ENABLED: TOKEN_PRIVILEGES_ATTRIBUTES = 2u32;
        pub const TOKEN_ADJUST_PRIVILEGES: TOKEN_ACCESS_MASK = 32u32;
        #[link(name = "advapi32.dll", kind = "raw-dylib", modifiers = "+verbatim")]
        unsafe extern "system" {
            pub fn AdjustTokenPrivileges(
                tokenhandle: super::Foundation::HANDLE,
                disableallprivileges: crate::windows_sys::core::BOOL,
                newstate: *const TOKEN_PRIVILEGES,
                bufferlength: u32,
                previousstate: *mut TOKEN_PRIVILEGES,
                returnlength: *mut u32,
            ) -> crate::windows_sys::core::BOOL;
            pub fn LookupPrivilegeValueW(
                lpsystemname: crate::windows_sys::core::PCWSTR,
                lpname: crate::windows_sys::core::PCWSTR,
                lpluid: *mut super::Foundation::LUID,
            ) -> crate::windows_sys::core::BOOL;
        }
        pub mod Authorization {
            pub const SDDL_REVISION_1: u32 = 1u32;
            #[link(name = "advapi32.dll", kind = "raw-dylib", modifiers = "+verbatim")]
            unsafe extern "system" {
                pub fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    stringsecuritydescriptor: crate::windows_sys::core::PCWSTR,
                    stringsdrevision: u32,
                    securitydescriptor: *mut super::PSECURITY_DESCRIPTOR,
                    securitydescriptorsize: *mut u32,
                ) -> crate::windows_sys::core::BOOL;
            }
        }
        pub mod Cryptography {
            pub type BCRYPTGENRANDOM_FLAGS = u32;
            pub type BCRYPT_ALG_HANDLE = *mut core::ffi::c_void;
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct CERT_CHAIN_CONTEXT {
                pub cbSize: u32,
                pub TrustStatus: CERT_TRUST_STATUS,
                pub cChain: u32,
                pub rgpChain: *mut *mut CERT_SIMPLE_CHAIN,
                pub cLowerQualityChainContext: u32,
                pub rgpLowerQualityChainContext: *mut *mut CERT_CHAIN_CONTEXT,
                pub fHasRevocationFreshnessTime: crate::windows_sys::core::BOOL,
                pub dwRevocationFreshnessTime: u32,
                pub dwCreateFlags: u32,
                pub ChainId: crate::windows_sys::core::GUID,
            }
            impl Default for CERT_CHAIN_CONTEXT {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct CERT_CHAIN_ELEMENT {
                pub cbSize: u32,
                pub pCertContext: *const CERT_CONTEXT,
                pub TrustStatus: CERT_TRUST_STATUS,
                pub pRevocationInfo: *mut CERT_REVOCATION_INFO,
                pub pIssuanceUsage: *mut CTL_USAGE,
                pub pApplicationUsage: *mut CTL_USAGE,
                pub pwszExtendedErrorInfo: crate::windows_sys::core::PCWSTR,
            }
            impl Default for CERT_CHAIN_ELEMENT {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct CERT_CHAIN_ENGINE_CONFIG {
                pub cbSize: u32,
                pub hRestrictedRoot: HCERTSTORE,
                pub hRestrictedTrust: HCERTSTORE,
                pub hRestrictedOther: HCERTSTORE,
                pub cAdditionalStore: u32,
                pub rghAdditionalStore: *mut HCERTSTORE,
                pub dwFlags: u32,
                pub dwUrlRetrievalTimeout: u32,
                pub MaximumCachedCertificates: u32,
                pub CycleDetectionModulus: u32,
                pub hExclusiveRoot: HCERTSTORE,
                pub hExclusiveTrustedPeople: HCERTSTORE,
                pub dwExclusiveFlags: u32,
            }
            impl Default for CERT_CHAIN_ENGINE_CONFIG {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct CERT_CHAIN_PARA {
                pub cbSize: u32,
                pub RequestedUsage: CERT_USAGE_MATCH,
                pub RequestedIssuancePolicy: CERT_USAGE_MATCH,
                pub dwUrlRetrievalTimeout: u32,
                pub fCheckRevocationFreshnessTime: crate::windows_sys::core::BOOL,
                pub dwRevocationFreshnessTime: u32,
                pub pftCacheResync: *mut super::super::Foundation::FILETIME,
                pub pStrongSignPara: *mut CERT_STRONG_SIGN_PARA,
                pub dwStrongSignFlags: u32,
            }
            impl Default for CERT_CHAIN_PARA {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            pub type CERT_CHAIN_POLICY_FLAGS = u32;
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct CERT_CHAIN_POLICY_PARA {
                pub cbSize: u32,
                pub dwFlags: CERT_CHAIN_POLICY_FLAGS,
                pub pvExtraPolicyPara: *mut core::ffi::c_void,
            }
            impl Default for CERT_CHAIN_POLICY_PARA {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct CERT_CHAIN_POLICY_STATUS {
                pub cbSize: u32,
                pub dwError: u32,
                pub lChainIndex: i32,
                pub lElementIndex: i32,
                pub pvExtraPolicyStatus: *mut core::ffi::c_void,
            }
            impl Default for CERT_CHAIN_POLICY_STATUS {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct CERT_CONTEXT {
                pub dwCertEncodingType: CERT_QUERY_ENCODING_TYPE,
                pub pbCertEncoded: *mut u8,
                pub cbCertEncoded: u32,
                pub pCertInfo: *mut CERT_INFO,
                pub hCertStore: HCERTSTORE,
            }
            impl Default for CERT_CONTEXT {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct CERT_EXTENSION {
                pub pszObjId: crate::windows_sys::core::PSTR,
                pub fCritical: crate::windows_sys::core::BOOL,
                pub Value: CRYPT_INTEGER_BLOB,
            }
            impl Default for CERT_EXTENSION {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct CERT_INFO {
                pub dwVersion: u32,
                pub SerialNumber: CRYPT_INTEGER_BLOB,
                pub SignatureAlgorithm: CRYPT_ALGORITHM_IDENTIFIER,
                pub Issuer: CRYPT_INTEGER_BLOB,
                pub NotBefore: super::super::Foundation::FILETIME,
                pub NotAfter: super::super::Foundation::FILETIME,
                pub Subject: CRYPT_INTEGER_BLOB,
                pub SubjectPublicKeyInfo: CERT_PUBLIC_KEY_INFO,
                pub IssuerUniqueId: CRYPT_BIT_BLOB,
                pub SubjectUniqueId: CRYPT_BIT_BLOB,
                pub cExtension: u32,
                pub rgExtension: *mut CERT_EXTENSION,
            }
            impl Default for CERT_INFO {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            pub type CERT_OPEN_STORE_FLAGS = u32;
            #[derive(Clone, Copy, Default)]
            #[repr(C)]
            pub struct CERT_PUBLIC_KEY_INFO {
                pub Algorithm: CRYPT_ALGORITHM_IDENTIFIER,
                pub PublicKey: CRYPT_BIT_BLOB,
            }
            pub type CERT_QUERY_ENCODING_TYPE = u32;
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct CERT_REVOCATION_CRL_INFO {
                pub cbSize: u32,
                pub pBaseCrlContext: *mut CRL_CONTEXT,
                pub pDeltaCrlContext: *mut CRL_CONTEXT,
                pub pCrlEntry: *mut CRL_ENTRY,
                pub fDeltaCrlEntry: crate::windows_sys::core::BOOL,
            }
            impl Default for CERT_REVOCATION_CRL_INFO {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct CERT_REVOCATION_INFO {
                pub cbSize: u32,
                pub dwRevocationResult: u32,
                pub pszRevocationOid: crate::windows_sys::core::PCSTR,
                pub pvOidSpecificInfo: *mut core::ffi::c_void,
                pub fHasFreshnessTime: crate::windows_sys::core::BOOL,
                pub dwFreshnessTime: u32,
                pub pCrlInfo: *mut CERT_REVOCATION_CRL_INFO,
            }
            impl Default for CERT_REVOCATION_INFO {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct CERT_SIMPLE_CHAIN {
                pub cbSize: u32,
                pub TrustStatus: CERT_TRUST_STATUS,
                pub cElement: u32,
                pub rgpElement: *mut *mut CERT_CHAIN_ELEMENT,
                pub pTrustListInfo: *mut CERT_TRUST_LIST_INFO,
                pub fHasRevocationFreshnessTime: crate::windows_sys::core::BOOL,
                pub dwRevocationFreshnessTime: u32,
            }
            impl Default for CERT_SIMPLE_CHAIN {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            pub type CERT_STRONG_SIGN_FLAGS = u32;
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct CERT_STRONG_SIGN_PARA {
                pub cbSize: u32,
                pub dwInfoChoice: u32,
                pub Anonymous: CERT_STRONG_SIGN_PARA_0,
            }
            impl Default for CERT_STRONG_SIGN_PARA {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub union CERT_STRONG_SIGN_PARA_0 {
                pub pvInfo: *mut core::ffi::c_void,
                pub pSerializedInfo: *mut CERT_STRONG_SIGN_SERIALIZED_INFO,
                pub pszOID: crate::windows_sys::core::PSTR,
            }
            impl Default for CERT_STRONG_SIGN_PARA_0 {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct CERT_STRONG_SIGN_SERIALIZED_INFO {
                pub dwFlags: CERT_STRONG_SIGN_FLAGS,
                pub pwszCNGSignHashAlgids: crate::windows_sys::core::PWSTR,
                pub pwszCNGPubKeyMinBitLengths: crate::windows_sys::core::PWSTR,
            }
            impl Default for CERT_STRONG_SIGN_SERIALIZED_INFO {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct CERT_TRUST_LIST_INFO {
                pub cbSize: u32,
                pub pCtlEntry: *mut CTL_ENTRY,
                pub pCtlContext: *mut CTL_CONTEXT,
            }
            impl Default for CERT_TRUST_LIST_INFO {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy, Default)]
            #[repr(C)]
            pub struct CERT_TRUST_STATUS {
                pub dwErrorStatus: u32,
                pub dwInfoStatus: u32,
            }
            #[derive(Clone, Copy, Default)]
            #[repr(C)]
            pub struct CERT_USAGE_MATCH {
                pub dwType: u32,
                pub Usage: CTL_USAGE,
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct CRL_CONTEXT {
                pub dwCertEncodingType: CERT_QUERY_ENCODING_TYPE,
                pub pbCrlEncoded: *mut u8,
                pub cbCrlEncoded: u32,
                pub pCrlInfo: *mut CRL_INFO,
                pub hCertStore: HCERTSTORE,
            }
            impl Default for CRL_CONTEXT {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct CRL_ENTRY {
                pub SerialNumber: CRYPT_INTEGER_BLOB,
                pub RevocationDate: super::super::Foundation::FILETIME,
                pub cExtension: u32,
                pub rgExtension: *mut CERT_EXTENSION,
            }
            impl Default for CRL_ENTRY {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct CRL_INFO {
                pub dwVersion: u32,
                pub SignatureAlgorithm: CRYPT_ALGORITHM_IDENTIFIER,
                pub Issuer: CRYPT_INTEGER_BLOB,
                pub ThisUpdate: super::super::Foundation::FILETIME,
                pub NextUpdate: super::super::Foundation::FILETIME,
                pub cCRLEntry: u32,
                pub rgCRLEntry: *mut CRL_ENTRY,
                pub cExtension: u32,
                pub rgExtension: *mut CERT_EXTENSION,
            }
            impl Default for CRL_INFO {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct CRYPT_ALGORITHM_IDENTIFIER {
                pub pszObjId: crate::windows_sys::core::PSTR,
                pub Parameters: CRYPT_INTEGER_BLOB,
            }
            impl Default for CRYPT_ALGORITHM_IDENTIFIER {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct CRYPT_ATTRIBUTE {
                pub pszObjId: crate::windows_sys::core::PSTR,
                pub cValue: u32,
                pub rgValue: *mut CRYPT_INTEGER_BLOB,
            }
            impl Default for CRYPT_ATTRIBUTE {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct CRYPT_BIT_BLOB {
                pub cbData: u32,
                pub pbData: *mut u8,
                pub cUnusedBits: u32,
            }
            impl Default for CRYPT_BIT_BLOB {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct CRYPT_INTEGER_BLOB {
                pub cbData: u32,
                pub pbData: *mut u8,
            }
            impl Default for CRYPT_INTEGER_BLOB {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct CTL_CONTEXT {
                pub dwMsgAndCertEncodingType: u32,
                pub pbCtlEncoded: *mut u8,
                pub cbCtlEncoded: u32,
                pub pCtlInfo: *mut CTL_INFO,
                pub hCertStore: HCERTSTORE,
                pub hCryptMsg: *mut core::ffi::c_void,
                pub pbCtlContent: *mut u8,
                pub cbCtlContent: u32,
            }
            impl Default for CTL_CONTEXT {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct CTL_ENTRY {
                pub SubjectIdentifier: CRYPT_INTEGER_BLOB,
                pub cAttribute: u32,
                pub rgAttribute: *mut CRYPT_ATTRIBUTE,
            }
            impl Default for CTL_ENTRY {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct CTL_INFO {
                pub dwVersion: u32,
                pub SubjectUsage: CTL_USAGE,
                pub ListIdentifier: CRYPT_INTEGER_BLOB,
                pub SequenceNumber: CRYPT_INTEGER_BLOB,
                pub ThisUpdate: super::super::Foundation::FILETIME,
                pub NextUpdate: super::super::Foundation::FILETIME,
                pub SubjectAlgorithm: CRYPT_ALGORITHM_IDENTIFIER,
                pub cCTLEntry: u32,
                pub rgCTLEntry: *mut CTL_ENTRY,
                pub cExtension: u32,
                pub rgExtension: *mut CERT_EXTENSION,
            }
            impl Default for CTL_INFO {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct CTL_USAGE {
                pub cUsageIdentifier: u32,
                pub rgpszUsageIdentifier: *mut crate::windows_sys::core::PSTR,
            }
            impl Default for CTL_USAGE {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            pub type HCERTCHAINENGINE = *mut core::ffi::c_void;
            pub type HCERTSTORE = *mut core::ffi::c_void;
            pub type HCRYPTPROV_LEGACY = usize;
            pub type HTTPSPOLICY_CALLBACK_DATA_AUTH_TYPE = u32;
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct HTTPSPolicyCallbackData {
                pub Anonymous: HTTPSPolicyCallbackData_0,
                pub dwAuthType: HTTPSPOLICY_CALLBACK_DATA_AUTH_TYPE,
                pub fdwChecks: u32,
                pub pwszServerName: crate::windows_sys::core::PWSTR,
            }
            impl Default for HTTPSPolicyCallbackData {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub union HTTPSPolicyCallbackData_0 {
                pub cbStruct: u32,
                pub cbSize: u32,
            }
            impl Default for HTTPSPolicyCallbackData_0 {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            pub const AUTHTYPE_SERVER: HTTPSPOLICY_CALLBACK_DATA_AUTH_TYPE = 2u32;
            pub const BCRYPT_USE_SYSTEM_PREFERRED_RNG: BCRYPTGENRANDOM_FLAGS = 2u32;
            pub const CERT_CHAIN_CACHE_END_CERT: u32 = 1u32;
            pub const CERT_CHAIN_POLICY_IGNORE_ALL_REV_UNKNOWN_FLAGS: CERT_CHAIN_POLICY_FLAGS =
                3840u32;
            pub const CERT_CHAIN_POLICY_SSL: crate::windows_sys::core::PCSTR = 4i32 as _;
            pub const CERT_CHAIN_REVOCATION_ACCUMULATIVE_TIMEOUT: u32 = 134217728u32;
            pub const CERT_CHAIN_REVOCATION_CHECK_END_CERT: u32 = 268435456u32;
            pub const CERT_OCSP_RESPONSE_PROP_ID: u32 = 70u32;
            pub const CERT_SET_PROPERTY_IGNORE_PERSIST_ERROR_FLAG: u32 = 2147483648u32;
            pub const CERT_STORE_ADD_ALWAYS: u32 = 4u32;
            pub const CERT_STORE_DEFER_CLOSE_UNTIL_LAST_FREE_FLAG: CERT_OPEN_STORE_FLAGS = 4u32;
            pub const CERT_STORE_PROV_MEMORY: crate::windows_sys::core::PCSTR = 2i32 as _;
            pub const CERT_STORE_PROV_SYSTEM_W: crate::windows_sys::core::PCSTR = 10i32 as _;
            pub const CERT_SYSTEM_STORE_CURRENT_USER_ID: u32 = 1u32;
            pub const CERT_SYSTEM_STORE_LOCAL_MACHINE_ID: u32 = 2u32;
            pub const CERT_SYSTEM_STORE_LOCATION_SHIFT: u32 = 16u32;
            pub const CERT_TRUST_IS_PARTIAL_CHAIN: u32 = 65536u32;
            pub const CERT_TRUST_IS_UNTRUSTED_ROOT: u32 = 32u32;
            pub const PKCS_7_ASN_ENCODING: CERT_QUERY_ENCODING_TYPE = 65536u32;
            pub const USAGE_MATCH_TYPE_AND: u32 = 0u32;
            pub const X509_ASN_ENCODING: CERT_QUERY_ENCODING_TYPE = 1u32;
            pub const szOID_PKIX_KP_SERVER_AUTH: crate::windows_sys::core::PCSTR =
                "1.3.6.1.5.5.7.3.1\u{0}".as_ptr();
            #[link(name = "bcrypt.dll", kind = "raw-dylib", modifiers = "+verbatim")]
            unsafe extern "system" {
                pub fn BCryptGenRandom(
                    halgorithm: BCRYPT_ALG_HANDLE,
                    pbbuffer: *mut u8,
                    cbbuffer: u32,
                    dwflags: BCRYPTGENRANDOM_FLAGS,
                ) -> super::super::Foundation::NTSTATUS;
            }
            #[link(name = "crypt32.dll", kind = "raw-dylib", modifiers = "+verbatim")]
            unsafe extern "system" {
                pub fn CertAddEncodedCertificateToStore(
                    hcertstore: HCERTSTORE,
                    dwcertencodingtype: CERT_QUERY_ENCODING_TYPE,
                    pbcertencoded: *const u8,
                    cbcertencoded: u32,
                    dwadddisposition: u32,
                    ppcertcontext: *mut *mut CERT_CONTEXT,
                ) -> crate::windows_sys::core::BOOL;
                pub fn CertCloseStore(
                    hcertstore: HCERTSTORE,
                    dwflags: u32,
                ) -> crate::windows_sys::core::BOOL;
                pub fn CertCreateCertificateChainEngine(
                    pconfig: *const CERT_CHAIN_ENGINE_CONFIG,
                    phchainengine: *mut HCERTCHAINENGINE,
                ) -> crate::windows_sys::core::BOOL;
                pub fn CertEnumCRLsInStore(
                    hcertstore: HCERTSTORE,
                    pprevcrlcontext: *const CRL_CONTEXT,
                ) -> *mut CRL_CONTEXT;
                pub fn CertEnumCertificatesInStore(
                    hcertstore: HCERTSTORE,
                    pprevcertcontext: *const CERT_CONTEXT,
                ) -> *mut CERT_CONTEXT;
                pub fn CertFreeCertificateChain(pchaincontext: *const CERT_CHAIN_CONTEXT);
                pub fn CertFreeCertificateChainEngine(hchainengine: HCERTCHAINENGINE);
                pub fn CertFreeCertificateContext(
                    pcertcontext: *const CERT_CONTEXT,
                ) -> crate::windows_sys::core::BOOL;
                pub fn CertGetCertificateChain(
                    hchainengine: HCERTCHAINENGINE,
                    pcertcontext: *const CERT_CONTEXT,
                    ptime: *const super::super::Foundation::FILETIME,
                    hadditionalstore: HCERTSTORE,
                    pchainpara: *const CERT_CHAIN_PARA,
                    dwflags: u32,
                    pvreserved: *const core::ffi::c_void,
                    ppchaincontext: *mut *mut CERT_CHAIN_CONTEXT,
                ) -> crate::windows_sys::core::BOOL;
                pub fn CertGetEnhancedKeyUsage(
                    pcertcontext: *const CERT_CONTEXT,
                    dwflags: u32,
                    pusage: *mut CTL_USAGE,
                    pcbusage: *mut u32,
                ) -> crate::windows_sys::core::BOOL;
                pub fn CertOpenStore(
                    lpszstoreprovider: crate::windows_sys::core::PCSTR,
                    dwencodingtype: CERT_QUERY_ENCODING_TYPE,
                    hcryptprov: HCRYPTPROV_LEGACY,
                    dwflags: CERT_OPEN_STORE_FLAGS,
                    pvpara: *const core::ffi::c_void,
                ) -> HCERTSTORE;
                pub fn CertOpenSystemStoreW(
                    hprov: HCRYPTPROV_LEGACY,
                    szsubsystemprotocol: crate::windows_sys::core::PCWSTR,
                ) -> HCERTSTORE;
                pub fn CertSetCertificateContextProperty(
                    pcertcontext: *const CERT_CONTEXT,
                    dwpropid: u32,
                    dwflags: u32,
                    pvdata: *const core::ffi::c_void,
                ) -> crate::windows_sys::core::BOOL;
                pub fn CertVerifyCertificateChainPolicy(
                    pszpolicyoid: crate::windows_sys::core::PCSTR,
                    pchaincontext: *const CERT_CHAIN_CONTEXT,
                    ppolicypara: *const CERT_CHAIN_POLICY_PARA,
                    ppolicystatus: *mut CERT_CHAIN_POLICY_STATUS,
                ) -> crate::windows_sys::core::BOOL;
                pub fn CertVerifyTimeValidity(
                    ptimetoverify: *const super::super::Foundation::FILETIME,
                    pcertinfo: *const CERT_INFO,
                ) -> i32;
            }
        }
    }
    pub mod Storage {
        pub mod FileSystem {
            #[derive(Clone, Copy, Default)]
            #[repr(C)]
            pub struct BY_HANDLE_FILE_INFORMATION {
                pub dwFileAttributes: u32,
                pub ftCreationTime: super::super::Foundation::FILETIME,
                pub ftLastAccessTime: super::super::Foundation::FILETIME,
                pub ftLastWriteTime: super::super::Foundation::FILETIME,
                pub dwVolumeSerialNumber: u32,
                pub nFileSizeHigh: u32,
                pub nFileSizeLow: u32,
                pub nNumberOfLinks: u32,
                pub nFileIndexHigh: u32,
                pub nFileIndexLow: u32,
            }
            pub type COPYFILE2_COPY_PHASE = i32;
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct COPYFILE2_EXTENDED_PARAMETERS {
                pub dwSize: u32,
                pub dwCopyFlags: COPYFILE_FLAGS,
                pub pfCancel: *mut crate::windows_sys::core::BOOL,
                pub pProgressRoutine: PCOPYFILE2_PROGRESS_ROUTINE,
                pub pvCallbackContext: *mut core::ffi::c_void,
            }
            impl Default for COPYFILE2_EXTENDED_PARAMETERS {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct COPYFILE2_MESSAGE {
                pub Type: COPYFILE2_MESSAGE_TYPE,
                pub dwPadding: u32,
                pub Info: COPYFILE2_MESSAGE_0,
            }
            impl Default for COPYFILE2_MESSAGE {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub union COPYFILE2_MESSAGE_0 {
                pub ChunkStarted: COPYFILE2_MESSAGE_0_0,
                pub ChunkFinished: COPYFILE2_MESSAGE_0_1,
                pub StreamStarted: COPYFILE2_MESSAGE_0_2,
                pub StreamFinished: COPYFILE2_MESSAGE_0_3,
                pub PollContinue: COPYFILE2_MESSAGE_0_4,
                pub Error: COPYFILE2_MESSAGE_0_5,
            }
            impl Default for COPYFILE2_MESSAGE_0 {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct COPYFILE2_MESSAGE_0_0 {
                pub dwStreamNumber: u32,
                pub dwReserved: u32,
                pub hSourceFile: super::super::Foundation::HANDLE,
                pub hDestinationFile: super::super::Foundation::HANDLE,
                pub uliChunkNumber: u64,
                pub uliChunkSize: u64,
                pub uliStreamSize: u64,
                pub uliTotalFileSize: u64,
            }
            impl Default for COPYFILE2_MESSAGE_0_0 {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct COPYFILE2_MESSAGE_0_1 {
                pub dwStreamNumber: u32,
                pub dwFlags: u32,
                pub hSourceFile: super::super::Foundation::HANDLE,
                pub hDestinationFile: super::super::Foundation::HANDLE,
                pub uliChunkNumber: u64,
                pub uliChunkSize: u64,
                pub uliStreamSize: u64,
                pub uliStreamBytesTransferred: u64,
                pub uliTotalFileSize: u64,
                pub uliTotalBytesTransferred: u64,
            }
            impl Default for COPYFILE2_MESSAGE_0_1 {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct COPYFILE2_MESSAGE_0_2 {
                pub dwStreamNumber: u32,
                pub dwReserved: u32,
                pub hSourceFile: super::super::Foundation::HANDLE,
                pub hDestinationFile: super::super::Foundation::HANDLE,
                pub uliStreamSize: u64,
                pub uliTotalFileSize: u64,
            }
            impl Default for COPYFILE2_MESSAGE_0_2 {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct COPYFILE2_MESSAGE_0_3 {
                pub dwStreamNumber: u32,
                pub dwReserved: u32,
                pub hSourceFile: super::super::Foundation::HANDLE,
                pub hDestinationFile: super::super::Foundation::HANDLE,
                pub uliStreamSize: u64,
                pub uliStreamBytesTransferred: u64,
                pub uliTotalFileSize: u64,
                pub uliTotalBytesTransferred: u64,
            }
            impl Default for COPYFILE2_MESSAGE_0_3 {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy, Default)]
            #[repr(C)]
            pub struct COPYFILE2_MESSAGE_0_4 {
                pub dwReserved: u32,
            }
            #[derive(Clone, Copy, Default)]
            #[repr(C)]
            pub struct COPYFILE2_MESSAGE_0_5 {
                pub CopyPhase: COPYFILE2_COPY_PHASE,
                pub dwStreamNumber: u32,
                pub hrFailure: crate::windows_sys::core::HRESULT,
                pub dwReserved: u32,
                pub uliChunkNumber: u64,
                pub uliStreamSize: u64,
                pub uliStreamBytesTransferred: u64,
                pub uliTotalFileSize: u64,
                pub uliTotalBytesTransferred: u64,
            }
            pub type COPYFILE2_MESSAGE_ACTION = i32;
            pub type COPYFILE2_MESSAGE_TYPE = i32;
            pub type COPYFILE_FLAGS = u32;
            pub type FILE_ACCESS_RIGHTS = u32;
            #[derive(Clone, Copy, Default)]
            #[repr(C)]
            pub struct FILE_ATTRIBUTE_TAG_INFO {
                pub FileAttributes: u32,
                pub ReparseTag: u32,
            }
            #[derive(Clone, Copy, Default)]
            #[repr(C)]
            pub struct FILE_BASIC_INFO {
                pub CreationTime: i64,
                pub LastAccessTime: i64,
                pub LastWriteTime: i64,
                pub ChangeTime: i64,
                pub FileAttributes: u32,
            }
            pub type FILE_CREATION_DISPOSITION = u32;
            pub type FILE_DEVICE_TYPE = u32;
            pub type FILE_FLAGS_AND_ATTRIBUTES = u32;
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct FILE_ID_128 {
                pub Identifier: [u8; 16],
            }
            impl Default for FILE_ID_128 {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy, Default)]
            #[repr(C)]
            pub struct FILE_ID_INFO {
                pub VolumeSerialNumber: u64,
                pub FileId: FILE_ID_128,
            }
            pub type FILE_INFO_BY_HANDLE_CLASS = i32;
            pub type FILE_SHARE_MODE = u32;
            pub type FILE_TYPE = u32;
            pub type GETFINALPATHNAMEBYHANDLE_FLAGS = u32;
            pub type GET_FILEEX_INFO_LEVELS = i32;
            pub type MOVE_FILE_FLAGS = u32;
            pub type PCOPYFILE2_PROGRESS_ROUTINE = Option<
                unsafe extern "system" fn(
                    pmessage: *const COPYFILE2_MESSAGE,
                    pvcallbackcontext: *const core::ffi::c_void,
                ) -> COPYFILE2_MESSAGE_ACTION,
            >;
            pub type SET_FILE_POINTER_MOVE_METHOD = u32;
            pub type SYMBOLIC_LINK_FLAGS = u32;
            #[derive(Clone, Copy, Default)]
            #[repr(C)]
            pub struct VS_FIXEDFILEINFO {
                pub dwSignature: u32,
                pub dwStrucVersion: u32,
                pub dwFileVersionMS: u32,
                pub dwFileVersionLS: u32,
                pub dwProductVersionMS: u32,
                pub dwProductVersionLS: u32,
                pub dwFileFlagsMask: u32,
                pub dwFileFlags: VS_FIXEDFILEINFO_FILE_FLAGS,
                pub dwFileOS: VS_FIXEDFILEINFO_FILE_OS,
                pub dwFileType: u32,
                pub dwFileSubtype: u32,
                pub dwFileDateMS: u32,
                pub dwFileDateLS: u32,
            }
            pub type VS_FIXEDFILEINFO_FILE_FLAGS = u32;
            pub type VS_FIXEDFILEINFO_FILE_OS = u32;
            #[derive(Clone, Copy, Default)]
            #[repr(C)]
            pub struct WIN32_FILE_ATTRIBUTE_DATA {
                pub dwFileAttributes: u32,
                pub ftCreationTime: super::super::Foundation::FILETIME,
                pub ftLastAccessTime: super::super::Foundation::FILETIME,
                pub ftLastWriteTime: super::super::Foundation::FILETIME,
                pub nFileSizeHigh: u32,
                pub nFileSizeLow: u32,
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct WIN32_FIND_DATAW {
                pub dwFileAttributes: u32,
                pub ftCreationTime: super::super::Foundation::FILETIME,
                pub ftLastAccessTime: super::super::Foundation::FILETIME,
                pub ftLastWriteTime: super::super::Foundation::FILETIME,
                pub nFileSizeHigh: u32,
                pub nFileSizeLow: u32,
                pub dwReserved0: u32,
                pub dwReserved1: u32,
                pub cFileName: [u16; 260],
                pub cAlternateFileName: [u16; 14],
            }
            impl Default for WIN32_FIND_DATAW {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            pub const COPYFILE2_CALLBACK_CHUNK_FINISHED: COPYFILE2_MESSAGE_TYPE = 2i32;
            pub const COPYFILE2_CALLBACK_CHUNK_STARTED: COPYFILE2_MESSAGE_TYPE = 1i32;
            pub const COPYFILE2_CALLBACK_ERROR: COPYFILE2_MESSAGE_TYPE = 6i32;
            pub const COPYFILE2_CALLBACK_POLL_CONTINUE: COPYFILE2_MESSAGE_TYPE = 5i32;
            pub const COPYFILE2_CALLBACK_STREAM_FINISHED: COPYFILE2_MESSAGE_TYPE = 4i32;
            pub const COPYFILE2_CALLBACK_STREAM_STARTED: COPYFILE2_MESSAGE_TYPE = 3i32;
            pub const COPYFILE2_PROGRESS_CANCEL: COPYFILE2_MESSAGE_ACTION = 1i32;
            pub const COPYFILE2_PROGRESS_CONTINUE: COPYFILE2_MESSAGE_ACTION = 0i32;
            pub const COPYFILE2_PROGRESS_PAUSE: COPYFILE2_MESSAGE_ACTION = 4i32;
            pub const COPYFILE2_PROGRESS_QUIET: COPYFILE2_MESSAGE_ACTION = 3i32;
            pub const COPYFILE2_PROGRESS_STOP: COPYFILE2_MESSAGE_ACTION = 2i32;
            pub const COPY_FILE_ALLOW_DECRYPTED_DESTINATION: COPYFILE_FLAGS = 8u32;
            pub const COPY_FILE_COPY_SYMLINK: COPYFILE_FLAGS = 2048u32;
            pub const COPY_FILE_DIRECTORY: COPYFILE_FLAGS = 128u32;
            pub const COPY_FILE_FAIL_IF_EXISTS: COPYFILE_FLAGS = 1u32;
            pub const COPY_FILE_NO_BUFFERING: COPYFILE_FLAGS = 4096u32;
            pub const COPY_FILE_NO_OFFLOAD: COPYFILE_FLAGS = 262144u32;
            pub const COPY_FILE_OPEN_SOURCE_FOR_WRITE: COPYFILE_FLAGS = 4u32;
            pub const COPY_FILE_REQUEST_COMPRESSED_TRAFFIC: COPYFILE_FLAGS = 268435456u32;
            pub const COPY_FILE_REQUEST_SECURITY_PRIVILEGES: COPYFILE_FLAGS = 8192u32;
            pub const COPY_FILE_RESTARTABLE: COPYFILE_FLAGS = 2u32;
            pub const COPY_FILE_RESUME_FROM_PAUSE: COPYFILE_FLAGS = 16384u32;
            pub const FILE_ATTRIBUTE_ARCHIVE: FILE_FLAGS_AND_ATTRIBUTES = 32u32;
            pub const FILE_ATTRIBUTE_COMPRESSED: FILE_FLAGS_AND_ATTRIBUTES = 2048u32;
            pub const FILE_ATTRIBUTE_DEVICE: FILE_FLAGS_AND_ATTRIBUTES = 64u32;
            pub const FILE_ATTRIBUTE_DIRECTORY: FILE_FLAGS_AND_ATTRIBUTES = 16u32;
            pub const FILE_ATTRIBUTE_ENCRYPTED: FILE_FLAGS_AND_ATTRIBUTES = 16384u32;
            pub const FILE_ATTRIBUTE_HIDDEN: FILE_FLAGS_AND_ATTRIBUTES = 2u32;
            pub const FILE_ATTRIBUTE_INTEGRITY_STREAM: FILE_FLAGS_AND_ATTRIBUTES = 32768u32;
            pub const FILE_ATTRIBUTE_NORMAL: FILE_FLAGS_AND_ATTRIBUTES = 128u32;
            pub const FILE_ATTRIBUTE_NOT_CONTENT_INDEXED: FILE_FLAGS_AND_ATTRIBUTES = 8192u32;
            pub const FILE_ATTRIBUTE_NO_SCRUB_DATA: FILE_FLAGS_AND_ATTRIBUTES = 131072u32;
            pub const FILE_ATTRIBUTE_OFFLINE: FILE_FLAGS_AND_ATTRIBUTES = 4096u32;
            pub const FILE_ATTRIBUTE_READONLY: FILE_FLAGS_AND_ATTRIBUTES = 1u32;
            pub const FILE_ATTRIBUTE_REPARSE_POINT: FILE_FLAGS_AND_ATTRIBUTES = 1024u32;
            pub const FILE_ATTRIBUTE_SPARSE_FILE: FILE_FLAGS_AND_ATTRIBUTES = 512u32;
            pub const FILE_ATTRIBUTE_SYSTEM: FILE_FLAGS_AND_ATTRIBUTES = 4u32;
            pub const FILE_ATTRIBUTE_TEMPORARY: FILE_FLAGS_AND_ATTRIBUTES = 256u32;
            pub const FILE_ATTRIBUTE_VIRTUAL: FILE_FLAGS_AND_ATTRIBUTES = 65536u32;
            pub const FILE_DEVICE_CD_ROM: FILE_DEVICE_TYPE = 2u32;
            pub const FILE_DEVICE_DISK: FILE_DEVICE_TYPE = 7u32;
            pub const FILE_FLAG_BACKUP_SEMANTICS: FILE_FLAGS_AND_ATTRIBUTES = 33554432u32;
            pub const FILE_FLAG_FIRST_PIPE_INSTANCE: FILE_FLAGS_AND_ATTRIBUTES = 524288u32;
            pub const FILE_FLAG_OPEN_REPARSE_POINT: FILE_FLAGS_AND_ATTRIBUTES = 2097152u32;
            pub const FILE_FLAG_OVERLAPPED: FILE_FLAGS_AND_ATTRIBUTES = 1073741824u32;
            pub const FILE_GENERIC_READ: FILE_ACCESS_RIGHTS = 1179785u32;
            pub const FILE_GENERIC_WRITE: FILE_ACCESS_RIGHTS = 1179926u32;
            pub const FILE_READ_ATTRIBUTES: FILE_ACCESS_RIGHTS = 128u32;
            pub const FILE_SHARE_DELETE: FILE_SHARE_MODE = 4u32;
            pub const FILE_SHARE_READ: FILE_SHARE_MODE = 1u32;
            pub const FILE_SHARE_WRITE: FILE_SHARE_MODE = 2u32;
            pub const FILE_TYPE_CHAR: FILE_TYPE = 2u32;
            pub const FILE_TYPE_DISK: FILE_TYPE = 1u32;
            pub const FILE_TYPE_PIPE: FILE_TYPE = 3u32;
            pub const FILE_TYPE_REMOTE: FILE_TYPE = 32768u32;
            pub const FILE_TYPE_UNKNOWN: FILE_TYPE = 0u32;
            pub const FILE_WRITE_ATTRIBUTES: FILE_ACCESS_RIGHTS = 256u32;
            pub const FileAttributeTagInfo: FILE_INFO_BY_HANDLE_CLASS = 9i32;
            pub const FileBasicInfo: FILE_INFO_BY_HANDLE_CLASS = 0i32;
            pub const FileIdInfo: FILE_INFO_BY_HANDLE_CLASS = 18i32;
            pub const GetFileExInfoStandard: GET_FILEEX_INFO_LEVELS = 0i32;
            pub const INVALID_FILE_ATTRIBUTES: u32 = 4294967295u32;
            pub const MAXIMUM_REPARSE_DATA_BUFFER_SIZE: u32 = 16384u32;
            pub const MOVEFILE_REPLACE_EXISTING: MOVE_FILE_FLAGS = 1u32;
            pub const OPEN_EXISTING: FILE_CREATION_DISPOSITION = 3u32;
            pub const PIPE_ACCESS_DUPLEX: FILE_FLAGS_AND_ATTRIBUTES = 3u32;
            pub const PIPE_ACCESS_INBOUND: FILE_FLAGS_AND_ATTRIBUTES = 1u32;
            pub const PIPE_ACCESS_OUTBOUND: FILE_FLAGS_AND_ATTRIBUTES = 2u32;
            pub const SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE: SYMBOLIC_LINK_FLAGS = 2u32;
            pub const SYMBOLIC_LINK_FLAG_DIRECTORY: SYMBOLIC_LINK_FLAGS = 1u32;
            pub const SYNCHRONIZE: FILE_ACCESS_RIGHTS = 1048576u32;
            pub const VOLUME_NAME_DOS: GETFINALPATHNAMEBYHANDLE_FLAGS = 0u32;
            #[link(name = "kernel32.dll", kind = "raw-dylib", modifiers = "+verbatim")]
            unsafe extern "system" {
                pub fn CopyFile2(
                    pwszexistingfilename: crate::windows_sys::core::PCWSTR,
                    pwsznewfilename: crate::windows_sys::core::PCWSTR,
                    pextendedparameters: *const COPYFILE2_EXTENDED_PARAMETERS,
                ) -> crate::windows_sys::core::HRESULT;
                pub fn CreateDirectoryW(
                    lppathname: crate::windows_sys::core::PCWSTR,
                    lpsecurityattributes: *const super::super::Security::SECURITY_ATTRIBUTES,
                ) -> crate::windows_sys::core::BOOL;
                pub fn CreateFileW(
                    lpfilename: crate::windows_sys::core::PCWSTR,
                    dwdesiredaccess: u32,
                    dwsharemode: FILE_SHARE_MODE,
                    lpsecurityattributes: *const super::super::Security::SECURITY_ATTRIBUTES,
                    dwcreationdisposition: FILE_CREATION_DISPOSITION,
                    dwflagsandattributes: FILE_FLAGS_AND_ATTRIBUTES,
                    htemplatefile: super::super::Foundation::HANDLE,
                ) -> super::super::Foundation::HANDLE;
                pub fn CreateHardLinkW(
                    lpfilename: crate::windows_sys::core::PCWSTR,
                    lpexistingfilename: crate::windows_sys::core::PCWSTR,
                    lpsecurityattributes: *const super::super::Security::SECURITY_ATTRIBUTES,
                ) -> crate::windows_sys::core::BOOL;
                pub fn CreateSymbolicLinkW(
                    lpsymlinkfilename: crate::windows_sys::core::PCWSTR,
                    lptargetfilename: crate::windows_sys::core::PCWSTR,
                    dwflags: SYMBOLIC_LINK_FLAGS,
                ) -> bool;
                pub fn DeleteFileW(
                    lpfilename: crate::windows_sys::core::PCWSTR,
                ) -> crate::windows_sys::core::BOOL;
                pub fn FindClose(
                    hfindfile: super::super::Foundation::HANDLE,
                ) -> crate::windows_sys::core::BOOL;
                pub fn FindFirstFileW(
                    lpfilename: crate::windows_sys::core::PCWSTR,
                    lpfindfiledata: *mut WIN32_FIND_DATAW,
                ) -> super::super::Foundation::HANDLE;
                pub fn FindFirstVolumeW(
                    lpszvolumename: crate::windows_sys::core::PWSTR,
                    cchbufferlength: u32,
                ) -> super::super::Foundation::HANDLE;
                pub fn FindNextFileW(
                    hfindfile: super::super::Foundation::HANDLE,
                    lpfindfiledata: *mut WIN32_FIND_DATAW,
                ) -> crate::windows_sys::core::BOOL;
                pub fn FindNextVolumeW(
                    hfindvolume: super::super::Foundation::HANDLE,
                    lpszvolumename: crate::windows_sys::core::PWSTR,
                    cchbufferlength: u32,
                ) -> crate::windows_sys::core::BOOL;
                pub fn FindVolumeClose(
                    hfindvolume: super::super::Foundation::HANDLE,
                ) -> crate::windows_sys::core::BOOL;
                pub fn GetBinaryTypeW(
                    lpapplicationname: crate::windows_sys::core::PCWSTR,
                    lpbinarytype: *mut u32,
                ) -> crate::windows_sys::core::BOOL;
                pub fn GetDiskFreeSpaceExW(
                    lpdirectoryname: crate::windows_sys::core::PCWSTR,
                    lpfreebytesavailabletocaller: *mut u64,
                    lptotalnumberofbytes: *mut u64,
                    lptotalnumberoffreebytes: *mut u64,
                ) -> crate::windows_sys::core::BOOL;
                pub fn GetDriveTypeW(lprootpathname: crate::windows_sys::core::PCWSTR) -> u32;
                pub fn GetFileAttributesExW(
                    lpfilename: crate::windows_sys::core::PCWSTR,
                    finfolevelid: GET_FILEEX_INFO_LEVELS,
                    lpfileinformation: *mut core::ffi::c_void,
                ) -> crate::windows_sys::core::BOOL;
                pub fn GetFileAttributesW(lpfilename: crate::windows_sys::core::PCWSTR) -> u32;
                pub fn GetFileInformationByHandle(
                    hfile: super::super::Foundation::HANDLE,
                    lpfileinformation: *mut BY_HANDLE_FILE_INFORMATION,
                ) -> crate::windows_sys::core::BOOL;
                pub fn GetFileInformationByHandleEx(
                    hfile: super::super::Foundation::HANDLE,
                    fileinformationclass: FILE_INFO_BY_HANDLE_CLASS,
                    lpfileinformation: *mut core::ffi::c_void,
                    dwbuffersize: u32,
                ) -> crate::windows_sys::core::BOOL;
                pub fn GetFileType(hfile: super::super::Foundation::HANDLE) -> FILE_TYPE;
                pub fn GetFinalPathNameByHandleW(
                    hfile: super::super::Foundation::HANDLE,
                    lpszfilepath: crate::windows_sys::core::PWSTR,
                    cchfilepath: u32,
                    dwflags: GETFINALPATHNAMEBYHANDLE_FLAGS,
                ) -> u32;
                pub fn GetFullPathNameW(
                    lpfilename: crate::windows_sys::core::PCWSTR,
                    nbufferlength: u32,
                    lpbuffer: crate::windows_sys::core::PWSTR,
                    lpfilepart: *mut crate::windows_sys::core::PWSTR,
                ) -> u32;
                pub fn GetLogicalDriveStringsW(
                    nbufferlength: u32,
                    lpbuffer: crate::windows_sys::core::PWSTR,
                ) -> u32;
                pub fn GetLongPathNameW(
                    lpszshortpath: crate::windows_sys::core::PCWSTR,
                    lpszlongpath: crate::windows_sys::core::PWSTR,
                    cchbuffer: u32,
                ) -> u32;
                pub fn GetShortPathNameW(
                    lpszlongpath: crate::windows_sys::core::PCWSTR,
                    lpszshortpath: crate::windows_sys::core::PWSTR,
                    cchbuffer: u32,
                ) -> u32;
                pub fn GetVolumePathNameW(
                    lpszfilename: crate::windows_sys::core::PCWSTR,
                    lpszvolumepathname: crate::windows_sys::core::PWSTR,
                    cchbufferlength: u32,
                ) -> crate::windows_sys::core::BOOL;
                pub fn GetVolumePathNamesForVolumeNameW(
                    lpszvolumename: crate::windows_sys::core::PCWSTR,
                    lpszvolumepathnames: crate::windows_sys::core::PWSTR,
                    cchbufferlength: u32,
                    lpcchreturnlength: *mut u32,
                ) -> crate::windows_sys::core::BOOL;
                pub fn MoveFileExW(
                    lpexistingfilename: crate::windows_sys::core::PCWSTR,
                    lpnewfilename: crate::windows_sys::core::PCWSTR,
                    dwflags: MOVE_FILE_FLAGS,
                ) -> crate::windows_sys::core::BOOL;
                pub fn ReadFile(
                    hfile: super::super::Foundation::HANDLE,
                    lpbuffer: *mut u8,
                    nnumberofbytestoread: u32,
                    lpnumberofbytesread: *mut u32,
                    lpoverlapped: *mut super::super::System::IO::OVERLAPPED,
                ) -> crate::windows_sys::core::BOOL;
                pub fn RemoveDirectoryW(
                    lppathname: crate::windows_sys::core::PCWSTR,
                ) -> crate::windows_sys::core::BOOL;
                pub fn SetFileAttributesW(
                    lpfilename: crate::windows_sys::core::PCWSTR,
                    dwfileattributes: FILE_FLAGS_AND_ATTRIBUTES,
                ) -> crate::windows_sys::core::BOOL;
                pub fn SetFileInformationByHandle(
                    hfile: super::super::Foundation::HANDLE,
                    fileinformationclass: FILE_INFO_BY_HANDLE_CLASS,
                    lpfileinformation: *const core::ffi::c_void,
                    dwbuffersize: u32,
                ) -> crate::windows_sys::core::BOOL;
                pub fn SetFilePointerEx(
                    hfile: super::super::Foundation::HANDLE,
                    lidistancetomove: i64,
                    lpnewfilepointer: *mut i64,
                    dwmovemethod: SET_FILE_POINTER_MOVE_METHOD,
                ) -> crate::windows_sys::core::BOOL;
                pub fn SetFileTime(
                    hfile: super::super::Foundation::HANDLE,
                    lpcreationtime: *const super::super::Foundation::FILETIME,
                    lplastaccesstime: *const super::super::Foundation::FILETIME,
                    lplastwritetime: *const super::super::Foundation::FILETIME,
                ) -> crate::windows_sys::core::BOOL;
                pub fn WriteFile(
                    hfile: super::super::Foundation::HANDLE,
                    lpbuffer: *const u8,
                    nnumberofbytestowrite: u32,
                    lpnumberofbyteswritten: *mut u32,
                    lpoverlapped: *mut super::super::System::IO::OVERLAPPED,
                ) -> crate::windows_sys::core::BOOL;
            }
            #[link(name = "version.dll", kind = "raw-dylib", modifiers = "+verbatim")]
            unsafe extern "system" {
                pub fn GetFileVersionInfoSizeW(
                    lptstrfilename: crate::windows_sys::core::PCWSTR,
                    lpdwhandle: *mut u32,
                ) -> u32;
                pub fn GetFileVersionInfoW(
                    lptstrfilename: crate::windows_sys::core::PCWSTR,
                    dwhandle: u32,
                    dwlen: u32,
                    lpdata: *mut core::ffi::c_void,
                ) -> crate::windows_sys::core::BOOL;
                pub fn VerQueryValueW(
                    pblock: *const core::ffi::c_void,
                    lpsubblock: crate::windows_sys::core::PCWSTR,
                    lplpbuffer: *mut *mut core::ffi::c_void,
                    pulen: *mut u32,
                ) -> crate::windows_sys::core::BOOL;
            }
        }
    }
    pub mod System {
        pub mod Com {
            #[link(name = "combase.dll", kind = "raw-dylib", modifiers = "+verbatim")]
            unsafe extern "system" {
                pub fn CoTaskMemFree(pv: *const core::ffi::c_void);
            }
            #[link(name = "ole32.dll", kind = "raw-dylib", modifiers = "+verbatim")]
            unsafe extern "system" {
                pub fn ProgIDFromCLSID(
                    clsid: *const crate::windows_sys::core::GUID,
                    lplpszprogid: *mut crate::windows_sys::core::PWSTR,
                ) -> crate::windows_sys::core::HRESULT;
            }
            #[link(name = "oleaut32.dll", kind = "raw-dylib", modifiers = "+verbatim")]
            unsafe extern "system" {
                pub fn GetErrorInfo(
                    dwreserved: u32,
                    pperrinfo: *mut *mut core::ffi::c_void,
                ) -> crate::windows_sys::core::HRESULT;
            }
        }
        pub mod Console {
            pub type CONSOLE_CHARACTER_ATTRIBUTES = u16;
            pub type CONSOLE_MODE = u32;
            #[derive(Clone, Copy, Default)]
            #[repr(C)]
            pub struct CONSOLE_READCONSOLE_CONTROL {
                pub nLength: u32,
                pub nInitialChars: u32,
                pub dwCtrlWakeupMask: u32,
                pub dwControlKeyState: u32,
            }
            #[derive(Clone, Copy, Default)]
            #[repr(C)]
            pub struct CONSOLE_SCREEN_BUFFER_INFO {
                pub dwSize: COORD,
                pub dwCursorPosition: COORD,
                pub wAttributes: CONSOLE_CHARACTER_ATTRIBUTES,
                pub srWindow: SMALL_RECT,
                pub dwMaximumWindowSize: COORD,
            }
            #[derive(Clone, Copy, Default)]
            #[repr(C)]
            pub struct COORD {
                pub X: i16,
                pub Y: i16,
            }
            #[derive(Clone, Copy, Default)]
            #[repr(C)]
            pub struct FOCUS_EVENT_RECORD {
                pub bSetFocus: crate::windows_sys::core::BOOL,
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct INPUT_RECORD {
                pub EventType: u16,
                pub Event: INPUT_RECORD_0,
            }
            impl Default for INPUT_RECORD {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub union INPUT_RECORD_0 {
                pub KeyEvent: KEY_EVENT_RECORD,
                pub MouseEvent: MOUSE_EVENT_RECORD,
                pub WindowBufferSizeEvent: WINDOW_BUFFER_SIZE_RECORD,
                pub MenuEvent: MENU_EVENT_RECORD,
                pub FocusEvent: FOCUS_EVENT_RECORD,
            }
            impl Default for INPUT_RECORD_0 {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct KEY_EVENT_RECORD {
                pub bKeyDown: crate::windows_sys::core::BOOL,
                pub wRepeatCount: u16,
                pub wVirtualKeyCode: u16,
                pub wVirtualScanCode: u16,
                pub uChar: KEY_EVENT_RECORD_0,
                pub dwControlKeyState: u32,
            }
            impl Default for KEY_EVENT_RECORD {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub union KEY_EVENT_RECORD_0 {
                pub UnicodeChar: u16,
                pub AsciiChar: i8,
            }
            impl Default for KEY_EVENT_RECORD_0 {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy, Default)]
            #[repr(C)]
            pub struct MENU_EVENT_RECORD {
                pub dwCommandId: u32,
            }
            #[derive(Clone, Copy, Default)]
            #[repr(C)]
            pub struct MOUSE_EVENT_RECORD {
                pub dwMousePosition: COORD,
                pub dwButtonState: u32,
                pub dwControlKeyState: u32,
                pub dwEventFlags: u32,
            }
            pub type PHANDLER_ROUTINE =
                Option<unsafe extern "system" fn(ctrltype: u32) -> crate::windows_sys::core::BOOL>;
            #[derive(Clone, Copy, Default)]
            #[repr(C)]
            pub struct SMALL_RECT {
                pub Left: i16,
                pub Top: i16,
                pub Right: i16,
                pub Bottom: i16,
            }
            pub type STD_HANDLE = u32;
            #[derive(Clone, Copy, Default)]
            #[repr(C)]
            pub struct WINDOW_BUFFER_SIZE_RECORD {
                pub dwSize: COORD,
            }
            pub const CTRL_BREAK_EVENT: u32 = 1u32;
            pub const CTRL_CLOSE_EVENT: u32 = 2u32;
            pub const CTRL_C_EVENT: u32 = 0u32;
            pub const CTRL_LOGOFF_EVENT: u32 = 5u32;
            pub const CTRL_SHUTDOWN_EVENT: u32 = 6u32;
            pub const ENABLE_VIRTUAL_TERMINAL_PROCESSING: CONSOLE_MODE = 4u32;
            pub const KEY_EVENT: u32 = 1u32;
            pub const STD_ERROR_HANDLE: STD_HANDLE = 4294967284u32;
            pub const STD_INPUT_HANDLE: STD_HANDLE = 4294967286u32;
            pub const STD_OUTPUT_HANDLE: STD_HANDLE = 4294967285u32;
            #[link(name = "kernel32.dll", kind = "raw-dylib", modifiers = "+verbatim")]
            unsafe extern "system" {
                pub fn GenerateConsoleCtrlEvent(
                    dwctrlevent: u32,
                    dwprocessgroupid: u32,
                ) -> crate::windows_sys::core::BOOL;
                pub fn GetConsoleCP() -> u32;
                pub fn GetConsoleMode(
                    hconsolehandle: super::super::Foundation::HANDLE,
                    lpmode: *mut CONSOLE_MODE,
                ) -> crate::windows_sys::core::BOOL;
                pub fn GetConsoleOutputCP() -> u32;
                pub fn GetConsoleScreenBufferInfo(
                    hconsoleoutput: super::super::Foundation::HANDLE,
                    lpconsolescreenbufferinfo: *mut CONSOLE_SCREEN_BUFFER_INFO,
                ) -> crate::windows_sys::core::BOOL;
                pub fn GetConsoleWindow() -> super::super::Foundation::HWND;
                pub fn GetNumberOfConsoleInputEvents(
                    hconsoleinput: super::super::Foundation::HANDLE,
                    lpnumberofevents: *mut u32,
                ) -> crate::windows_sys::core::BOOL;
                pub fn GetStdHandle(nstdhandle: STD_HANDLE) -> super::super::Foundation::HANDLE;
                pub fn ReadConsoleW(
                    hconsoleinput: super::super::Foundation::HANDLE,
                    lpbuffer: *mut core::ffi::c_void,
                    nnumberofcharstoread: u32,
                    lpnumberofcharsread: *mut u32,
                    pinputcontrol: *const CONSOLE_READCONSOLE_CONTROL,
                ) -> crate::windows_sys::core::BOOL;
                pub fn SetConsoleCtrlHandler(
                    handlerroutine: PHANDLER_ROUTINE,
                    add: crate::windows_sys::core::BOOL,
                ) -> crate::windows_sys::core::BOOL;
                pub fn WriteConsoleInputW(
                    hconsoleinput: super::super::Foundation::HANDLE,
                    lpbuffer: *const INPUT_RECORD,
                    nlength: u32,
                    lpnumberofeventswritten: *mut u32,
                ) -> crate::windows_sys::core::BOOL;
                pub fn WriteConsoleW(
                    hconsoleoutput: super::super::Foundation::HANDLE,
                    lpbuffer: crate::windows_sys::core::PCWSTR,
                    nnumberofcharstowrite: u32,
                    lpnumberofcharswritten: *mut u32,
                    lpreserved: *const core::ffi::c_void,
                ) -> crate::windows_sys::core::BOOL;
            }
        }
        pub mod Diagnostics {
            pub mod Debug {
                pub type FORMAT_MESSAGE_OPTIONS = u32;
                pub type THREAD_ERROR_MODE = u32;
                pub const FORMAT_MESSAGE_ALLOCATE_BUFFER: FORMAT_MESSAGE_OPTIONS = 256u32;
                pub const FORMAT_MESSAGE_FROM_SYSTEM: FORMAT_MESSAGE_OPTIONS = 4096u32;
                pub const FORMAT_MESSAGE_IGNORE_INSERTS: FORMAT_MESSAGE_OPTIONS = 512u32;
                pub const SEM_FAILCRITICALERRORS: THREAD_ERROR_MODE = 1u32;
                pub const SEM_NOALIGNMENTFAULTEXCEPT: THREAD_ERROR_MODE = 4u32;
                pub const SEM_NOGPFAULTERRORBOX: THREAD_ERROR_MODE = 2u32;
                pub const SEM_NOOPENFILEERRORBOX: THREAD_ERROR_MODE = 32768u32;
                #[link(name = "kernel32.dll", kind = "raw-dylib", modifiers = "+verbatim")]
                unsafe extern "system" {
                    pub fn FormatMessageW(
                        dwflags: FORMAT_MESSAGE_OPTIONS,
                        lpsource: *const core::ffi::c_void,
                        dwmessageid: u32,
                        dwlanguageid: u32,
                        lpbuffer: crate::windows_sys::core::PWSTR,
                        nsize: u32,
                        arguments: *const *const i8,
                    ) -> u32;
                    pub fn GetErrorMode() -> u32;
                    pub fn SetErrorMode(umode: THREAD_ERROR_MODE) -> THREAD_ERROR_MODE;
                }
            }
            pub mod ToolHelp {
                pub type CREATE_TOOLHELP_SNAPSHOT_FLAGS = u32;
                #[derive(Clone, Copy)]
                #[repr(C)]
                pub struct PROCESSENTRY32W {
                    pub dwSize: u32,
                    pub cntUsage: u32,
                    pub th32ProcessID: u32,
                    pub th32DefaultHeapID: usize,
                    pub th32ModuleID: u32,
                    pub cntThreads: u32,
                    pub th32ParentProcessID: u32,
                    pub pcPriClassBase: i32,
                    pub dwFlags: u32,
                    pub szExeFile: [u16; 260],
                }
                impl Default for PROCESSENTRY32W {
                    fn default() -> Self {
                        unsafe { core::mem::zeroed() }
                    }
                }
                pub const TH32CS_SNAPPROCESS: CREATE_TOOLHELP_SNAPSHOT_FLAGS = 2u32;
                #[link(name = "kernel32.dll", kind = "raw-dylib", modifiers = "+verbatim")]
                unsafe extern "system" {
                    pub fn CreateToolhelp32Snapshot(
                        dwflags: CREATE_TOOLHELP_SNAPSHOT_FLAGS,
                        th32processid: u32,
                    ) -> super::super::super::Foundation::HANDLE;
                    pub fn Process32FirstW(
                        hsnapshot: super::super::super::Foundation::HANDLE,
                        lppe: *mut PROCESSENTRY32W,
                    ) -> crate::windows_sys::core::BOOL;
                    pub fn Process32NextW(
                        hsnapshot: super::super::super::Foundation::HANDLE,
                        lppe: *mut PROCESSENTRY32W,
                    ) -> crate::windows_sys::core::BOOL;
                }
            }
        }
        pub mod Environment {
            #[link(name = "kernel32.dll", kind = "raw-dylib", modifiers = "+verbatim")]
            unsafe extern "system" {
                pub fn ExpandEnvironmentStringsW(
                    lpsrc: crate::windows_sys::core::PCWSTR,
                    lpdst: crate::windows_sys::core::PWSTR,
                    nsize: u32,
                ) -> u32;
                pub fn FreeEnvironmentStringsW(
                    penv: crate::windows_sys::core::PCWSTR,
                ) -> crate::windows_sys::core::BOOL;
                pub fn GetEnvironmentStringsW() -> crate::windows_sys::core::PWSTR;
                pub fn NeedCurrentDirectoryForExePathW(
                    exename: crate::windows_sys::core::PCWSTR,
                ) -> crate::windows_sys::core::BOOL;
                pub fn SetEnvironmentVariableW(
                    lpname: crate::windows_sys::core::PCWSTR,
                    lpvalue: crate::windows_sys::core::PCWSTR,
                ) -> crate::windows_sys::core::BOOL;
            }
        }
        pub mod IO {
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct OVERLAPPED {
                pub Internal: usize,
                pub InternalHigh: usize,
                pub Anonymous: OVERLAPPED_0,
                pub hEvent: super::super::Foundation::HANDLE,
            }
            impl Default for OVERLAPPED {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub union OVERLAPPED_0 {
                pub Anonymous: OVERLAPPED_0_0,
                pub Pointer: *mut core::ffi::c_void,
            }
            impl Default for OVERLAPPED_0 {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy, Default)]
            #[repr(C)]
            pub struct OVERLAPPED_0_0 {
                pub Offset: u32,
                pub OffsetHigh: u32,
            }
            #[link(name = "kernel32.dll", kind = "raw-dylib", modifiers = "+verbatim")]
            unsafe extern "system" {
                pub fn CancelIoEx(
                    hfile: super::super::Foundation::HANDLE,
                    lpoverlapped: *const OVERLAPPED,
                ) -> crate::windows_sys::core::BOOL;
                pub fn CreateIoCompletionPort(
                    filehandle: super::super::Foundation::HANDLE,
                    existingcompletionport: super::super::Foundation::HANDLE,
                    completionkey: usize,
                    numberofconcurrentthreads: u32,
                ) -> super::super::Foundation::HANDLE;
                pub fn DeviceIoControl(
                    hdevice: super::super::Foundation::HANDLE,
                    dwiocontrolcode: u32,
                    lpinbuffer: *const core::ffi::c_void,
                    ninbuffersize: u32,
                    lpoutbuffer: *mut core::ffi::c_void,
                    noutbuffersize: u32,
                    lpbytesreturned: *mut u32,
                    lpoverlapped: *mut OVERLAPPED,
                ) -> crate::windows_sys::core::BOOL;
                pub fn GetOverlappedResult(
                    hfile: super::super::Foundation::HANDLE,
                    lpoverlapped: *const OVERLAPPED,
                    lpnumberofbytestransferred: *mut u32,
                    bwait: crate::windows_sys::core::BOOL,
                ) -> crate::windows_sys::core::BOOL;
                pub fn GetQueuedCompletionStatus(
                    completionport: super::super::Foundation::HANDLE,
                    lpnumberofbytestransferred: *mut u32,
                    lpcompletionkey: *mut usize,
                    lpoverlapped: *mut *mut OVERLAPPED,
                    dwmilliseconds: u32,
                ) -> crate::windows_sys::core::BOOL;
                pub fn PostQueuedCompletionStatus(
                    completionport: super::super::Foundation::HANDLE,
                    dwnumberofbytestransferred: u32,
                    dwcompletionkey: usize,
                    lpoverlapped: *const OVERLAPPED,
                ) -> crate::windows_sys::core::BOOL;
            }
        }
        pub mod Ioctl {
            pub const FILE_DEVICE_CD_ROM_FILE_SYSTEM: u32 = 3u32;
            pub const FILE_DEVICE_CONSOLE: u32 = 80u32;
            pub const FILE_DEVICE_CONTROLLER: u32 = 4u32;
            pub const FILE_DEVICE_DATALINK: u32 = 5u32;
            pub const FILE_DEVICE_DFS: u32 = 6u32;
            pub const FILE_DEVICE_DISK_FILE_SYSTEM: u32 = 8u32;
            pub const FILE_DEVICE_KEYBOARD: u32 = 11u32;
            pub const FILE_DEVICE_MODEM: u32 = 43u32;
            pub const FILE_DEVICE_MOUSE: u32 = 15u32;
            pub const FILE_DEVICE_NAMED_PIPE: u32 = 17u32;
            pub const FILE_DEVICE_NETWORK_FILE_SYSTEM: u32 = 20u32;
            pub const FILE_DEVICE_NULL: u32 = 21u32;
            pub const FILE_DEVICE_PARALLEL_PORT: u32 = 22u32;
            pub const FILE_DEVICE_PRINTER: u32 = 24u32;
            pub const FILE_DEVICE_SCREEN: u32 = 28u32;
            pub const FILE_DEVICE_SERIAL_PORT: u32 = 27u32;
            pub const FILE_DEVICE_SOUND: u32 = 29u32;
            pub const FILE_DEVICE_VIRTUAL_DISK: u32 = 36u32;
            pub const FSCTL_GET_REPARSE_POINT: u32 = 589992u32;
            pub const FSCTL_QUERY_PERSISTENT_VOLUME_STATE: u32 = 590396u32;
        }
        pub mod JobObjects {
            pub type JOBOBJECTINFOCLASS = i32;
            #[derive(Clone, Copy, Default)]
            #[repr(C)]
            pub struct JOBOBJECT_BASIC_ACCOUNTING_INFORMATION {
                pub TotalUserTime: i64,
                pub TotalKernelTime: i64,
                pub ThisPeriodTotalUserTime: i64,
                pub ThisPeriodTotalKernelTime: i64,
                pub TotalPageFaultCount: u32,
                pub TotalProcesses: u32,
                pub ActiveProcesses: u32,
                pub TotalTerminatedProcesses: u32,
            }
            #[derive(Clone, Copy, Default)]
            #[repr(C)]
            pub struct JOBOBJECT_BASIC_LIMIT_INFORMATION {
                pub PerProcessUserTimeLimit: i64,
                pub PerJobUserTimeLimit: i64,
                pub LimitFlags: JOB_OBJECT_LIMIT,
                pub MinimumWorkingSetSize: usize,
                pub MaximumWorkingSetSize: usize,
                pub ActiveProcessLimit: u32,
                pub Affinity: usize,
                pub PriorityClass: u32,
                pub SchedulingClass: u32,
            }
            #[derive(Clone, Copy, Default)]
            #[repr(C)]
            pub struct JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
                pub BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION,
                pub IoInfo: super::Threading::IO_COUNTERS,
                pub ProcessMemoryLimit: usize,
                pub JobMemoryLimit: usize,
                pub PeakProcessMemoryUsed: usize,
                pub PeakJobMemoryUsed: usize,
            }
            pub type JOB_OBJECT_LIMIT = u32;
            pub const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: JOB_OBJECT_LIMIT = 8192u32;
            pub const JobObjectBasicAccountingInformation: JOBOBJECTINFOCLASS = 1i32;
            pub const JobObjectExtendedLimitInformation: JOBOBJECTINFOCLASS = 9i32;
            #[link(name = "kernel32.dll", kind = "raw-dylib", modifiers = "+verbatim")]
            unsafe extern "system" {
                pub fn AssignProcessToJobObject(
                    hjob: super::super::Foundation::HANDLE,
                    hprocess: super::super::Foundation::HANDLE,
                ) -> crate::windows_sys::core::BOOL;
                pub fn CreateJobObjectW(
                    lpjobattributes: *const super::super::Security::SECURITY_ATTRIBUTES,
                    lpname: crate::windows_sys::core::PCWSTR,
                ) -> super::super::Foundation::HANDLE;
                pub fn QueryInformationJobObject(
                    hjob: super::super::Foundation::HANDLE,
                    jobobjectinformationclass: JOBOBJECTINFOCLASS,
                    lpjobobjectinformation: *mut core::ffi::c_void,
                    cbjobobjectinformationlength: u32,
                    lpreturnlength: *mut u32,
                ) -> crate::windows_sys::core::BOOL;
                pub fn SetInformationJobObject(
                    hjob: super::super::Foundation::HANDLE,
                    jobobjectinformationclass: JOBOBJECTINFOCLASS,
                    lpjobobjectinformation: *const core::ffi::c_void,
                    cbjobobjectinformationlength: u32,
                ) -> crate::windows_sys::core::BOOL;
                pub fn TerminateJobObject(
                    hjob: super::super::Foundation::HANDLE,
                    uexitcode: u32,
                ) -> crate::windows_sys::core::BOOL;
            }
        }
        pub mod Kernel {
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct LIST_ENTRY {
                pub Flink: *mut LIST_ENTRY,
                pub Blink: *mut LIST_ENTRY,
            }
            impl Default for LIST_ENTRY {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
        }
        pub mod LibraryLoader {
            pub type LOAD_LIBRARY_FLAGS = u32;
            pub const LOAD_LIBRARY_SEARCH_APPLICATION_DIR: LOAD_LIBRARY_FLAGS = 512u32;
            pub const LOAD_LIBRARY_SEARCH_DEFAULT_DIRS: LOAD_LIBRARY_FLAGS = 4096u32;
            pub const LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR: LOAD_LIBRARY_FLAGS = 256u32;
            pub const LOAD_LIBRARY_SEARCH_SYSTEM32: LOAD_LIBRARY_FLAGS = 2048u32;
            pub const LOAD_LIBRARY_SEARCH_USER_DIRS: LOAD_LIBRARY_FLAGS = 1024u32;
            #[link(name = "kernel32.dll", kind = "raw-dylib", modifiers = "+verbatim")]
            unsafe extern "system" {
                pub fn AddDllDirectory(
                    newdirectory: crate::windows_sys::core::PCWSTR,
                ) -> *mut core::ffi::c_void;
                pub fn GetModuleFileNameW(
                    hmodule: super::super::Foundation::HMODULE,
                    lpfilename: crate::windows_sys::core::PWSTR,
                    nsize: u32,
                ) -> u32;
                pub fn GetModuleHandleW(
                    lpmodulename: crate::windows_sys::core::PCWSTR,
                ) -> super::super::Foundation::HMODULE;
                pub fn GetProcAddress(
                    hmodule: super::super::Foundation::HMODULE,
                    lpprocname: crate::windows_sys::core::PCSTR,
                ) -> super::super::Foundation::FARPROC;
                pub fn LoadLibraryExW(
                    lplibfilename: crate::windows_sys::core::PCWSTR,
                    hfile: super::super::Foundation::HANDLE,
                    dwflags: LOAD_LIBRARY_FLAGS,
                ) -> super::super::Foundation::HMODULE;
                pub fn LoadLibraryW(
                    lplibfilename: crate::windows_sys::core::PCWSTR,
                ) -> super::super::Foundation::HMODULE;
                pub fn RemoveDllDirectory(
                    cookie: *const core::ffi::c_void,
                ) -> crate::windows_sys::core::BOOL;
            }
        }
        pub mod Memory {
            pub type FILE_MAP = u32;
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct MEMORY_BASIC_INFORMATION {
                pub BaseAddress: *mut core::ffi::c_void,
                pub AllocationBase: *mut core::ffi::c_void,
                pub AllocationProtect: PAGE_PROTECTION_FLAGS,
                pub PartitionId: u16,
                pub RegionSize: usize,
                pub State: VIRTUAL_ALLOCATION_TYPE,
                pub Protect: PAGE_PROTECTION_FLAGS,
                pub Type: PAGE_TYPE,
            }
            impl Default for MEMORY_BASIC_INFORMATION {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct MEMORY_MAPPED_VIEW_ADDRESS {
                pub Value: *mut core::ffi::c_void,
            }
            impl Default for MEMORY_MAPPED_VIEW_ADDRESS {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            pub type PAGE_PROTECTION_FLAGS = u32;
            pub type PAGE_TYPE = u32;
            pub type VIRTUAL_ALLOCATION_TYPE = u32;
            pub const FILE_MAP_ALL_ACCESS: FILE_MAP = 983071u32;
            pub const FILE_MAP_COPY: FILE_MAP = 1u32;
            pub const FILE_MAP_EXECUTE: FILE_MAP = 32u32;
            pub const FILE_MAP_READ: FILE_MAP = 4u32;
            pub const FILE_MAP_WRITE: FILE_MAP = 2u32;
            pub const MEM_COMMIT: VIRTUAL_ALLOCATION_TYPE = 4096u32;
            pub const MEM_FREE: VIRTUAL_ALLOCATION_TYPE = 65536u32;
            pub const MEM_IMAGE: PAGE_TYPE = 16777216u32;
            pub const MEM_MAPPED: PAGE_TYPE = 262144u32;
            pub const MEM_PRIVATE: PAGE_TYPE = 131072u32;
            pub const MEM_RESERVE: VIRTUAL_ALLOCATION_TYPE = 8192u32;
            pub const PAGE_EXECUTE: PAGE_PROTECTION_FLAGS = 16u32;
            pub const PAGE_EXECUTE_READ: PAGE_PROTECTION_FLAGS = 32u32;
            pub const PAGE_EXECUTE_READWRITE: PAGE_PROTECTION_FLAGS = 64u32;
            pub const PAGE_EXECUTE_WRITECOPY: PAGE_PROTECTION_FLAGS = 128u32;
            pub const PAGE_GUARD: PAGE_PROTECTION_FLAGS = 256u32;
            pub const PAGE_NOACCESS: PAGE_PROTECTION_FLAGS = 1u32;
            pub const PAGE_NOCACHE: PAGE_PROTECTION_FLAGS = 512u32;
            pub const PAGE_READONLY: PAGE_PROTECTION_FLAGS = 2u32;
            pub const PAGE_READWRITE: PAGE_PROTECTION_FLAGS = 4u32;
            pub const PAGE_WRITECOMBINE: PAGE_PROTECTION_FLAGS = 1024u32;
            pub const PAGE_WRITECOPY: PAGE_PROTECTION_FLAGS = 8u32;
            pub const SEC_COMMIT: PAGE_PROTECTION_FLAGS = 134217728u32;
            pub const SEC_IMAGE: PAGE_PROTECTION_FLAGS = 16777216u32;
            pub const SEC_LARGE_PAGES: PAGE_PROTECTION_FLAGS = 2147483648u32;
            pub const SEC_NOCACHE: PAGE_PROTECTION_FLAGS = 268435456u32;
            pub const SEC_RESERVE: PAGE_PROTECTION_FLAGS = 67108864u32;
            pub const SEC_WRITECOMBINE: PAGE_PROTECTION_FLAGS = 1073741824u32;
            #[link(name = "kernel32.dll", kind = "raw-dylib", modifiers = "+verbatim")]
            unsafe extern "system" {
                pub fn CreateFileMappingW(
                    hfile: super::super::Foundation::HANDLE,
                    lpfilemappingattributes: *const super::super::Security::SECURITY_ATTRIBUTES,
                    flprotect: PAGE_PROTECTION_FLAGS,
                    dwmaximumsizehigh: u32,
                    dwmaximumsizelow: u32,
                    lpname: crate::windows_sys::core::PCWSTR,
                ) -> super::super::Foundation::HANDLE;
                pub fn MapViewOfFile(
                    hfilemappingobject: super::super::Foundation::HANDLE,
                    dwdesiredaccess: FILE_MAP,
                    dwfileoffsethigh: u32,
                    dwfileoffsetlow: u32,
                    dwnumberofbytestomap: usize,
                ) -> MEMORY_MAPPED_VIEW_ADDRESS;
                pub fn OpenFileMappingW(
                    dwdesiredaccess: u32,
                    binherithandle: crate::windows_sys::core::BOOL,
                    lpname: crate::windows_sys::core::PCWSTR,
                ) -> super::super::Foundation::HANDLE;
                pub fn UnmapViewOfFile(
                    lpbaseaddress: MEMORY_MAPPED_VIEW_ADDRESS,
                ) -> crate::windows_sys::core::BOOL;
                pub fn VirtualQuery(
                    lpaddress: *const core::ffi::c_void,
                    lpbuffer: *mut MEMORY_BASIC_INFORMATION,
                    dwlength: usize,
                ) -> usize;
            }
        }
        pub mod Performance {
            #[link(name = "kernel32.dll", kind = "raw-dylib", modifiers = "+verbatim")]
            unsafe extern "system" {
                pub fn QueryPerformanceCounter(
                    lpperformancecount: *mut i64,
                ) -> crate::windows_sys::core::BOOL;
                pub fn QueryPerformanceFrequency(
                    lpfrequency: *mut i64,
                ) -> crate::windows_sys::core::BOOL;
            }
        }
        pub mod Pipes {
            pub type NAMED_PIPE_MODE = u32;
            pub const NMPWAIT_WAIT_FOREVER: u32 = 4294967295u32;
            pub const PIPE_READMODE_BYTE: NAMED_PIPE_MODE = 0u32;
            pub const PIPE_READMODE_MESSAGE: NAMED_PIPE_MODE = 2u32;
            pub const PIPE_REJECT_REMOTE_CLIENTS: NAMED_PIPE_MODE = 8u32;
            pub const PIPE_TYPE_BYTE: NAMED_PIPE_MODE = 0u32;
            pub const PIPE_TYPE_MESSAGE: NAMED_PIPE_MODE = 4u32;
            pub const PIPE_UNLIMITED_INSTANCES: u32 = 255u32;
            pub const PIPE_WAIT: NAMED_PIPE_MODE = 0u32;
            #[link(name = "kernel32.dll", kind = "raw-dylib", modifiers = "+verbatim")]
            unsafe extern "system" {
                pub fn ConnectNamedPipe(
                    hnamedpipe: super::super::Foundation::HANDLE,
                    lpoverlapped: *mut super::IO::OVERLAPPED,
                ) -> crate::windows_sys::core::BOOL;
                pub fn CreateNamedPipeW(
                    lpname: crate::windows_sys::core::PCWSTR,
                    dwopenmode: super::super::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES,
                    dwpipemode: NAMED_PIPE_MODE,
                    nmaxinstances: u32,
                    noutbuffersize: u32,
                    ninbuffersize: u32,
                    ndefaulttimeout: u32,
                    lpsecurityattributes: *const super::super::Security::SECURITY_ATTRIBUTES,
                ) -> super::super::Foundation::HANDLE;
                pub fn CreatePipe(
                    hreadpipe: *mut super::super::Foundation::HANDLE,
                    hwritepipe: *mut super::super::Foundation::HANDLE,
                    lppipeattributes: *const super::super::Security::SECURITY_ATTRIBUTES,
                    nsize: u32,
                ) -> crate::windows_sys::core::BOOL;
                pub fn GetNamedPipeHandleStateW(
                    hnamedpipe: super::super::Foundation::HANDLE,
                    lpstate: *mut NAMED_PIPE_MODE,
                    lpcurinstances: *mut u32,
                    lpmaxcollectioncount: *mut u32,
                    lpcollectdatatimeout: *mut u32,
                    lpusername: crate::windows_sys::core::PWSTR,
                    nmaxusernamesize: u32,
                ) -> crate::windows_sys::core::BOOL;
                pub fn PeekNamedPipe(
                    hnamedpipe: super::super::Foundation::HANDLE,
                    lpbuffer: *mut core::ffi::c_void,
                    nbuffersize: u32,
                    lpbytesread: *mut u32,
                    lptotalbytesavail: *mut u32,
                    lpbytesleftthismessage: *mut u32,
                ) -> crate::windows_sys::core::BOOL;
                pub fn SetNamedPipeHandleState(
                    hnamedpipe: super::super::Foundation::HANDLE,
                    lpmode: *const NAMED_PIPE_MODE,
                    lpmaxcollectioncount: *const u32,
                    lpcollectdatatimeout: *const u32,
                ) -> crate::windows_sys::core::BOOL;
                pub fn WaitNamedPipeW(
                    lpnamedpipename: crate::windows_sys::core::PCWSTR,
                    ntimeout: u32,
                ) -> crate::windows_sys::core::BOOL;
            }
        }
        pub mod Registry {
            pub type HKEY = *mut core::ffi::c_void;
            pub type REG_CREATE_KEY_DISPOSITION = u32;
            pub type REG_NOTIFY_FILTER = u32;
            pub type REG_OPEN_CREATE_OPTIONS = u32;
            pub type REG_RESTORE_KEY_FLAGS = i32;
            pub type REG_SAM_FLAGS = u32;
            pub type REG_VALUE_TYPE = u32;
            pub const HKEY_CLASSES_ROOT: HKEY = -2147483648i32 as _;
            pub const HKEY_CURRENT_CONFIG: HKEY = -2147483643i32 as _;
            pub const HKEY_CURRENT_USER: HKEY = -2147483647i32 as _;
            pub const HKEY_DYN_DATA: HKEY = -2147483642i32 as _;
            pub const HKEY_LOCAL_MACHINE: HKEY = -2147483646i32 as _;
            pub const HKEY_PERFORMANCE_DATA: HKEY = -2147483644i32 as _;
            pub const HKEY_USERS: HKEY = -2147483645i32 as _;
            pub const KEY_ALL_ACCESS: REG_SAM_FLAGS = 983103u32;
            pub const KEY_CREATE_LINK: REG_SAM_FLAGS = 32u32;
            pub const KEY_CREATE_SUB_KEY: REG_SAM_FLAGS = 4u32;
            pub const KEY_ENUMERATE_SUB_KEYS: REG_SAM_FLAGS = 8u32;
            pub const KEY_EXECUTE: REG_SAM_FLAGS = 131097u32;
            pub const KEY_NOTIFY: REG_SAM_FLAGS = 16u32;
            pub const KEY_QUERY_VALUE: REG_SAM_FLAGS = 1u32;
            pub const KEY_READ: REG_SAM_FLAGS = 131097u32;
            pub const KEY_SET_VALUE: REG_SAM_FLAGS = 2u32;
            pub const KEY_WOW64_32KEY: REG_SAM_FLAGS = 512u32;
            pub const KEY_WOW64_64KEY: REG_SAM_FLAGS = 256u32;
            pub const KEY_WRITE: REG_SAM_FLAGS = 131078u32;
            pub const REG_BINARY: REG_VALUE_TYPE = 3u32;
            pub const REG_CREATED_NEW_KEY: REG_CREATE_KEY_DISPOSITION = 1u32;
            pub const REG_DWORD: REG_VALUE_TYPE = 4u32;
            pub const REG_DWORD_BIG_ENDIAN: REG_VALUE_TYPE = 5u32;
            pub const REG_DWORD_LITTLE_ENDIAN: REG_VALUE_TYPE = 4u32;
            pub const REG_EXPAND_SZ: REG_VALUE_TYPE = 2u32;
            pub const REG_FULL_RESOURCE_DESCRIPTOR: REG_VALUE_TYPE = 9u32;
            pub const REG_LINK: REG_VALUE_TYPE = 6u32;
            pub const REG_MULTI_SZ: REG_VALUE_TYPE = 7u32;
            pub const REG_NONE: REG_VALUE_TYPE = 0u32;
            pub const REG_NOTIFY_CHANGE_ATTRIBUTES: REG_NOTIFY_FILTER = 2u32;
            pub const REG_NOTIFY_CHANGE_LAST_SET: REG_NOTIFY_FILTER = 4u32;
            pub const REG_NOTIFY_CHANGE_NAME: REG_NOTIFY_FILTER = 1u32;
            pub const REG_NOTIFY_CHANGE_SECURITY: REG_NOTIFY_FILTER = 8u32;
            pub const REG_OPENED_EXISTING_KEY: REG_CREATE_KEY_DISPOSITION = 2u32;
            pub const REG_OPTION_BACKUP_RESTORE: REG_OPEN_CREATE_OPTIONS = 4u32;
            pub const REG_OPTION_CREATE_LINK: REG_OPEN_CREATE_OPTIONS = 2u32;
            pub const REG_OPTION_NON_VOLATILE: REG_OPEN_CREATE_OPTIONS = 0u32;
            pub const REG_OPTION_OPEN_LINK: REG_OPEN_CREATE_OPTIONS = 8u32;
            pub const REG_OPTION_RESERVED: REG_OPEN_CREATE_OPTIONS = 0u32;
            pub const REG_OPTION_VOLATILE: REG_OPEN_CREATE_OPTIONS = 1u32;
            pub const REG_QWORD: REG_VALUE_TYPE = 11u32;
            pub const REG_QWORD_LITTLE_ENDIAN: REG_VALUE_TYPE = 11u32;
            pub const REG_RESOURCE_LIST: REG_VALUE_TYPE = 8u32;
            pub const REG_RESOURCE_REQUIREMENTS_LIST: REG_VALUE_TYPE = 10u32;
            pub const REG_SZ: REG_VALUE_TYPE = 1u32;
            pub const REG_WHOLE_HIVE_VOLATILE: REG_RESTORE_KEY_FLAGS = 1i32;
            #[link(name = "advapi32.dll", kind = "raw-dylib", modifiers = "+verbatim")]
            unsafe extern "system" {
                pub fn RegCloseKey(hkey: HKEY) -> super::super::Foundation::WIN32_ERROR;
                pub fn RegConnectRegistryW(
                    lpmachinename: crate::windows_sys::core::PCWSTR,
                    hkey: HKEY,
                    phkresult: *mut HKEY,
                ) -> super::super::Foundation::WIN32_ERROR;
                pub fn RegCreateKeyExW(
                    hkey: HKEY,
                    lpsubkey: crate::windows_sys::core::PCWSTR,
                    reserved: u32,
                    lpclass: crate::windows_sys::core::PCWSTR,
                    dwoptions: REG_OPEN_CREATE_OPTIONS,
                    samdesired: REG_SAM_FLAGS,
                    lpsecurityattributes: *const super::super::Security::SECURITY_ATTRIBUTES,
                    phkresult: *mut HKEY,
                    lpdwdisposition: *mut REG_CREATE_KEY_DISPOSITION,
                ) -> super::super::Foundation::WIN32_ERROR;
                pub fn RegCreateKeyW(
                    hkey: HKEY,
                    lpsubkey: crate::windows_sys::core::PCWSTR,
                    phkresult: *mut HKEY,
                ) -> super::super::Foundation::WIN32_ERROR;
                pub fn RegDeleteKeyExW(
                    hkey: HKEY,
                    lpsubkey: crate::windows_sys::core::PCWSTR,
                    samdesired: u32,
                    reserved: u32,
                ) -> super::super::Foundation::WIN32_ERROR;
                pub fn RegDeleteKeyW(
                    hkey: HKEY,
                    lpsubkey: crate::windows_sys::core::PCWSTR,
                ) -> super::super::Foundation::WIN32_ERROR;
                pub fn RegDeleteValueW(
                    hkey: HKEY,
                    lpvaluename: crate::windows_sys::core::PCWSTR,
                ) -> super::super::Foundation::WIN32_ERROR;
                pub fn RegDisableReflectionKey(
                    hbase: HKEY,
                ) -> super::super::Foundation::WIN32_ERROR;
                pub fn RegEnableReflectionKey(hbase: HKEY)
                -> super::super::Foundation::WIN32_ERROR;
                pub fn RegEnumKeyExW(
                    hkey: HKEY,
                    dwindex: u32,
                    lpname: crate::windows_sys::core::PWSTR,
                    lpcchname: *mut u32,
                    lpreserved: *const u32,
                    lpclass: crate::windows_sys::core::PWSTR,
                    lpcchclass: *mut u32,
                    lpftlastwritetime: *mut super::super::Foundation::FILETIME,
                ) -> super::super::Foundation::WIN32_ERROR;
                pub fn RegEnumValueW(
                    hkey: HKEY,
                    dwindex: u32,
                    lpvaluename: crate::windows_sys::core::PWSTR,
                    lpcchvaluename: *mut u32,
                    lpreserved: *const u32,
                    lptype: *mut u32,
                    lpdata: *mut u8,
                    lpcbdata: *mut u32,
                ) -> super::super::Foundation::WIN32_ERROR;
                pub fn RegFlushKey(hkey: HKEY) -> super::super::Foundation::WIN32_ERROR;
                pub fn RegLoadKeyW(
                    hkey: HKEY,
                    lpsubkey: crate::windows_sys::core::PCWSTR,
                    lpfile: crate::windows_sys::core::PCWSTR,
                ) -> super::super::Foundation::WIN32_ERROR;
                pub fn RegOpenKeyExW(
                    hkey: HKEY,
                    lpsubkey: crate::windows_sys::core::PCWSTR,
                    uloptions: u32,
                    samdesired: REG_SAM_FLAGS,
                    phkresult: *mut HKEY,
                ) -> super::super::Foundation::WIN32_ERROR;
                pub fn RegQueryInfoKeyW(
                    hkey: HKEY,
                    lpclass: crate::windows_sys::core::PWSTR,
                    lpcchclass: *mut u32,
                    lpreserved: *const u32,
                    lpcsubkeys: *mut u32,
                    lpcbmaxsubkeylen: *mut u32,
                    lpcbmaxclasslen: *mut u32,
                    lpcvalues: *mut u32,
                    lpcbmaxvaluenamelen: *mut u32,
                    lpcbmaxvaluelen: *mut u32,
                    lpcbsecuritydescriptor: *mut u32,
                    lpftlastwritetime: *mut super::super::Foundation::FILETIME,
                ) -> super::super::Foundation::WIN32_ERROR;
                pub fn RegQueryReflectionKey(
                    hbase: HKEY,
                    bisreflectiondisabled: *mut crate::windows_sys::core::BOOL,
                ) -> super::super::Foundation::WIN32_ERROR;
                pub fn RegQueryValueExW(
                    hkey: HKEY,
                    lpvaluename: crate::windows_sys::core::PCWSTR,
                    lpreserved: *const u32,
                    lptype: *mut REG_VALUE_TYPE,
                    lpdata: *mut u8,
                    lpcbdata: *mut u32,
                ) -> super::super::Foundation::WIN32_ERROR;
                pub fn RegSaveKeyW(
                    hkey: HKEY,
                    lpfile: crate::windows_sys::core::PCWSTR,
                    lpsecurityattributes: *const super::super::Security::SECURITY_ATTRIBUTES,
                ) -> super::super::Foundation::WIN32_ERROR;
                pub fn RegSetValueExW(
                    hkey: HKEY,
                    lpvaluename: crate::windows_sys::core::PCWSTR,
                    reserved: u32,
                    dwtype: REG_VALUE_TYPE,
                    lpdata: *const u8,
                    cbdata: u32,
                ) -> super::super::Foundation::WIN32_ERROR;
            }
        }
        pub mod Rpc {
            pub type RPC_STATUS = i32;
            pub const RPC_S_OK: RPC_STATUS = 0i32;
            pub const RPC_S_UUID_LOCAL_ONLY: RPC_STATUS = 1824i32;
            pub const RPC_S_UUID_NO_ADDRESS: RPC_STATUS = 1739i32;
            #[link(name = "rpcrt4.dll", kind = "raw-dylib", modifiers = "+verbatim")]
            unsafe extern "system" {
                pub fn UuidCreateSequential(
                    uuid: *mut crate::windows_sys::core::GUID,
                ) -> RPC_STATUS;
            }
        }
        pub mod SystemInformation {
            pub type COMPUTER_NAME_FORMAT = i32;
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct OSVERSIONINFOEXW {
                pub dwOSVersionInfoSize: u32,
                pub dwMajorVersion: u32,
                pub dwMinorVersion: u32,
                pub dwBuildNumber: u32,
                pub dwPlatformId: u32,
                pub szCSDVersion: [u16; 128],
                pub wServicePackMajor: u16,
                pub wServicePackMinor: u16,
                pub wSuiteMask: u16,
                pub wProductType: u8,
                pub wReserved: u8,
            }
            impl Default for OSVERSIONINFOEXW {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct OSVERSIONINFOW {
                pub dwOSVersionInfoSize: u32,
                pub dwMajorVersion: u32,
                pub dwMinorVersion: u32,
                pub dwBuildNumber: u32,
                pub dwPlatformId: u32,
                pub szCSDVersion: [u16; 128],
            }
            impl Default for OSVERSIONINFOW {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            pub type PROCESSOR_ARCHITECTURE = u16;
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct SYSTEM_INFO {
                pub Anonymous: SYSTEM_INFO_0,
                pub dwPageSize: u32,
                pub lpMinimumApplicationAddress: *mut core::ffi::c_void,
                pub lpMaximumApplicationAddress: *mut core::ffi::c_void,
                pub dwActiveProcessorMask: usize,
                pub dwNumberOfProcessors: u32,
                pub dwProcessorType: u32,
                pub dwAllocationGranularity: u32,
                pub wProcessorLevel: u16,
                pub wProcessorRevision: u16,
            }
            impl Default for SYSTEM_INFO {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub union SYSTEM_INFO_0 {
                pub dwOemId: u32,
                pub Anonymous: SYSTEM_INFO_0_0,
            }
            impl Default for SYSTEM_INFO_0 {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy, Default)]
            #[repr(C)]
            pub struct SYSTEM_INFO_0_0 {
                pub wProcessorArchitecture: PROCESSOR_ARCHITECTURE,
                pub wReserved: u16,
            }
            pub const ComputerNamePhysicalDnsHostname: COMPUTER_NAME_FORMAT = 5i32;
            #[link(name = "kernel32.dll", kind = "raw-dylib", modifiers = "+verbatim")]
            unsafe extern "system" {
                pub fn GetComputerNameExW(
                    nametype: COMPUTER_NAME_FORMAT,
                    lpbuffer: crate::windows_sys::core::PWSTR,
                    nsize: *mut u32,
                ) -> crate::windows_sys::core::BOOL;
                pub fn GetSystemDirectoryW(
                    lpbuffer: crate::windows_sys::core::PWSTR,
                    usize: u32,
                ) -> u32;
                pub fn GetSystemInfo(lpsysteminfo: *mut SYSTEM_INFO);
                pub fn GetSystemTimeAdjustment(
                    lptimeadjustment: *mut u32,
                    lptimeincrement: *mut u32,
                    lptimeadjustmentdisabled: *mut crate::windows_sys::core::BOOL,
                ) -> crate::windows_sys::core::BOOL;
                pub fn GetTickCount64() -> u64;
                pub fn GetVersion() -> u32;
                pub fn GetVersionExW(
                    lpversioninformation: *mut OSVERSIONINFOW,
                ) -> crate::windows_sys::core::BOOL;
            }
        }
        pub mod SystemServices {
            pub const IO_REPARSE_TAG_APPEXECLINK: u32 = 2147483675u32;
            pub const IO_REPARSE_TAG_MOUNT_POINT: u32 = 2684354563u32;
            pub const IO_REPARSE_TAG_SYMLINK: u32 = 2684354572u32;
            pub const LOCALE_NAME_MAX_LENGTH: u32 = 85u32;
            pub const MAXIMUM_WAIT_OBJECTS: u32 = 64u32;
        }
        pub mod Threading {
            #[derive(Clone, Copy, Default)]
            #[repr(C)]
            pub struct IO_COUNTERS {
                pub ReadOperationCount: u64,
                pub WriteOperationCount: u64,
                pub OtherOperationCount: u64,
                pub ReadTransferCount: u64,
                pub WriteTransferCount: u64,
                pub OtherTransferCount: u64,
            }
            pub type LPPROC_THREAD_ATTRIBUTE_LIST = *mut core::ffi::c_void;
            pub type LPTHREAD_START_ROUTINE =
                Option<unsafe extern "system" fn(lpthreadparameter: *mut core::ffi::c_void) -> u32>;
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct PEB {
                pub Reserved1: [u8; 2],
                pub BeingDebugged: u8,
                pub Reserved2: [u8; 1],
                pub Reserved3: [*mut core::ffi::c_void; 2],
                pub Ldr: *mut PEB_LDR_DATA,
                pub ProcessParameters: *mut RTL_USER_PROCESS_PARAMETERS,
                pub Reserved4: [*mut core::ffi::c_void; 3],
                pub AtlThunkSListPtr: *mut core::ffi::c_void,
                pub Reserved5: *mut core::ffi::c_void,
                pub Reserved6: u32,
                pub Reserved7: *mut core::ffi::c_void,
                pub Reserved8: u32,
                pub AtlThunkSListPtr32: u32,
                pub Reserved9: [*mut core::ffi::c_void; 45],
                pub Reserved10: [u8; 96],
                pub PostProcessInitRoutine: PPS_POST_PROCESS_INIT_ROUTINE,
                pub Reserved11: [u8; 128],
                pub Reserved12: [*mut core::ffi::c_void; 1],
                pub SessionId: u32,
            }
            impl Default for PEB {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct PEB_LDR_DATA {
                pub Reserved1: [u8; 8],
                pub Reserved2: [*mut core::ffi::c_void; 3],
                pub InMemoryOrderModuleList: super::Kernel::LIST_ENTRY,
            }
            impl Default for PEB_LDR_DATA {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            pub type PPS_POST_PROCESS_INIT_ROUTINE = Option<unsafe extern "system" fn()>;
            pub type PROCESS_ACCESS_RIGHTS = u32;
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct PROCESS_BASIC_INFORMATION {
                pub ExitStatus: super::super::Foundation::NTSTATUS,
                pub PebBaseAddress: *mut PEB,
                pub AffinityMask: usize,
                pub BasePriority: i32,
                pub UniqueProcessId: usize,
                pub InheritedFromUniqueProcessId: usize,
            }
            impl Default for PROCESS_BASIC_INFORMATION {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            pub type PROCESS_CREATION_FLAGS = u32;
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct PROCESS_INFORMATION {
                pub hProcess: super::super::Foundation::HANDLE,
                pub hThread: super::super::Foundation::HANDLE,
                pub dwProcessId: u32,
                pub dwThreadId: u32,
            }
            impl Default for PROCESS_INFORMATION {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct RTL_USER_PROCESS_PARAMETERS {
                pub Reserved1: [u8; 16],
                pub Reserved2: [*mut core::ffi::c_void; 10],
                pub ImagePathName: super::super::Foundation::UNICODE_STRING,
                pub CommandLine: super::super::Foundation::UNICODE_STRING,
            }
            impl Default for RTL_USER_PROCESS_PARAMETERS {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct STARTUPINFOEXW {
                pub StartupInfo: STARTUPINFOW,
                pub lpAttributeList: LPPROC_THREAD_ATTRIBUTE_LIST,
            }
            impl Default for STARTUPINFOEXW {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct STARTUPINFOW {
                pub cb: u32,
                pub lpReserved: crate::windows_sys::core::PWSTR,
                pub lpDesktop: crate::windows_sys::core::PWSTR,
                pub lpTitle: crate::windows_sys::core::PWSTR,
                pub dwX: u32,
                pub dwY: u32,
                pub dwXSize: u32,
                pub dwYSize: u32,
                pub dwXCountChars: u32,
                pub dwYCountChars: u32,
                pub dwFillAttribute: u32,
                pub dwFlags: STARTUPINFOW_FLAGS,
                pub wShowWindow: u16,
                pub cbReserved2: u16,
                pub lpReserved2: *mut u8,
                pub hStdInput: super::super::Foundation::HANDLE,
                pub hStdOutput: super::super::Foundation::HANDLE,
                pub hStdError: super::super::Foundation::HANDLE,
            }
            impl Default for STARTUPINFOW {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            pub type STARTUPINFOW_FLAGS = u32;
            pub type SYNCHRONIZATION_ACCESS_RIGHTS = u32;
            pub type THREAD_CREATION_FLAGS = u32;
            pub type WAITORTIMERCALLBACK =
                Option<unsafe extern "system" fn(param0: *mut core::ffi::c_void, param1: bool)>;
            pub type WORKER_THREAD_FLAGS = u32;
            pub const ABOVE_NORMAL_PRIORITY_CLASS: PROCESS_CREATION_FLAGS = 32768u32;
            pub const BELOW_NORMAL_PRIORITY_CLASS: PROCESS_CREATION_FLAGS = 16384u32;
            pub const CREATE_BREAKAWAY_FROM_JOB: PROCESS_CREATION_FLAGS = 16777216u32;
            pub const CREATE_DEFAULT_ERROR_MODE: PROCESS_CREATION_FLAGS = 67108864u32;
            pub const CREATE_NEW_CONSOLE: PROCESS_CREATION_FLAGS = 16u32;
            pub const CREATE_NEW_PROCESS_GROUP: PROCESS_CREATION_FLAGS = 512u32;
            pub const CREATE_NO_WINDOW: PROCESS_CREATION_FLAGS = 134217728u32;
            pub const CREATE_SUSPENDED: PROCESS_CREATION_FLAGS = 4u32;
            pub const CREATE_UNICODE_ENVIRONMENT: PROCESS_CREATION_FLAGS = 1024u32;
            pub const DETACHED_PROCESS: PROCESS_CREATION_FLAGS = 8u32;
            pub const EXTENDED_STARTUPINFO_PRESENT: PROCESS_CREATION_FLAGS = 524288u32;
            pub const HIGH_PRIORITY_CLASS: PROCESS_CREATION_FLAGS = 128u32;
            pub const IDLE_PRIORITY_CLASS: PROCESS_CREATION_FLAGS = 64u32;
            pub const INFINITE: u32 = 4294967295u32;
            pub const NORMAL_PRIORITY_CLASS: PROCESS_CREATION_FLAGS = 32u32;
            pub const PROCESS_ALL_ACCESS: PROCESS_ACCESS_RIGHTS = 2097151u32;
            pub const PROCESS_DUP_HANDLE: PROCESS_ACCESS_RIGHTS = 64u32;
            pub const PROCESS_QUERY_LIMITED_INFORMATION: PROCESS_ACCESS_RIGHTS = 4096u32;
            pub const PROCESS_TERMINATE: PROCESS_ACCESS_RIGHTS = 1u32;
            pub const PROC_THREAD_ATTRIBUTE_HANDLE_LIST: u32 = 131074u32;
            pub const REALTIME_PRIORITY_CLASS: PROCESS_CREATION_FLAGS = 256u32;
            pub const STARTF_FORCEOFFFEEDBACK: STARTUPINFOW_FLAGS = 128u32;
            pub const STARTF_FORCEONFEEDBACK: STARTUPINFOW_FLAGS = 64u32;
            pub const STARTF_PREVENTPINNING: STARTUPINFOW_FLAGS = 8192u32;
            pub const STARTF_RUNFULLSCREEN: STARTUPINFOW_FLAGS = 32u32;
            pub const STARTF_TITLEISAPPID: STARTUPINFOW_FLAGS = 4096u32;
            pub const STARTF_TITLEISLINKNAME: STARTUPINFOW_FLAGS = 2048u32;
            pub const STARTF_UNTRUSTEDSOURCE: STARTUPINFOW_FLAGS = 32768u32;
            pub const STARTF_USECOUNTCHARS: STARTUPINFOW_FLAGS = 8u32;
            pub const STARTF_USEFILLATTRIBUTE: STARTUPINFOW_FLAGS = 16u32;
            pub const STARTF_USEHOTKEY: STARTUPINFOW_FLAGS = 512u32;
            pub const STARTF_USEPOSITION: STARTUPINFOW_FLAGS = 4u32;
            pub const STARTF_USESHOWWINDOW: STARTUPINFOW_FLAGS = 1u32;
            pub const STARTF_USESIZE: STARTUPINFOW_FLAGS = 2u32;
            pub const STARTF_USESTDHANDLES: STARTUPINFOW_FLAGS = 256u32;
            pub const WT_EXECUTEINWAITTHREAD: WORKER_THREAD_FLAGS = 4u32;
            pub const WT_EXECUTEONLYONCE: WORKER_THREAD_FLAGS = 8u32;
            #[link(name = "advapi32.dll", kind = "raw-dylib", modifiers = "+verbatim")]
            unsafe extern "system" {
                pub fn OpenProcessToken(
                    processhandle: super::super::Foundation::HANDLE,
                    desiredaccess: super::super::Security::TOKEN_ACCESS_MASK,
                    tokenhandle: *mut super::super::Foundation::HANDLE,
                ) -> crate::windows_sys::core::BOOL;
            }
            #[link(name = "kernel32.dll", kind = "raw-dylib", modifiers = "+verbatim")]
            unsafe extern "system" {
                pub fn CreateEventW(
                    lpeventattributes: *const super::super::Security::SECURITY_ATTRIBUTES,
                    bmanualreset: crate::windows_sys::core::BOOL,
                    binitialstate: crate::windows_sys::core::BOOL,
                    lpname: crate::windows_sys::core::PCWSTR,
                ) -> super::super::Foundation::HANDLE;
                pub fn CreateMutexW(
                    lpmutexattributes: *const super::super::Security::SECURITY_ATTRIBUTES,
                    binitialowner: crate::windows_sys::core::BOOL,
                    lpname: crate::windows_sys::core::PCWSTR,
                ) -> super::super::Foundation::HANDLE;
                pub fn CreateProcessW(
                    lpapplicationname: crate::windows_sys::core::PCWSTR,
                    lpcommandline: crate::windows_sys::core::PWSTR,
                    lpprocessattributes: *const super::super::Security::SECURITY_ATTRIBUTES,
                    lpthreadattributes: *const super::super::Security::SECURITY_ATTRIBUTES,
                    binherithandles: crate::windows_sys::core::BOOL,
                    dwcreationflags: PROCESS_CREATION_FLAGS,
                    lpenvironment: *const core::ffi::c_void,
                    lpcurrentdirectory: crate::windows_sys::core::PCWSTR,
                    lpstartupinfo: *const STARTUPINFOW,
                    lpprocessinformation: *mut PROCESS_INFORMATION,
                ) -> crate::windows_sys::core::BOOL;
                pub fn CreateThread(
                    lpthreadattributes: *const super::super::Security::SECURITY_ATTRIBUTES,
                    dwstacksize: usize,
                    lpstartaddress: LPTHREAD_START_ROUTINE,
                    lpparameter: *const core::ffi::c_void,
                    dwcreationflags: THREAD_CREATION_FLAGS,
                    lpthreadid: *mut u32,
                ) -> super::super::Foundation::HANDLE;
                pub fn DeleteProcThreadAttributeList(lpattributelist: LPPROC_THREAD_ATTRIBUTE_LIST);
                pub fn ExitProcess(uexitcode: u32) -> !;
                pub fn GetCurrentProcess() -> super::super::Foundation::HANDLE;
                pub fn GetCurrentThread() -> super::super::Foundation::HANDLE;
                pub fn GetCurrentThreadStackLimits(lowlimit: *mut usize, highlimit: *mut usize);
                pub fn GetExitCodeProcess(
                    hprocess: super::super::Foundation::HANDLE,
                    lpexitcode: *mut u32,
                ) -> crate::windows_sys::core::BOOL;
                pub fn GetExitCodeThread(
                    hthread: super::super::Foundation::HANDLE,
                    lpexitcode: *mut u32,
                ) -> crate::windows_sys::core::BOOL;
                pub fn GetProcessTimes(
                    hprocess: super::super::Foundation::HANDLE,
                    lpcreationtime: *mut super::super::Foundation::FILETIME,
                    lpexittime: *mut super::super::Foundation::FILETIME,
                    lpkerneltime: *mut super::super::Foundation::FILETIME,
                    lpusertime: *mut super::super::Foundation::FILETIME,
                ) -> crate::windows_sys::core::BOOL;
                pub fn GetThreadTimes(
                    hthread: super::super::Foundation::HANDLE,
                    lpcreationtime: *mut super::super::Foundation::FILETIME,
                    lpexittime: *mut super::super::Foundation::FILETIME,
                    lpkerneltime: *mut super::super::Foundation::FILETIME,
                    lpusertime: *mut super::super::Foundation::FILETIME,
                ) -> crate::windows_sys::core::BOOL;
                pub fn InitializeProcThreadAttributeList(
                    lpattributelist: LPPROC_THREAD_ATTRIBUTE_LIST,
                    dwattributecount: u32,
                    dwflags: u32,
                    lpsize: *mut usize,
                ) -> crate::windows_sys::core::BOOL;
                pub fn OpenEventW(
                    dwdesiredaccess: SYNCHRONIZATION_ACCESS_RIGHTS,
                    binherithandle: crate::windows_sys::core::BOOL,
                    lpname: crate::windows_sys::core::PCWSTR,
                ) -> super::super::Foundation::HANDLE;
                pub fn OpenMutexW(
                    dwdesiredaccess: SYNCHRONIZATION_ACCESS_RIGHTS,
                    binherithandle: crate::windows_sys::core::BOOL,
                    lpname: crate::windows_sys::core::PCWSTR,
                ) -> super::super::Foundation::HANDLE;
                pub fn OpenProcess(
                    dwdesiredaccess: PROCESS_ACCESS_RIGHTS,
                    binherithandle: crate::windows_sys::core::BOOL,
                    dwprocessid: u32,
                ) -> super::super::Foundation::HANDLE;
                pub fn RegisterWaitForSingleObject(
                    phnewwaitobject: *mut super::super::Foundation::HANDLE,
                    hobject: super::super::Foundation::HANDLE,
                    callback: WAITORTIMERCALLBACK,
                    context: *const core::ffi::c_void,
                    dwmilliseconds: u32,
                    dwflags: WORKER_THREAD_FLAGS,
                ) -> crate::windows_sys::core::BOOL;
                pub fn ReleaseMutex(
                    hmutex: super::super::Foundation::HANDLE,
                ) -> crate::windows_sys::core::BOOL;
                pub fn ResetEvent(
                    hevent: super::super::Foundation::HANDLE,
                ) -> crate::windows_sys::core::BOOL;
                pub fn ResumeThread(hthread: super::super::Foundation::HANDLE) -> u32;
                pub fn SetEvent(
                    hevent: super::super::Foundation::HANDLE,
                ) -> crate::windows_sys::core::BOOL;
                pub fn SetThreadStackGuarantee(
                    stacksizeinbytes: *mut u32,
                ) -> crate::windows_sys::core::BOOL;
                pub fn TerminateProcess(
                    hprocess: super::super::Foundation::HANDLE,
                    uexitcode: u32,
                ) -> crate::windows_sys::core::BOOL;
                pub fn TerminateThread(
                    hthread: super::super::Foundation::HANDLE,
                    dwexitcode: u32,
                ) -> crate::windows_sys::core::BOOL;
                pub fn UnregisterWait(
                    waithandle: super::super::Foundation::HANDLE,
                ) -> crate::windows_sys::core::BOOL;
                pub fn UnregisterWaitEx(
                    waithandle: super::super::Foundation::HANDLE,
                    completionevent: super::super::Foundation::HANDLE,
                ) -> crate::windows_sys::core::BOOL;
                pub fn UpdateProcThreadAttribute(
                    lpattributelist: LPPROC_THREAD_ATTRIBUTE_LIST,
                    dwflags: u32,
                    attribute: usize,
                    lpvalue: *const core::ffi::c_void,
                    cbsize: usize,
                    lppreviousvalue: *mut core::ffi::c_void,
                    lpreturnsize: *const usize,
                ) -> crate::windows_sys::core::BOOL;
                pub fn WaitForMultipleObjects(
                    ncount: u32,
                    lphandles: *const super::super::Foundation::HANDLE,
                    bwaitall: crate::windows_sys::core::BOOL,
                    dwmilliseconds: u32,
                ) -> super::super::Foundation::WAIT_EVENT;
                pub fn WaitForSingleObject(
                    hhandle: super::super::Foundation::HANDLE,
                    dwmilliseconds: u32,
                ) -> super::super::Foundation::WAIT_EVENT;
            }
        }
        pub mod Time {
            #[derive(Clone, Copy)]
            #[repr(C)]
            pub struct TIME_ZONE_INFORMATION {
                pub Bias: i32,
                pub StandardName: [u16; 32],
                pub StandardDate: super::super::Foundation::SYSTEMTIME,
                pub StandardBias: i32,
                pub DaylightName: [u16; 32],
                pub DaylightDate: super::super::Foundation::SYSTEMTIME,
                pub DaylightBias: i32,
            }
            impl Default for TIME_ZONE_INFORMATION {
                fn default() -> Self {
                    unsafe { core::mem::zeroed() }
                }
            }
            #[link(name = "kernel32.dll", kind = "raw-dylib", modifiers = "+verbatim")]
            unsafe extern "system" {
                pub fn FileTimeToSystemTime(
                    lpfiletime: *const super::super::Foundation::FILETIME,
                    lpsystemtime: *mut super::super::Foundation::SYSTEMTIME,
                ) -> crate::windows_sys::core::BOOL;
                pub fn GetTimeZoneInformation(
                    lptimezoneinformation: *mut TIME_ZONE_INFORMATION,
                ) -> u32;
                pub fn SystemTimeToFileTime(
                    lpsystemtime: *const super::super::Foundation::SYSTEMTIME,
                    lpfiletime: *mut super::super::Foundation::FILETIME,
                ) -> crate::windows_sys::core::BOOL;
                pub fn SystemTimeToTzSpecificLocalTime(
                    lptimezoneinformation: *const TIME_ZONE_INFORMATION,
                    lpuniversaltime: *const super::super::Foundation::SYSTEMTIME,
                    lplocaltime: *mut super::super::Foundation::SYSTEMTIME,
                ) -> crate::windows_sys::core::BOOL;
            }
        }
        pub mod WindowsProgramming {
            pub const DRIVE_FIXED: u32 = 3u32;
            #[link(name = "advapi32.dll", kind = "raw-dylib", modifiers = "+verbatim")]
            unsafe extern "system" {
                pub fn GetUserNameW(
                    lpbuffer: crate::windows_sys::core::PWSTR,
                    pcbbuffer: *mut u32,
                ) -> crate::windows_sys::core::BOOL;
            }
        }
    }
    pub mod UI {
        pub mod Shell {
            #[link(
                name = "api-ms-win-core-path-l1-1-0.dll",
                kind = "raw-dylib",
                modifiers = "+verbatim"
            )]
            unsafe extern "system" {
                pub fn PathCchSkipRoot(
                    pszpath: crate::windows_sys::core::PCWSTR,
                    ppszrootend: *mut crate::windows_sys::core::PCWSTR,
                ) -> crate::windows_sys::core::HRESULT;
            }
            #[link(name = "shell32.dll", kind = "raw-dylib", modifiers = "+verbatim")]
            unsafe extern "system" {
                pub fn ShellExecuteW(
                    hwnd: super::super::Foundation::HWND,
                    lpoperation: crate::windows_sys::core::PCWSTR,
                    lpfile: crate::windows_sys::core::PCWSTR,
                    lpparameters: crate::windows_sys::core::PCWSTR,
                    lpdirectory: crate::windows_sys::core::PCWSTR,
                    nshowcmd: super::WindowsAndMessaging::SHOW_WINDOW_CMD,
                ) -> super::super::Foundation::HINSTANCE;
            }
        }
        pub mod WindowsAndMessaging {
            pub type MESSAGEBOX_STYLE = u32;
            pub type SHOW_WINDOW_CMD = i32;
            pub const MB_ICONASTERISK: MESSAGEBOX_STYLE = 64u32;
            pub const MB_ICONERROR: MESSAGEBOX_STYLE = 16u32;
            pub const MB_ICONEXCLAMATION: MESSAGEBOX_STYLE = 48u32;
            pub const MB_ICONHAND: MESSAGEBOX_STYLE = 16u32;
            pub const MB_ICONINFORMATION: MESSAGEBOX_STYLE = 64u32;
            pub const MB_ICONQUESTION: MESSAGEBOX_STYLE = 32u32;
            pub const MB_ICONSTOP: MESSAGEBOX_STYLE = 16u32;
            pub const MB_ICONWARNING: MESSAGEBOX_STYLE = 48u32;
            pub const MB_OK: MESSAGEBOX_STYLE = 0u32;
            pub const SW_HIDE: SHOW_WINDOW_CMD = 0i32;
        }
    }
}
pub mod core {
    pub use literals::*;
    pub type BOOL = i32;
    pub type BSTR = *const u16;
    #[derive(Clone, Copy, Default)]
    #[repr(C)]
    pub struct GUID {
        pub data1: u32,
        pub data2: u16,
        pub data3: u16,
        pub data4: [u8; 8],
    }
    impl GUID {
        pub const fn from_u128(uuid: u128) -> Self {
            Self {
                data1: (uuid >> 96) as u32,
                data2: (uuid >> 80 & 0xffff) as u16,
                data3: (uuid >> 64 & 0xffff) as u16,
                data4: (uuid as u64).to_be_bytes(),
            }
        }
    }
    pub type HRESULT = i32;
    pub type PCSTR = *const u8;
    pub type PCWSTR = *const u16;
    pub type PSTR = *mut u8;
    pub type PWSTR = *mut u16;
    pub mod literals {
        pub const fn decode_utf8_char(bytes: &[u8], mut pos: usize) -> Option<(u32, usize)> {
            if bytes.len() == pos {
                return None;
            }
            let ch = bytes[pos] as u32;
            pos += 1;
            if ch <= 0x7f {
                return Some((ch, pos));
            }
            if (ch & 0xe0) == 0xc0 {
                if bytes.len() - pos < 1 {
                    return None;
                }
                let ch2 = bytes[pos] as u32;
                pos += 1;
                if (ch2 & 0xc0) != 0x80 {
                    return None;
                }
                let result: u32 = ((ch & 0x1f) << 6) | (ch2 & 0x3f);
                if result <= 0x7f {
                    return None;
                }
                return Some((result, pos));
            }
            if (ch & 0xf0) == 0xe0 {
                if bytes.len() - pos < 2 {
                    return None;
                }
                let ch2 = bytes[pos] as u32;
                pos += 1;
                let ch3 = bytes[pos] as u32;
                pos += 1;
                if (ch2 & 0xc0) != 0x80 || (ch3 & 0xc0) != 0x80 {
                    return None;
                }
                let result = ((ch & 0x0f) << 12) | ((ch2 & 0x3f) << 6) | (ch3 & 0x3f);
                if result <= 0x7ff || (0xd800 <= result && result <= 0xdfff) {
                    return None;
                }
                return Some((result, pos));
            }
            if (ch & 0xf8) == 0xf0 {
                if bytes.len() - pos < 3 {
                    return None;
                }
                let ch2 = bytes[pos] as u32;
                pos += 1;
                let ch3 = bytes[pos] as u32;
                pos += 1;
                let ch4 = bytes[pos] as u32;
                pos += 1;
                if (ch2 & 0xc0) != 0x80 || (ch3 & 0xc0) != 0x80 || (ch4 & 0xc0) != 0x80 {
                    return None;
                }
                let result =
                    ((ch & 0x07) << 18) | ((ch2 & 0x3f) << 12) | ((ch3 & 0x3f) << 6) | (ch4 & 0x3f);
                if result <= 0xffff || 0x10ffff < result {
                    return None;
                }
                return Some((result, pos));
            }
            None
        }
        pub const fn utf16_len(bytes: &[u8]) -> usize {
            let mut pos = 0;
            let mut len = 0;
            while let Some((code_point, new_pos)) = decode_utf8_char(bytes, pos) {
                pos = new_pos;
                len += if code_point <= 0xffff { 1 } else { 2 };
            }
            len
        }
    }
}
