//! Windows declarations: every function, struct, and constant sys-lib calls on
//! Windows, written by hand from the Windows SDK headers named beside each item.
//!
//! Imports use `raw-dylib`, so no import library is needed at link time (on
//! 32-bit x86 the names are undecorated, as the system DLLs export them).
//! Structs mirror the C layout with `#[repr(C)]`; each has a size and alignment
//! assertion for 64-bit and 32-bit targets, and the tests at the bottom compare
//! them with `windows-sys` field by field.

#![expect(
    dead_code,
    reason = "the declaration list is locked in 0.2.0; each item is used by the reading that \
              arrives in 0.3 to 0.5 (docs/API.md), and this expectation goes when the last one is"
)]

use core::ffi::c_void;

/// `BOOL`: nonzero is true.
pub(crate) type Bool = i32;
/// `HANDLE`.
pub(crate) type Handle = *mut c_void;
/// `HKEY`.
pub(crate) type Hkey = *mut c_void;
/// `NTSTATUS`: zero or positive is success.
pub(crate) type NtStatus = i32;

/// `INVALID_HANDLE_VALUE` (`(HANDLE)-1`).
pub(crate) const INVALID_HANDLE_VALUE: Handle = core::ptr::without_provenance_mut(usize::MAX);

// Win32 error codes (winerror.h).
pub(crate) const ERROR_SUCCESS: u32 = 0;
pub(crate) const ERROR_FILE_NOT_FOUND: u32 = 2;
pub(crate) const ERROR_ACCESS_DENIED: u32 = 5;
pub(crate) const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
pub(crate) const ERROR_MORE_DATA: u32 = 234;
pub(crate) const ERROR_BUFFER_OVERFLOW: u32 = 111;
pub(crate) const ERROR_NO_MORE_FILES: u32 = 18;
pub(crate) const ERROR_NOT_READY: u32 = 21;

/// `STATUS_INFO_LENGTH_MISMATCH` (ntstatus.h).
pub(crate) const STATUS_INFO_LENGTH_MISMATCH: NtStatus = 0xC000_0004_u32 as i32;

macro_rules! dll {
    ($lib:literal { $($item:tt)* }) => {
        #[cfg_attr(
            target_arch = "x86",
            link(name = $lib, kind = "raw-dylib", modifiers = "+verbatim", import_name_type = "undecorated")
        )]
        #[cfg_attr(not(target_arch = "x86"), link(name = $lib, kind = "raw-dylib", modifiers = "+verbatim"))]
        unsafe extern "system" { $($item)* }
    };
}

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

/// `FILETIME` (minwinbase.h): 100 ns intervals, split in two halves.
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub(crate) struct FileTime {
    pub(crate) low: u32,
    pub(crate) high: u32,
}
layout!(FileTime, size = 8, align = 4);

impl FileTime {
    /// The 64-bit value.
    pub(crate) const fn get(self) -> u64 {
        ((self.high as u64) << 32) | self.low as u64
    }
}

/// `MEMORYSTATUSEX` (sysinfoapi.h).
#[repr(C)]
pub(crate) struct MemoryStatusEx {
    pub(crate) length: u32,
    pub(crate) memory_load: u32,
    pub(crate) total_phys: u64,
    pub(crate) avail_phys: u64,
    pub(crate) total_page_file: u64,
    pub(crate) avail_page_file: u64,
    pub(crate) total_virtual: u64,
    pub(crate) avail_virtual: u64,
    pub(crate) avail_extended_virtual: u64,
}
layout!(MemoryStatusEx, size = 64, align = 8);

/// `SYSTEM_INFO` (sysinfoapi.h). The leading `DWORD`/`{WORD, WORD}` union is
/// declared as its two `WORD` halves.
#[repr(C)]
pub(crate) struct SystemInfo {
    pub(crate) processor_architecture: u16,
    pub(crate) reserved: u16,
    pub(crate) page_size: u32,
    pub(crate) minimum_application_address: *mut c_void,
    pub(crate) maximum_application_address: *mut c_void,
    pub(crate) active_processor_mask: usize,
    pub(crate) number_of_processors: u32,
    pub(crate) processor_type: u32,
    pub(crate) allocation_granularity: u32,
    pub(crate) processor_level: u16,
    pub(crate) processor_revision: u16,
}
#[cfg(target_pointer_width = "64")]
layout!(SystemInfo, size = 48, align = 8);
#[cfg(target_pointer_width = "32")]
layout!(SystemInfo, size = 36, align = 4);

/// `RTL_OSVERSIONINFOEXW` / `OSVERSIONINFOEXW` (winnt.h).
#[repr(C)]
pub(crate) struct OsVersionInfoExW {
    pub(crate) os_version_info_size: u32,
    pub(crate) major_version: u32,
    pub(crate) minor_version: u32,
    pub(crate) build_number: u32,
    pub(crate) platform_id: u32,
    pub(crate) csd_version: [u16; 128],
    pub(crate) service_pack_major: u16,
    pub(crate) service_pack_minor: u16,
    pub(crate) suite_mask: u16,
    pub(crate) product_type: u8,
    pub(crate) reserved: u8,
}
layout!(OsVersionInfoExW, size = 284, align = 4);

/// `wProductType` values (winnt.h).
pub(crate) const VER_NT_WORKSTATION: u8 = 1;
pub(crate) const VER_NT_DOMAIN_CONTROLLER: u8 = 2;
pub(crate) const VER_NT_SERVER: u8 = 3;

/// `PROCESS_MEMORY_COUNTERS_EX` (psapi.h).
#[repr(C)]
pub(crate) struct ProcessMemoryCountersEx {
    pub(crate) cb: u32,
    pub(crate) page_fault_count: u32,
    pub(crate) peak_working_set_size: usize,
    pub(crate) working_set_size: usize,
    pub(crate) quota_peak_paged_pool_usage: usize,
    pub(crate) quota_paged_pool_usage: usize,
    pub(crate) quota_peak_non_paged_pool_usage: usize,
    pub(crate) quota_non_paged_pool_usage: usize,
    pub(crate) pagefile_usage: usize,
    pub(crate) peak_pagefile_usage: usize,
    pub(crate) private_usage: usize,
}
#[cfg(target_pointer_width = "64")]
layout!(ProcessMemoryCountersEx, size = 80, align = 8);
#[cfg(target_pointer_width = "32")]
layout!(ProcessMemoryCountersEx, size = 44, align = 4);

/// `PERFORMANCE_INFORMATION` (psapi.h). Counts are in pages of `page_size`.
#[repr(C)]
pub(crate) struct PerformanceInformation {
    pub(crate) cb: u32,
    pub(crate) commit_total: usize,
    pub(crate) commit_limit: usize,
    pub(crate) commit_peak: usize,
    pub(crate) physical_total: usize,
    pub(crate) physical_available: usize,
    pub(crate) system_cache: usize,
    pub(crate) kernel_total: usize,
    pub(crate) kernel_paged: usize,
    pub(crate) kernel_nonpaged: usize,
    pub(crate) page_size: usize,
    pub(crate) handle_count: u32,
    pub(crate) process_count: u32,
    pub(crate) thread_count: u32,
}
#[cfg(target_pointer_width = "64")]
layout!(PerformanceInformation, size = 104, align = 8);
#[cfg(target_pointer_width = "32")]
layout!(PerformanceInformation, size = 56, align = 4);

/// `IO_COUNTERS` (winnt.h).
#[repr(C)]
pub(crate) struct IoCounters {
    pub(crate) read_operation_count: u64,
    pub(crate) write_operation_count: u64,
    pub(crate) other_operation_count: u64,
    pub(crate) read_transfer_count: u64,
    pub(crate) write_transfer_count: u64,
    pub(crate) other_transfer_count: u64,
}
layout!(IoCounters, size = 48, align = 8);

