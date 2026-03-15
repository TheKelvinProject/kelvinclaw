#![no_std]

#[link(wasm_import_module = "kelvin_model_host_v1")]
extern "C" {
    fn provider_profile_call(req_ptr: i32, req_len: i32) -> i64;
}

const HEAP_SIZE: usize = 1024 * 1024;
static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];
static mut NEXT_OFFSET: usize = 0;

#[no_mangle]
pub extern "C" fn alloc(len: i32) -> i32 {
    if len <= 0 {
        return 0;
    }

    let len = len as usize;
    let align = 8usize;

    unsafe {
        let start = (NEXT_OFFSET + (align - 1)) & !(align - 1);
        let Some(end) = start.checked_add(len) else {
            return 0;
        };
        if end > HEAP_SIZE {
            return 0;
        }
        NEXT_OFFSET = end;
        core::ptr::addr_of_mut!(HEAP).cast::<u8>().add(start) as usize as i32
    }
}

#[no_mangle]
pub extern "C" fn dealloc(_ptr: i32, _len: i32) {}

#[no_mangle]
pub extern "C" fn infer(req_ptr: i32, req_len: i32) -> i64 {
    // SAFETY: The trusted Kelvin host provides this import for approved
    // provider_profile-backed model plugins.
    unsafe { provider_profile_call(req_ptr, req_len) }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
