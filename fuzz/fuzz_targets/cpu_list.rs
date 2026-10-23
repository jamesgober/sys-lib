#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| sys_lib::__fuzz::cpu_list(data));