/// `JOBOBJECT_BASIC_LIMIT_INFORMATION` (winnt.h).
#[repr(C)]
pub(crate) struct JobObjectBasicLimitInformation {
    pub(crate) per_process_user_time_limit: i64,
    pub(crate) per_job_user_time_limit: i64,
    pub(crate) limit_flags: u32,
    pub(crate) minimum_working_set_size: usize,
    pub(crate) maximum_working_set_size: usize,
    pub(crate) active_process_limit: u32,
    pub(crate) affinity: usize,
    pub(crate) priority_class: u32,
    pub(crate) scheduling_class: u32,
}
#[cfg(target_pointer_width = "64")]
layout!(JobObjectBasicLimitInformation, size = 64, align = 8);
#[cfg(target_pointer_width = "32")]
layout!(JobObjectBasicLimitInformation, size = 48, align = 8);

/// `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` (winnt.h).
#[repr(C)]
pub(crate) struct JobObjectExtendedLimitInformation {
    pub(crate) basic_limit_information: JobObjectBasicLimitInformation,
    pub(crate) io_info: IoCounters,
    pub(crate) process_memory_limit: usize,
    pub(crate) job_memory_limit: usize,
    pub(crate) peak_process_memory_used: usize,
    pub(crate) peak_job_memory_used: usize,
}
#[cfg(target_pointer_width = "64")]
layout!(JobObjectExtendedLimitInformation, size = 144, align = 8);
#[cfg(target_pointer_width = "32")]
layout!(JobObjectExtendedLimitInformation, size = 112, align = 8);

/// `JOBOBJECT_CPU_RATE_CONTROL_INFORMATION` (winnt.h). The union after the flags
/// is `CpuRate`, `Weight`, or `{MinRate, MaxRate}`; it is read as one `u32`.
#[repr(C)]
pub(crate) struct JobObjectCpuRateControlInformation {
    pub(crate) control_flags: u32,
    pub(crate) value: u32,
}
layout!(JobObjectCpuRateControlInformation, size = 8, align = 4);

// JOBOBJECTINFOCLASS (winnt.h) and limit flags.
pub(crate) const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
pub(crate) const JOB_OBJECT_CPU_RATE_CONTROL_INFORMATION: i32 = 15;
pub(crate) const JOB_OBJECT_LIMIT_PROCESS_MEMORY: u32 = 0x0000_0100;
pub(crate) const JOB_OBJECT_LIMIT_JOB_MEMORY: u32 = 0x0000_0200;
pub(crate) const JOB_OBJECT_CPU_RATE_CONTROL_ENABLE: u32 = 0x1;
pub(crate) const JOB_OBJECT_CPU_RATE_CONTROL_WEIGHT_BASED: u32 = 0x2;
pub(crate) const JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP: u32 = 0x4;
pub(crate) const JOB_OBJECT_CPU_RATE_CONTROL_MIN_MAX_RATE: u32 = 0x10;

/// `SYSTEM_POWER_STATUS` (winbase.h).
#[repr(C)]
pub(crate) struct SystemPowerStatus {
    pub(crate) ac_line_status: u8,
    pub(crate) battery_flag: u8,
    pub(crate) battery_life_percent: u8,
    pub(crate) system_status_flag: u8,
    pub(crate) battery_life_time: u32,
    pub(crate) battery_full_life_time: u32,
}
layout!(SystemPowerStatus, size = 12, align = 4);

/// `PROCESSOR_POWER_INFORMATION` (powrprof.h), one per logical processor.
#[repr(C)]
pub(crate) struct ProcessorPowerInformation {
    pub(crate) number: u32,
    pub(crate) max_mhz: u32,
    pub(crate) current_mhz: u32,
    pub(crate) mhz_limit: u32,
    pub(crate) max_idle_state: u32,
    pub(crate) current_idle_state: u32,
}
layout!(ProcessorPowerInformation, size = 24, align = 4);

/// `POWER_INFORMATION_LEVEL::ProcessorInformation` (winnt.h).
pub(crate) const PROCESSOR_INFORMATION: i32 = 11;
/// `POWER_PLATFORM_ROLE_V2` (powerbase.h).
pub(crate) const POWER_PLATFORM_ROLE_V2: u32 = 2;
// POWER_PLATFORM_ROLE values (winnt.h).
pub(crate) const PLATFORM_ROLE_UNSPECIFIED: i32 = 0;
pub(crate) const PLATFORM_ROLE_DESKTOP: i32 = 1;
pub(crate) const PLATFORM_ROLE_MOBILE: i32 = 2;
pub(crate) const PLATFORM_ROLE_WORKSTATION: i32 = 3;
pub(crate) const PLATFORM_ROLE_ENTERPRISE_SERVER: i32 = 4;
pub(crate) const PLATFORM_ROLE_SOHO_SERVER: i32 = 5;
pub(crate) const PLATFORM_ROLE_APPLIANCE_PC: i32 = 6;
pub(crate) const PLATFORM_ROLE_PERFORMANCE_SERVER: i32 = 7;
pub(crate) const PLATFORM_ROLE_SLATE: i32 = 8;

/// `SYSTEMTIME` (minwinbase.h).
#[repr(C)]
pub(crate) struct SystemTime {
    pub(crate) year: u16,
    pub(crate) month: u16,
    pub(crate) day_of_week: u16,
    pub(crate) day: u16,
    pub(crate) hour: u16,
    pub(crate) minute: u16,
    pub(crate) second: u16,
    pub(crate) milliseconds: u16,
}
layout!(SystemTime, size = 16, align = 2);

/// `DYNAMIC_TIME_ZONE_INFORMATION` (timezoneapi.h).
#[repr(C)]
pub(crate) struct DynamicTimeZoneInformation {
    pub(crate) bias: i32,
    pub(crate) standard_name: [u16; 32],
    pub(crate) standard_date: SystemTime,
    pub(crate) standard_bias: i32,
    pub(crate) daylight_name: [u16; 32],
    pub(crate) daylight_date: SystemTime,
    pub(crate) daylight_bias: i32,
    pub(crate) time_zone_key_name: [u16; 128],
    /// `BOOLEAN`.
    pub(crate) dynamic_daylight_time_disabled: u8,
}
layout!(DynamicTimeZoneInformation, size = 432, align = 4);

/// `GetDynamicTimeZoneInformation` results (timezoneapi.h).
pub(crate) const TIME_ZONE_ID_INVALID: u32 = 0xFFFF_FFFF;
pub(crate) const TIME_ZONE_ID_DAYLIGHT: u32 = 2;

/// `PROCESSENTRY32W` (tlhelp32.h).
#[repr(C)]
pub(crate) struct ProcessEntry32W {
    pub(crate) size: u32,
    pub(crate) cnt_usage: u32,
    pub(crate) th32_process_id: u32,
    pub(crate) th32_default_heap_id: usize,
    pub(crate) th32_module_id: u32,
    pub(crate) cnt_threads: u32,
    pub(crate) th32_parent_process_id: u32,
    pub(crate) pc_pri_class_base: i32,
    pub(crate) flags: u32,
    pub(crate) exe_file: [u16; 260],
}
#[cfg(target_pointer_width = "64")]
layout!(ProcessEntry32W, size = 568, align = 8);
#[cfg(target_pointer_width = "32")]
layout!(ProcessEntry32W, size = 556, align = 4);

/// `THREADENTRY32` (tlhelp32.h).
#[repr(C)]
pub(crate) struct ThreadEntry32 {
    pub(crate) size: u32,
    pub(crate) cnt_usage: u32,
    pub(crate) th32_thread_id: u32,
    pub(crate) th32_owner_process_id: u32,
    pub(crate) tp_base_pri: i32,
    pub(crate) tp_delta_pri: i32,
    pub(crate) flags: u32,
}
layout!(ThreadEntry32, size = 28, align = 4);

// CreateToolhelp32Snapshot flags (tlhelp32.h).
pub(crate) const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
pub(crate) const TH32CS_SNAPTHREAD: u32 = 0x0000_0004;

/// `SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION` (winternl.h), one per logical
/// processor. `idle` is part of `kernel`. The two trailing fields are the DPC
/// and interrupt times and the interrupt count (documented as reserved).
#[repr(C)]
pub(crate) struct SystemProcessorPerformanceInformation {
    pub(crate) idle_time: i64,
    pub(crate) kernel_time: i64,
    pub(crate) user_time: i64,
    pub(crate) dpc_and_interrupt_time: [i64; 2],
    pub(crate) interrupt_count: u32,
}
layout!(SystemProcessorPerformanceInformation, size = 48, align = 8);

/// `SYSTEM_INFORMATION_CLASS::SystemProcessorPerformanceInformation` (winternl.h).
pub(crate) const SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION: i32 = 8;

/// `TOKEN_ELEVATION` (winnt.h).
#[repr(C)]
pub(crate) struct TokenElevation {
    pub(crate) token_is_elevated: u32,
}
layout!(TokenElevation, size = 4, align = 4);

/// `TOKEN_INFORMATION_CLASS::TokenElevation` (winnt.h).
pub(crate) const TOKEN_ELEVATION_CLASS: i32 = 20;
/// `TOKEN_QUERY` access right (winnt.h).
pub(crate) const TOKEN_QUERY: u32 = 0x0008;

/// `STORAGE_PROPERTY_QUERY` (winioctl.h).
#[repr(C)]
pub(crate) struct StoragePropertyQuery {
    pub(crate) property_id: i32,
    pub(crate) query_type: i32,
    pub(crate) additional_parameters: [u8; 1],
}
layout!(StoragePropertyQuery, size = 12, align = 4);

/// `DEVICE_SEEK_PENALTY_DESCRIPTOR` (winioctl.h).
#[repr(C)]
pub(crate) struct DeviceSeekPenaltyDescriptor {
    pub(crate) version: u32,
    pub(crate) size: u32,
    /// `BOOLEAN`.
    pub(crate) incurs_seek_penalty: u8,
}
layout!(DeviceSeekPenaltyDescriptor, size = 12, align = 4);

/// `IOCTL_STORAGE_QUERY_PROPERTY` (winioctl.h).
pub(crate) const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;
/// `STORAGE_PROPERTY_ID::StorageDeviceSeekPenaltyProperty` (winioctl.h).
pub(crate) const STORAGE_DEVICE_SEEK_PENALTY_PROPERTY: i32 = 7;
/// `STORAGE_QUERY_TYPE::PropertyStandardQuery` (winioctl.h).
pub(crate) const PROPERTY_STANDARD_QUERY: i32 = 0;

/// `GROUP_AFFINITY` (winnt.h).
#[repr(C)]
pub(crate) struct GroupAffinity {
    pub(crate) mask: usize,
    pub(crate) group: u16,
    pub(crate) reserved: [u16; 3],
}
#[cfg(target_pointer_width = "64")]
layout!(GroupAffinity, size = 16, align = 8);
#[cfg(target_pointer_width = "32")]
layout!(GroupAffinity, size = 12, align = 4);

/// `GetLogicalProcessorInformationEx` returns variable-size records; they are
/// parsed from bytes in safe Rust. These are the record offsets (winnt.h
/// `SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX` and its relationship structs),
/// checked against `windows-sys` in the tests below.
pub(crate) mod lpi {
    /// `Relationship` (`LOGICAL_PROCESSOR_RELATIONSHIP`, an `i32`).
    pub(crate) const RELATIONSHIP: usize = 0;
    /// `Size` of this record in bytes.
    pub(crate) const SIZE: usize = 4;
    /// Start of the relationship union.
    #[cfg(target_pointer_width = "64")]
    pub(crate) const BODY: usize = 8;
    #[cfg(target_pointer_width = "32")]
    pub(crate) const BODY: usize = 8;

    // PROCESSOR_RELATIONSHIP, relative to BODY.
    pub(crate) const PROCESSOR_FLAGS: usize = 0;
    pub(crate) const PROCESSOR_EFFICIENCY_CLASS: usize = 1;
    pub(crate) const PROCESSOR_GROUP_COUNT: usize = 22;
    #[cfg(target_pointer_width = "64")]
    pub(crate) const PROCESSOR_GROUP_MASK: usize = 24;
    #[cfg(target_pointer_width = "32")]
    pub(crate) const PROCESSOR_GROUP_MASK: usize = 24;

    // CACHE_RELATIONSHIP, relative to BODY.
    pub(crate) const CACHE_LEVEL: usize = 0;
    pub(crate) const CACHE_LINE_SIZE: usize = 2;
    pub(crate) const CACHE_SIZE: usize = 4;
    pub(crate) const CACHE_TYPE: usize = 8;
    pub(crate) const CACHE_GROUP_COUNT: usize = 30;
    pub(crate) const CACHE_GROUP_MASK: usize = 32;

    // NUMA_NODE_RELATIONSHIP, relative to BODY.
    pub(crate) const NUMA_NODE_NUMBER: usize = 0;
    pub(crate) const NUMA_GROUP_COUNT: usize = 22;
    pub(crate) const NUMA_GROUP_MASK: usize = 24;

    // GROUP_AFFINITY, relative to a mask entry.
    pub(crate) const AFFINITY_MASK: usize = 0;
    #[cfg(target_pointer_width = "64")]
    pub(crate) const AFFINITY_GROUP: usize = 8;
    #[cfg(target_pointer_width = "32")]
    pub(crate) const AFFINITY_GROUP: usize = 4;
    #[cfg(target_pointer_width = "64")]
    pub(crate) const AFFINITY_ENTRY: usize = 16;
    #[cfg(target_pointer_width = "32")]
    pub(crate) const AFFINITY_ENTRY: usize = 12;

    // LOGICAL_PROCESSOR_RELATIONSHIP values.
    pub(crate) const RELATION_PROCESSOR_CORE: i32 = 0;
    pub(crate) const RELATION_NUMA_NODE: i32 = 1;
    pub(crate) const RELATION_CACHE: i32 = 2;
    pub(crate) const RELATION_PROCESSOR_PACKAGE: i32 = 3;
    pub(crate) const RELATION_GROUP: i32 = 4;
    pub(crate) const RELATION_PROCESSOR_DIE: i32 = 5;
    pub(crate) const RELATION_NUMA_NODE_EX: i32 = 6;
    pub(crate) const RELATION_PROCESSOR_MODULE: i32 = 7;
    pub(crate) const RELATION_ALL: i32 = 0xFFFF;

    /// `PROCESSOR_RELATIONSHIP.Flags`: the core has more than one logical processor.
    pub(crate) const LTP_PC_SMT: u8 = 0x1;
}

/// `GUID` (guiddef.h).
#[derive(Clone, Copy)]
#[repr(C)]
pub(crate) struct Guid {
    pub(crate) data1: u32,
    pub(crate) data2: u16,
    pub(crate) data3: u16,
    pub(crate) data4: [u8; 8],
}
layout!(Guid, size = 16, align = 4);

/// `SOCKADDR` (ws2def.h).
#[repr(C)]
pub(crate) struct Sockaddr {
    pub(crate) family: u16,
    pub(crate) data: [u8; 14],
}
layout!(Sockaddr, size = 16, align = 2);

/// `SOCKADDR_IN` (ws2def.h). The address is in network byte order.
#[repr(C)]
pub(crate) struct SockaddrIn {
    pub(crate) family: u16,
    pub(crate) port: u16,
    pub(crate) addr: u32,
    pub(crate) zero: [u8; 8],
}
layout!(SockaddrIn, size = 16, align = 4);

/// `SOCKADDR_IN6` (ws2ipdef.h).
#[repr(C)]
pub(crate) struct SockaddrIn6 {
    pub(crate) family: u16,
    pub(crate) port: u16,
    pub(crate) flowinfo: u32,
    pub(crate) addr: [u8; 16],
    pub(crate) scope_id: u32,
}
layout!(SockaddrIn6, size = 28, align = 4);

/// `SOCKET_ADDRESS` (ws2def.h).
#[repr(C)]
pub(crate) struct SocketAddress {
    pub(crate) sockaddr: *mut Sockaddr,
    pub(crate) sockaddr_length: i32,
}
#[cfg(target_pointer_width = "64")]
layout!(SocketAddress, size = 16, align = 8);
#[cfg(target_pointer_width = "32")]
layout!(SocketAddress, size = 8, align = 4);

/// `IP_ADAPTER_UNICAST_ADDRESS_LH` (iptypes.h). The leading union is
/// `{ULONGLONG Alignment; struct {ULONG Length; DWORD Flags;}}`.
#[repr(C)]
pub(crate) struct IpAdapterUnicastAddress {
    pub(crate) length: u32,
    pub(crate) flags: u32,
    pub(crate) next: *mut IpAdapterUnicastAddress,
    pub(crate) address: SocketAddress,
    pub(crate) prefix_origin: i32,
    pub(crate) suffix_origin: i32,
    pub(crate) dad_state: i32,
    pub(crate) valid_lifetime: u32,
    pub(crate) preferred_lifetime: u32,
    pub(crate) lease_lifetime: u32,
    pub(crate) on_link_prefix_length: u8,
    /// Gives the leading union its `ULONGLONG` alignment.
    pub(crate) _align: [u64; 0],
}
#[cfg(target_pointer_width = "64")]
layout!(IpAdapterUnicastAddress, size = 64, align = 8);
#[cfg(target_pointer_width = "32")]
layout!(IpAdapterUnicastAddress, size = 48, align = 8);

/// `IP_ADAPTER_ADDRESSES_LH` (iptypes.h). The leading union is
/// `{ULONGLONG Alignment; struct {ULONG Length; IF_INDEX IfIndex;}}`; the
/// `Flags` union is one `ULONG` of bit flags.
#[repr(C)]
pub(crate) struct IpAdapterAddresses {
    pub(crate) length: u32,
    pub(crate) if_index: u32,
    pub(crate) next: *mut IpAdapterAddresses,
    pub(crate) adapter_name: *mut u8,
    pub(crate) first_unicast_address: *mut IpAdapterUnicastAddress,
    pub(crate) first_anycast_address: *mut c_void,
    pub(crate) first_multicast_address: *mut c_void,
    pub(crate) first_dns_server_address: *mut c_void,
    pub(crate) dns_suffix: *mut u16,
    pub(crate) description: *mut u16,
    pub(crate) friendly_name: *mut u16,
    pub(crate) physical_address: [u8; 8],
    pub(crate) physical_address_length: u32,
    pub(crate) flags: u32,
    pub(crate) mtu: u32,
    pub(crate) if_type: u32,
    pub(crate) oper_status: i32,
    pub(crate) ipv6_if_index: u32,
    pub(crate) zone_indices: [u32; 16],
    pub(crate) first_prefix: *mut c_void,
    pub(crate) transmit_link_speed: u64,
    pub(crate) receive_link_speed: u64,
    pub(crate) first_wins_server_address: *mut c_void,
    pub(crate) first_gateway_address: *mut c_void,
    pub(crate) ipv4_metric: u32,
    pub(crate) ipv6_metric: u32,
    pub(crate) luid: u64,
    pub(crate) dhcpv4_server: SocketAddress,
    pub(crate) compartment_id: u32,
    pub(crate) network_guid: Guid,
    pub(crate) connection_type: i32,
    pub(crate) tunnel_type: i32,
    pub(crate) dhcpv6_server: SocketAddress,
    pub(crate) dhcpv6_client_duid: [u8; 130],
    pub(crate) dhcpv6_client_duid_length: u32,
    pub(crate) dhcpv6_iaid: u32,
    pub(crate) first_dns_suffix: *mut c_void,
}
#[cfg(target_pointer_width = "64")]
layout!(IpAdapterAddresses, size = 448, align = 8);
#[cfg(target_pointer_width = "32")]
layout!(IpAdapterAddresses, size = 376, align = 8);

// GetAdaptersAddresses families and flags (iptypes.h, ws2def.h).
pub(crate) const AF_UNSPEC: u32 = 0;
pub(crate) const AF_INET: u16 = 2;
pub(crate) const AF_INET6: u16 = 23;
pub(crate) const GAA_FLAG_SKIP_ANYCAST: u32 = 0x0002;
pub(crate) const GAA_FLAG_SKIP_MULTICAST: u32 = 0x0004;
pub(crate) const GAA_FLAG_SKIP_DNS_SERVER: u32 = 0x0008;
pub(crate) const GAA_FLAG_INCLUDE_PREFIX: u32 = 0x0010;
// IANA interface types (ipifcons.h).
pub(crate) const IF_TYPE_OTHER: u32 = 1;
pub(crate) const IF_TYPE_ETHERNET_CSMACD: u32 = 6;
pub(crate) const IF_TYPE_PPP: u32 = 23;
pub(crate) const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;
pub(crate) const IF_TYPE_IEEE80211: u32 = 71;
pub(crate) const IF_TYPE_TUNNEL: u32 = 131;
pub(crate) const IF_TYPE_IEEE1394: u32 = 144;
pub(crate) const IF_TYPE_WWANPP: u32 = 243;
pub(crate) const IF_TYPE_WWANPP2: u32 = 244;
// IF_OPER_STATUS (ifdef.h).
pub(crate) const IF_OPER_STATUS_UP: i32 = 1;
// IP_ADAPTER_ADDRESSES Flags (iptypes.h).
pub(crate) const IP_ADAPTER_RECEIVE_ONLY: u32 = 0x0008;
pub(crate) const IP_ADAPTER_NO_MULTICAST: u32 = 0x0010;

/// `MIB_IF_ROW2` (netioapi.h). Counters are 64-bit.
#[repr(C)]
pub(crate) struct MibIfRow2 {
    pub(crate) interface_luid: u64,
    pub(crate) interface_index: u32,
    pub(crate) interface_guid: Guid,
    pub(crate) alias: [u16; 257],
    pub(crate) description: [u16; 257],
    pub(crate) physical_address_length: u32,
    pub(crate) physical_address: [u8; 32],
    pub(crate) permanent_physical_address: [u8; 32],
    pub(crate) mtu: u32,
    pub(crate) if_type: u32,
    pub(crate) tunnel_type: i32,
    pub(crate) media_type: i32,
    pub(crate) physical_medium_type: i32,
    pub(crate) access_type: i32,
    pub(crate) direction_type: i32,
    /// `InterfaceAndOperStatusFlags`: bit 0 hardware interface, bit 1 filter
    /// interface, bit 2 connector present, ...
    pub(crate) interface_and_oper_status_flags: u8,
    pub(crate) oper_status: i32,
    pub(crate) admin_status: i32,
    pub(crate) media_connect_state: i32,
    pub(crate) network_guid: Guid,
    pub(crate) connection_type: i32,
    pub(crate) transmit_link_speed: u64,
    pub(crate) receive_link_speed: u64,
    pub(crate) in_octets: u64,
    pub(crate) in_ucast_pkts: u64,
    pub(crate) in_n_ucast_pkts: u64,
    pub(crate) in_discards: u64,
    pub(crate) in_errors: u64,
    pub(crate) in_unknown_protos: u64,
    pub(crate) in_ucast_octets: u64,
    pub(crate) in_multicast_octets: u64,
    pub(crate) in_broadcast_octets: u64,
    pub(crate) out_octets: u64,
    pub(crate) out_ucast_pkts: u64,
    pub(crate) out_n_ucast_pkts: u64,
    pub(crate) out_discards: u64,
    pub(crate) out_errors: u64,
    pub(crate) out_ucast_octets: u64,
    pub(crate) out_multicast_octets: u64,
    pub(crate) out_broadcast_octets: u64,
    pub(crate) out_q_len: u64,
}
layout!(MibIfRow2, size = 1352, align = 8);

/// `MIB_IF_TABLE2` (netioapi.h): `num_entries` rows follow the header.
#[repr(C)]
pub(crate) struct MibIfTable2 {
    pub(crate) num_entries: u32,
    pub(crate) table: [MibIfRow2; 1],
}
layout!(MibIfTable2, size = 1360, align = 8);

// Registry (winreg.h, winnt.h).
/// `(HKEY)(ULONG_PTR)((LONG)0x80000002)`: sign-extended on 64-bit targets.
pub(crate) const HKEY_LOCAL_MACHINE: Hkey =
    core::ptr::without_provenance_mut(0x8000_0002_u32 as i32 as isize as usize);
pub(crate) const KEY_QUERY_VALUE: u32 = 0x0001;
pub(crate) const KEY_WOW64_64KEY: u32 = 0x0100;
pub(crate) const REG_SZ: u32 = 1;
pub(crate) const REG_DWORD: u32 = 4;

// Drives and volumes (fileapi.h, winbase.h).
pub(crate) const DRIVE_UNKNOWN: u32 = 0;
pub(crate) const DRIVE_NO_ROOT_DIR: u32 = 1;
pub(crate) const DRIVE_REMOVABLE: u32 = 2;
pub(crate) const DRIVE_FIXED: u32 = 3;
pub(crate) const DRIVE_REMOTE: u32 = 4;
pub(crate) const DRIVE_CDROM: u32 = 5;
pub(crate) const DRIVE_RAMDISK: u32 = 6;
pub(crate) const FILE_READ_ONLY_VOLUME: u32 = 0x0008_0000;
pub(crate) const SEM_FAILCRITICALERRORS: u32 = 0x0001;

// File attributes and flags (winnt.h, fileapi.h).
pub(crate) const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
pub(crate) const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

// CreateFileW (fileapi.h, winnt.h).
pub(crate) const FILE_SHARE_READ: u32 = 0x1;
pub(crate) const FILE_SHARE_WRITE: u32 = 0x2;
pub(crate) const OPEN_EXISTING: u32 = 3;

/// `ComputerNameDnsHostname` (`COMPUTER_NAME_FORMAT`, sysinfoapi.h).
pub(crate) const COMPUTER_NAME_DNS_HOSTNAME: i32 = 1;
/// `ALL_PROCESSOR_GROUPS` (winnt.h).
pub(crate) const ALL_PROCESSOR_GROUPS: u16 = 0xFFFF;
/// The `'RSMB'` firmware table provider (raw SMBIOS).
pub(crate) const RSMB: u32 = u32::from_be_bytes(*b"RSMB");
/// `LowMemoryResourceNotification` (`MEMORY_RESOURCE_NOTIFICATION_TYPE`, memoryapi.h).
pub(crate) const LOW_MEMORY_RESOURCE_NOTIFICATION: i32 = 0;
// IMAGE_FILE_MACHINE_* (winnt.h), from IsWow64Process2.
pub(crate) const IMAGE_FILE_MACHINE_UNKNOWN: u16 = 0;
pub(crate) const IMAGE_FILE_MACHINE_I386: u16 = 0x014C;
pub(crate) const IMAGE_FILE_MACHINE_ARMNT: u16 = 0x01C4;
pub(crate) const IMAGE_FILE_MACHINE_AMD64: u16 = 0x8664;
pub(crate) const IMAGE_FILE_MACHINE_ARM64: u16 = 0xAA64;
// GetSystemPowerStatus (winbase.h).
pub(crate) const AC_LINE_ONLINE: u8 = 1;
pub(crate) const AC_LINE_UNKNOWN: u8 = 255;
pub(crate) const BATTERY_FLAG_CHARGING: u8 = 8;
pub(crate) const BATTERY_FLAG_NO_BATTERY: u8 = 128;
pub(crate) const BATTERY_FLAG_UNKNOWN: u8 = 255;
pub(crate) const BATTERY_PERCENTAGE_UNKNOWN: u8 = 255;
pub(crate) const BATTERY_LIFE_UNKNOWN: u32 = 0xFFFF_FFFF;

// ---------------------------------------------------------------------------
// Functions
// ---------------------------------------------------------------------------

dll!("kernel32.dll" {
    // sysinfoapi.h
    pub(crate) fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> Bool;
    pub(crate) safe fn GetTickCount64() -> u64;
    pub(crate) fn GetSystemInfo(info: *mut SystemInfo);
    pub(crate) fn GetLogicalProcessorInformationEx(
        relationship: i32,
        buffer: *mut u8,
        returned_length: *mut u32,
    ) -> Bool;
    pub(crate) fn GetSystemFirmwareTable(provider: u32, id: u32, buffer: *mut u8, size: u32) -> u32;
    pub(crate) fn GetComputerNameExW(format: i32, buffer: *mut u16, size: *mut u32) -> Bool;
    // winbase.h, processthreadsapi.h
    pub(crate) safe fn GetActiveProcessorCount(group: u16) -> u32;
    pub(crate) fn GetSystemTimes(idle: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> Bool;
    pub(crate) safe fn GetCurrentProcess() -> Handle;
    pub(crate) fn GetProcessTimes(
        process: Handle,
        creation: *mut FileTime,
        exit: *mut FileTime,
        kernel: *mut FileTime,
        user: *mut FileTime,
    ) -> Bool;
    pub(crate) fn GetProcessAffinityMask(process: Handle, process_mask: *mut usize, system_mask: *mut usize) -> Bool;
    pub(crate) fn GetProcessGroupAffinity(process: Handle, group_count: *mut u16, groups: *mut u16) -> Bool;
    pub(crate) fn GetProcessHandleCount(process: Handle, count: *mut u32) -> Bool;
    pub(crate) fn IsWow64Process2(process: Handle, process_machine: *mut u16, native_machine: *mut u16) -> Bool;
    pub(crate) fn GetSystemPowerStatus(status: *mut SystemPowerStatus) -> Bool;
    pub(crate) fn SetThreadErrorMode(new_mode: u32, old_mode: *mut u32) -> Bool;
    // psapi.h (the K32 forms live in kernel32 from Windows 7 on)
    pub(crate) fn K32GetProcessMemoryInfo(process: Handle, counters: *mut ProcessMemoryCountersEx, cb: u32) -> Bool;
    pub(crate) fn K32GetPerformanceInfo(info: *mut PerformanceInformation, cb: u32) -> Bool;
    // jobapi2.h
    pub(crate) fn QueryInformationJobObject(
        job: Handle,
        class: i32,
        info: *mut c_void,
        length: u32,
        return_length: *mut u32,
    ) -> Bool;
    // memoryapi.h
    pub(crate) fn CreateMemoryResourceNotification(kind: i32) -> Handle;
    pub(crate) fn QueryMemoryResourceNotification(notification: Handle, state: *mut Bool) -> Bool;
    // fileapi.h
    pub(crate) fn GetDiskFreeSpaceExW(
        directory: *const u16,
        free_to_caller: *mut u64,
        total: *mut u64,
        total_free: *mut u64,
    ) -> Bool;
    pub(crate) fn GetLogicalDriveStringsW(length: u32, buffer: *mut u16) -> u32;
    pub(crate) fn GetDriveTypeW(root: *const u16) -> u32;
    pub(crate) fn GetVolumeInformationW(
        root: *const u16,
        volume_name: *mut u16,
        volume_name_size: u32,
        serial_number: *mut u32,
        max_component_length: *mut u32,
        file_system_flags: *mut u32,
        file_system_name: *mut u16,
        file_system_name_size: u32,
    ) -> Bool;
    pub(crate) fn GetVolumePathNameW(file_name: *const u16, volume_path: *mut u16, length: u32) -> Bool;
    pub(crate) fn FindFirstVolumeW(volume_name: *mut u16, length: u32) -> Handle;
    pub(crate) fn FindNextVolumeW(find: Handle, volume_name: *mut u16, length: u32) -> Bool;
    pub(crate) fn FindVolumeClose(find: Handle) -> Bool;
    pub(crate) fn GetVolumePathNamesForVolumeNameW(
        volume_name: *const u16,
        path_names: *mut u16,
        length: u32,
        return_length: *mut u32,
    ) -> Bool;
    pub(crate) fn CreateFileW(
        file_name: *const u16,
        desired_access: u32,
        share_mode: u32,
        security_attributes: *mut c_void,
        creation_disposition: u32,
        flags_and_attributes: u32,
        template_file: Handle,
    ) -> Handle;
    // ioapiset.h
    pub(crate) fn DeviceIoControl(
        device: Handle,
        control_code: u32,
        in_buffer: *const c_void,
        in_size: u32,
        out_buffer: *mut c_void,
        out_size: u32,
        bytes_returned: *mut u32,
        overlapped: *mut c_void,
    ) -> Bool;
    // handleapi.h
    pub(crate) fn CloseHandle(handle: Handle) -> Bool;
    // timezoneapi.h
    pub(crate) fn GetDynamicTimeZoneInformation(info: *mut DynamicTimeZoneInformation) -> u32;
    // winnls.h
    pub(crate) fn GetUserDefaultLocaleName(name: *mut u16, length: i32) -> i32;
    // tlhelp32.h
    pub(crate) fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> Handle;
    pub(crate) fn Process32FirstW(snapshot: Handle, entry: *mut ProcessEntry32W) -> Bool;
    pub(crate) fn Process32NextW(snapshot: Handle, entry: *mut ProcessEntry32W) -> Bool;
    pub(crate) fn Thread32First(snapshot: Handle, entry: *mut ThreadEntry32) -> Bool;
    pub(crate) fn Thread32Next(snapshot: Handle, entry: *mut ThreadEntry32) -> Bool;
});

dll!("advapi32.dll" {
    // winreg.h
    pub(crate) fn RegOpenKeyExW(key: Hkey, sub_key: *const u16, options: u32, sam: u32, result: *mut Hkey) -> i32;
    pub(crate) fn RegQueryValueExW(
        key: Hkey,
        value_name: *const u16,
        reserved: *mut u32,
        kind: *mut u32,
        data: *mut u8,
        data_length: *mut u32,
    ) -> i32;
    pub(crate) fn RegCloseKey(key: Hkey) -> i32;
    // processthreadsapi.h, securitybaseapi.h
    pub(crate) fn OpenProcessToken(process: Handle, desired_access: u32, token: *mut Handle) -> Bool;
    pub(crate) fn GetTokenInformation(
        token: Handle,
        class: i32,
        info: *mut c_void,
        length: u32,
        return_length: *mut u32,
    ) -> Bool;
    // winbase.h
    pub(crate) fn GetUserNameW(buffer: *mut u16, size: *mut u32) -> Bool;
});

dll!("iphlpapi.dll" {
    // iphlpapi.h, netioapi.h
    pub(crate) fn GetAdaptersAddresses(
        family: u32,
        flags: u32,
        reserved: *mut c_void,
        addresses: *mut IpAdapterAddresses,
        size: *mut u32,
    ) -> u32;
    pub(crate) fn GetIfTable2(table: *mut *mut MibIfTable2) -> u32;
    pub(crate) fn FreeMibTable(memory: *const c_void);
    pub(crate) fn GetBestInterfaceEx(destination: *const Sockaddr, index: *mut u32) -> u32;
});

dll!("powrprof.dll" {
    // powerbase.h
    pub(crate) fn CallNtPowerInformation(
        level: i32,
        input: *const c_void,
        input_length: u32,
        output: *mut c_void,
        output_length: u32,
    ) -> NtStatus;
    pub(crate) safe fn PowerDeterminePlatformRoleEx(version: u32) -> i32;
});

dll!("ntdll.dll" {
    // wdm.h: the real version, whatever the executable's manifest declares.
    pub(crate) fn RtlGetVersion(info: *mut OsVersionInfoExW) -> NtStatus;
    // winternl.h: per-processor CPU times.
    pub(crate) fn NtQuerySystemInformation(
        class: i32,
        info: *mut c_void,
        length: u32,
        return_length: *mut u32,
    ) -> NtStatus;
});

#[cfg(test)]
mod tests {
    //! Every struct above against `windows-sys` (a dev-dependency used as an
    //! oracle only): size, alignment, and the offset of every field.

    use windows_sys::{
        Win32::{
            Foundation as f, NetworkManagement::IpHelper as ip, Networking::WinSock as ws,
            Security as sec, System::Diagnostics::ToolHelp as th, System::Ioctl as io,
            System::JobObjects as job, System::Power as pw, System::ProcessStatus as ps,
            System::SystemInformation as si, System::Threading as thr, System::Time as tm,
            System::WindowsProgramming as wp,
        },
        core::GUID,
    };

    use super::*;

    same_layout!(FileTime, f::FILETIME, low => dwLowDateTime, high => dwHighDateTime);
    same_layout!(MemoryStatusEx, si::MEMORYSTATUSEX,
        length => dwLength, memory_load => dwMemoryLoad, total_phys => ullTotalPhys,
        avail_phys => ullAvailPhys, total_page_file => ullTotalPageFile,
        avail_page_file => ullAvailPageFile, total_virtual => ullTotalVirtual,
        avail_virtual => ullAvailVirtual, avail_extended_virtual => ullAvailExtendedVirtual);
    same_layout!(SystemInfo, si::SYSTEM_INFO,
        processor_architecture => Anonymous, page_size => dwPageSize,
        minimum_application_address => lpMinimumApplicationAddress,
        maximum_application_address => lpMaximumApplicationAddress,
        active_processor_mask => dwActiveProcessorMask,
        number_of_processors => dwNumberOfProcessors, processor_type => dwProcessorType,
        allocation_granularity => dwAllocationGranularity,
        processor_level => wProcessorLevel, processor_revision => wProcessorRevision);
    same_layout!(OsVersionInfoExW, si::OSVERSIONINFOEXW,
        os_version_info_size => dwOSVersionInfoSize, major_version => dwMajorVersion,
        minor_version => dwMinorVersion, build_number => dwBuildNumber,
        platform_id => dwPlatformId, csd_version => szCSDVersion,
        service_pack_major => wServicePackMajor, service_pack_minor => wServicePackMinor,
        suite_mask => wSuiteMask, product_type => wProductType, reserved => wReserved);
    same_layout!(ProcessMemoryCountersEx, ps::PROCESS_MEMORY_COUNTERS_EX,
        cb => cb, page_fault_count => PageFaultCount,
        peak_working_set_size => PeakWorkingSetSize, working_set_size => WorkingSetSize,
        pagefile_usage => PagefileUsage, peak_pagefile_usage => PeakPagefileUsage,
        private_usage => PrivateUsage);
    same_layout!(PerformanceInformation, ps::PERFORMANCE_INFORMATION,
        cb => cb, commit_total => CommitTotal, commit_limit => CommitLimit,
        commit_peak => CommitPeak, physical_total => PhysicalTotal,
        physical_available => PhysicalAvailable, system_cache => SystemCache,
        kernel_total => KernelTotal, page_size => PageSize, handle_count => HandleCount,
        process_count => ProcessCount, thread_count => ThreadCount);
    same_layout!(IoCounters, thr::IO_COUNTERS,
        read_operation_count => ReadOperationCount, other_transfer_count => OtherTransferCount);
    same_layout!(JobObjectBasicLimitInformation, job::JOBOBJECT_BASIC_LIMIT_INFORMATION,
        per_process_user_time_limit => PerProcessUserTimeLimit, limit_flags => LimitFlags,
        minimum_working_set_size => MinimumWorkingSetSize,
        maximum_working_set_size => MaximumWorkingSetSize,
        active_process_limit => ActiveProcessLimit, affinity => Affinity,
        priority_class => PriorityClass, scheduling_class => SchedulingClass);
    same_layout!(JobObjectExtendedLimitInformation, job::JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        basic_limit_information => BasicLimitInformation, io_info => IoInfo,
        process_memory_limit => ProcessMemoryLimit, job_memory_limit => JobMemoryLimit,
        peak_process_memory_used => PeakProcessMemoryUsed,
        peak_job_memory_used => PeakJobMemoryUsed);
    same_layout!(JobObjectCpuRateControlInformation, job::JOBOBJECT_CPU_RATE_CONTROL_INFORMATION,
        control_flags => ControlFlags, value => Anonymous);
    same_layout!(SystemPowerStatus, pw::SYSTEM_POWER_STATUS,
        ac_line_status => ACLineStatus, battery_flag => BatteryFlag,
        battery_life_percent => BatteryLifePercent, system_status_flag => SystemStatusFlag,
        battery_life_time => BatteryLifeTime, battery_full_life_time => BatteryFullLifeTime);
    same_layout!(ProcessorPowerInformation, pw::PROCESSOR_POWER_INFORMATION,
        number => Number, max_mhz => MaxMhz, current_mhz => CurrentMhz,
        mhz_limit => MhzLimit, current_idle_state => CurrentIdleState);
    same_layout!(SystemTime, f::SYSTEMTIME, year => wYear, milliseconds => wMilliseconds);
    same_layout!(DynamicTimeZoneInformation, tm::DYNAMIC_TIME_ZONE_INFORMATION,
        bias => Bias, standard_name => StandardName, standard_date => StandardDate,
        standard_bias => StandardBias, daylight_name => DaylightName,
        daylight_date => DaylightDate, daylight_bias => DaylightBias,
        time_zone_key_name => TimeZoneKeyName,
        dynamic_daylight_time_disabled => DynamicDaylightTimeDisabled);
    same_layout!(ProcessEntry32W, th::PROCESSENTRY32W,
        size => dwSize, th32_process_id => th32ProcessID,
        th32_default_heap_id => th32DefaultHeapID, cnt_threads => cntThreads,
        th32_parent_process_id => th32ParentProcessID, exe_file => szExeFile);
    same_layout!(ThreadEntry32, th::THREADENTRY32,
        size => dwSize, th32_thread_id => th32ThreadID,
        th32_owner_process_id => th32OwnerProcessID, flags => dwFlags);
    same_layout!(SystemProcessorPerformanceInformation, wp::SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION,
        idle_time => IdleTime, kernel_time => KernelTime, user_time => UserTime,
        dpc_and_interrupt_time => Reserved1, interrupt_count => Reserved2);
    same_layout!(TokenElevation, sec::TOKEN_ELEVATION, token_is_elevated => TokenIsElevated);
    same_layout!(StoragePropertyQuery, io::STORAGE_PROPERTY_QUERY,
        property_id => PropertyId, query_type => QueryType,
        additional_parameters => AdditionalParameters);
    same_layout!(DeviceSeekPenaltyDescriptor, io::DEVICE_SEEK_PENALTY_DESCRIPTOR,
        version => Version, size => Size, incurs_seek_penalty => IncursSeekPenalty);
    same_layout!(GroupAffinity, si::GROUP_AFFINITY, mask => Mask, group => Group);
    same_layout!(Guid, GUID, data1 => data1, data2 => data2, data3 => data3, data4 => data4);
    same_layout!(Sockaddr, ws::SOCKADDR, family => sa_family, data => sa_data);
    same_layout!(SockaddrIn, ws::SOCKADDR_IN,
        family => sin_family, port => sin_port, addr => sin_addr, zero => sin_zero);
    same_layout!(SockaddrIn6, ws::SOCKADDR_IN6,
        family => sin6_family, port => sin6_port, flowinfo => sin6_flowinfo,
        addr => sin6_addr, scope_id => Anonymous);
    same_layout!(SocketAddress, ws::SOCKET_ADDRESS,
        sockaddr => lpSockaddr, sockaddr_length => iSockaddrLength);
    same_layout!(IpAdapterUnicastAddress, ip::IP_ADAPTER_UNICAST_ADDRESS_LH,
        length => Anonymous, next => Next, address => Address,
        prefix_origin => PrefixOrigin, suffix_origin => SuffixOrigin, dad_state => DadState,
        valid_lifetime => ValidLifetime, preferred_lifetime => PreferredLifetime,
        lease_lifetime => LeaseLifetime, on_link_prefix_length => OnLinkPrefixLength);
    same_layout!(IpAdapterAddresses, ip::IP_ADAPTER_ADDRESSES_LH,
        length => Anonymous1, next => Next, adapter_name => AdapterName,
        first_unicast_address => FirstUnicastAddress, dns_suffix => DnsSuffix,
        description => Description, friendly_name => FriendlyName,
        physical_address => PhysicalAddress, physical_address_length => PhysicalAddressLength,
        flags => Anonymous2, mtu => Mtu, if_type => IfType, oper_status => OperStatus,
        ipv6_if_index => Ipv6IfIndex, zone_indices => ZoneIndices, first_prefix => FirstPrefix,
        transmit_link_speed => TransmitLinkSpeed, receive_link_speed => ReceiveLinkSpeed,
        first_gateway_address => FirstGatewayAddress, ipv4_metric => Ipv4Metric,
        ipv6_metric => Ipv6Metric, luid => Luid, dhcpv4_server => Dhcpv4Server,
        compartment_id => CompartmentId, network_guid => NetworkGuid,
        connection_type => ConnectionType, tunnel_type => TunnelType,
        dhcpv6_server => Dhcpv6Server, dhcpv6_client_duid => Dhcpv6ClientDuid,
        dhcpv6_client_duid_length => Dhcpv6ClientDuidLength, dhcpv6_iaid => Dhcpv6Iaid,
        first_dns_suffix => FirstDnsSuffix);
    same_layout!(MibIfRow2, ip::MIB_IF_ROW2,
        interface_luid => InterfaceLuid, interface_index => InterfaceIndex,
        interface_guid => InterfaceGuid, alias => Alias, description => Description,
        physical_address_length => PhysicalAddressLength, physical_address => PhysicalAddress,
        permanent_physical_address => PermanentPhysicalAddress, mtu => Mtu, if_type => Type,
        tunnel_type => TunnelType, media_type => MediaType,
        physical_medium_type => PhysicalMediumType, access_type => AccessType,
        direction_type => DirectionType,
        interface_and_oper_status_flags => InterfaceAndOperStatusFlags,
        oper_status => OperStatus, admin_status => AdminStatus,
        media_connect_state => MediaConnectState, network_guid => NetworkGuid,
        connection_type => ConnectionType, transmit_link_speed => TransmitLinkSpeed,
        receive_link_speed => ReceiveLinkSpeed, in_octets => InOctets,
        in_ucast_pkts => InUcastPkts, in_n_ucast_pkts => InNUcastPkts,
        in_discards => InDiscards, in_errors => InErrors,
        in_unknown_protos => InUnknownProtos, out_octets => OutOctets,
        out_ucast_pkts => OutUcastPkts, out_n_ucast_pkts => OutNUcastPkts,
        out_discards => OutDiscards, out_errors => OutErrors, out_q_len => OutQLen);
    same_layout!(MibIfTable2, ip::MIB_IF_TABLE2, num_entries => NumEntries, table => Table);

    // The byte offsets used to parse GetLogicalProcessorInformationEx records.
    const _: () = {
        use core::mem::{offset_of, size_of};
        type Ex = si::SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX;
        assert!(offset_of!(Ex, Relationship) == lpi::RELATIONSHIP);
        assert!(offset_of!(Ex, Size) == lpi::SIZE);
        assert!(offset_of!(Ex, Anonymous) == lpi::BODY);
        type P = si::PROCESSOR_RELATIONSHIP;
        assert!(offset_of!(P, Flags) == lpi::PROCESSOR_FLAGS);
        assert!(offset_of!(P, EfficiencyClass) == lpi::PROCESSOR_EFFICIENCY_CLASS);
        assert!(offset_of!(P, GroupCount) == lpi::PROCESSOR_GROUP_COUNT);
        assert!(offset_of!(P, GroupMask) == lpi::PROCESSOR_GROUP_MASK);
        type C = si::CACHE_RELATIONSHIP;
        assert!(offset_of!(C, Level) == lpi::CACHE_LEVEL);
        assert!(offset_of!(C, LineSize) == lpi::CACHE_LINE_SIZE);
        assert!(offset_of!(C, CacheSize) == lpi::CACHE_SIZE);
        assert!(offset_of!(C, Type) == lpi::CACHE_TYPE);
        assert!(offset_of!(C, GroupCount) == lpi::CACHE_GROUP_COUNT);
        assert!(offset_of!(C, Anonymous) == lpi::CACHE_GROUP_MASK);
        type N = si::NUMA_NODE_RELATIONSHIP;
        assert!(offset_of!(N, NodeNumber) == lpi::NUMA_NODE_NUMBER);
        assert!(offset_of!(N, GroupCount) == lpi::NUMA_GROUP_COUNT);
        assert!(offset_of!(N, Anonymous) == lpi::NUMA_GROUP_MASK);
        type G = si::GROUP_AFFINITY;
        assert!(offset_of!(G, Mask) == lpi::AFFINITY_MASK);
        assert!(offset_of!(G, Group) == lpi::AFFINITY_GROUP);
        assert!(size_of::<G>() == lpi::AFFINITY_ENTRY);
    };

    // Constants that windows-sys also defines.
    const _: () = {
        assert!(JOB_OBJECT_EXTENDED_LIMIT_INFORMATION == job::JobObjectExtendedLimitInformation);
        assert!(JOB_OBJECT_CPU_RATE_CONTROL_INFORMATION == job::JobObjectCpuRateControlInformation);
        assert!(JOB_OBJECT_LIMIT_PROCESS_MEMORY == job::JOB_OBJECT_LIMIT_PROCESS_MEMORY);
        assert!(JOB_OBJECT_LIMIT_JOB_MEMORY == job::JOB_OBJECT_LIMIT_JOB_MEMORY);
        assert!(JOB_OBJECT_CPU_RATE_CONTROL_ENABLE == job::JOB_OBJECT_CPU_RATE_CONTROL_ENABLE);
        assert!(JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP == job::JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP);
        assert!(
            JOB_OBJECT_CPU_RATE_CONTROL_MIN_MAX_RATE
                == job::JOB_OBJECT_CPU_RATE_CONTROL_MIN_MAX_RATE
        );
        assert!(PROCESSOR_INFORMATION == pw::ProcessorInformation);
        assert!(TH32CS_SNAPPROCESS == th::TH32CS_SNAPPROCESS);
        assert!(TH32CS_SNAPTHREAD == th::TH32CS_SNAPTHREAD);
        assert!(TOKEN_ELEVATION_CLASS == sec::TokenElevation);
        assert!(TOKEN_QUERY == sec::TOKEN_QUERY);
        assert!(
            FILE_ATTRIBUTE_REPARSE_POINT
                == windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
        );
        assert!(
            FILE_FLAG_OPEN_REPARSE_POINT
                == windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT
        );
        assert!(IOCTL_STORAGE_QUERY_PROPERTY == io::IOCTL_STORAGE_QUERY_PROPERTY);
        assert!(STORAGE_DEVICE_SEEK_PENALTY_PROPERTY == io::StorageDeviceSeekPenaltyProperty);
        assert!(PROPERTY_STANDARD_QUERY == io::PropertyStandardQuery);
        assert!(GAA_FLAG_INCLUDE_PREFIX == ip::GAA_FLAG_INCLUDE_PREFIX);
        assert!(GAA_FLAG_SKIP_ANYCAST == ip::GAA_FLAG_SKIP_ANYCAST);
        assert!(GAA_FLAG_SKIP_MULTICAST == ip::GAA_FLAG_SKIP_MULTICAST);
        assert!(GAA_FLAG_SKIP_DNS_SERVER == ip::GAA_FLAG_SKIP_DNS_SERVER);
        assert!(AF_INET == ws::AF_INET);
        assert!(AF_INET6 == ws::AF_INET6);
        assert!(ERROR_BUFFER_OVERFLOW == f::ERROR_BUFFER_OVERFLOW);
        assert!(ERROR_INSUFFICIENT_BUFFER == f::ERROR_INSUFFICIENT_BUFFER);
        assert!(STATUS_INFO_LENGTH_MISMATCH == f::STATUS_INFO_LENGTH_MISMATCH);
        assert!(lpi::RELATION_PROCESSOR_CORE == si::RelationProcessorCore);
        assert!(lpi::RELATION_NUMA_NODE == si::RelationNumaNode);
        assert!(lpi::RELATION_CACHE == si::RelationCache);
        assert!(lpi::RELATION_PROCESSOR_PACKAGE == si::RelationProcessorPackage);
        assert!(lpi::RELATION_GROUP == si::RelationGroup);
        assert!(lpi::RELATION_NUMA_NODE_EX == si::RelationNumaNodeEx);
        assert!(lpi::RELATION_ALL == si::RelationAll);
        assert!(ALL_PROCESSOR_GROUPS == thr::ALL_PROCESSOR_GROUPS);
        assert!(COMPUTER_NAME_DNS_HOSTNAME == si::ComputerNameDnsHostname);
        assert!(IMAGE_FILE_MACHINE_AMD64 == si::IMAGE_FILE_MACHINE_AMD64);
        assert!(IMAGE_FILE_MACHINE_ARM64 == si::IMAGE_FILE_MACHINE_ARM64);
        assert!(IMAGE_FILE_MACHINE_I386 == si::IMAGE_FILE_MACHINE_I386);
        assert!(IMAGE_FILE_MACHINE_ARMNT == si::IMAGE_FILE_MACHINE_ARMNT);
    };

    #[test]
    fn test_ffi_windows_layouts_match_windows_sys() {
        // The layout checks above run at compile time; this test covers the
        // pointer-valued constants, which cannot be compared in a const.
        assert_eq!(
            HKEY_LOCAL_MACHINE.addr(),
            windows_sys::Win32::System::Registry::HKEY_LOCAL_MACHINE.addr()
        );
        assert_eq!(INVALID_HANDLE_VALUE.addr(), f::INVALID_HANDLE_VALUE.addr());
        assert_eq!(core::mem::size_of::<MibIfRow2>(), 1352);
    }
}
