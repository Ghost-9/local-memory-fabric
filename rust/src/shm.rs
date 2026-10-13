use crate::dispatch;
use crate::paths::set_mode;
use crate::text::json_error;
use serde_json::Value;
use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::ptr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

pub const SHM_MAGIC: u32 = 0x4C4D4653; // 'LMFS'
pub const SHM_VERSION: u32 = 1;
pub const SHM_HEADER_SIZE: usize = 64;
pub const SHM_REQ_SIZE: usize = 131_072; // 128 KB
pub const SHM_RESP_SIZE: usize = 262_144; // 256 KB
pub const SHM_TOTAL_SIZE: usize = SHM_HEADER_SIZE + SHM_REQ_SIZE + SHM_RESP_SIZE;

pub const STATUS_IDLE: u32 = 0;
pub const STATUS_REQ_PENDING: u32 = 1;
pub const STATUS_RESP_READY: u32 = 2;
pub const STATUS_SHUTDOWN: u32 = 3;

#[repr(C)]
pub struct ShmControl {
    pub magic: u32,
    pub version: u32,
    pub req_seq: AtomicU64,
    pub req_len: AtomicU32,
    pub resp_seq: AtomicU64,
    pub resp_len: AtomicU32,
    pub status: AtomicU32,
    pub exit_code: AtomicU32,
    pub _reserved: [u8; 24],
}

pub struct ShmMapping {
    ptr: *mut u8,
    len: usize,
}

unsafe impl Send for ShmMapping {}
unsafe impl Sync for ShmMapping {}

impl Drop for ShmMapping {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.ptr as *mut libc::c_void, self.len);
        }
    }
}

impl ShmMapping {
    pub fn open_or_create(path: &Path, is_server: bool) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(is_server)
            .mode(0o600)
            .open(path)
            .map_err(|e| format!("cannot open shm file {}: {}", path.display(), e))?;

        if is_server {
            set_mode(path, 0o600);
            file.set_len(SHM_TOTAL_SIZE as u64)
                .map_err(|e| format!("cannot set shm file length: {}", e))?;
        }

        let fd = file.as_raw_fd();
        let ptr = unsafe {
            libc::mmap(
                ptr::null_mut(),
                SHM_TOTAL_SIZE,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };

        if ptr == libc::MAP_FAILED {
            return Err(format!("mmap failed on {}", path.display()));
        }

        let mut mapping = ShmMapping {
            ptr: ptr as *mut u8,
            len: SHM_TOTAL_SIZE,
        };

        if is_server {
            let ctrl = mapping.ctrl_mut();
            ctrl.magic = SHM_MAGIC;
            ctrl.version = SHM_VERSION;
            ctrl.req_seq.store(0, Ordering::Relaxed);
            ctrl.req_len.store(0, Ordering::Relaxed);
            ctrl.resp_seq.store(0, Ordering::Relaxed);
            ctrl.resp_len.store(0, Ordering::Relaxed);
            ctrl.status.store(STATUS_IDLE, Ordering::Release);
        }

        Ok(mapping)
    }

    #[inline]
    pub fn ctrl(&self) -> &ShmControl {
        unsafe { &*(self.ptr as *const ShmControl) }
    }

    #[inline]
    pub fn ctrl_mut(&mut self) -> &mut ShmControl {
        unsafe { &mut *(self.ptr as *mut ShmControl) }
    }

    #[inline]
    pub fn req_buf(&self) -> &[u8] {
        unsafe {
            let len = self.ctrl().req_len.load(Ordering::Acquire) as usize;
            let slice_ptr = self.ptr.add(SHM_HEADER_SIZE);
            std::slice::from_raw_parts(slice_ptr, len.min(SHM_REQ_SIZE))
        }
    }

    #[inline]
    pub fn write_req(&self, data: &[u8]) -> Result<(), String> {
        if data.len() > SHM_REQ_SIZE {
            return Err("request exceeds SHM slot limit".into());
        }
        unsafe {
            let slice_ptr = self.ptr.add(SHM_HEADER_SIZE);
            ptr::copy_nonoverlapping(data.as_ptr(), slice_ptr, data.len());
        }
        let ctrl = self.ctrl();
        ctrl.req_len.store(data.len() as u32, Ordering::Release);
        ctrl.req_seq.fetch_add(1, Ordering::Release);
        ctrl.status.store(STATUS_REQ_PENDING, Ordering::Release);
        Ok(())
    }

    #[inline]
    pub fn resp_buf(&self) -> &[u8] {
        unsafe {
            let len = self.ctrl().resp_len.load(Ordering::Acquire) as usize;
            let slice_ptr = self.ptr.add(SHM_HEADER_SIZE + SHM_REQ_SIZE);
            std::slice::from_raw_parts(slice_ptr, len.min(SHM_RESP_SIZE))
        }
    }

    #[inline]
    pub fn write_resp(&self, data: &[u8], code: i32) -> Result<(), String> {
        if data.len() > SHM_RESP_SIZE {
            return Err("response exceeds SHM slot limit".into());
        }
        unsafe {
            let slice_ptr = self.ptr.add(SHM_HEADER_SIZE + SHM_REQ_SIZE);
            ptr::copy_nonoverlapping(data.as_ptr(), slice_ptr, data.len());
        }
        let ctrl = self.ctrl();
        ctrl.resp_len.store(data.len() as u32, Ordering::Release);
        ctrl.exit_code.store(code as u32, Ordering::Release);
        ctrl.resp_seq.fetch_add(1, Ordering::Release);
        ctrl.status.store(STATUS_RESP_READY, Ordering::Release);
        Ok(())
    }
}

pub fn serve_shm(path: &Path) -> Result<(), String> {
    let mapping = ShmMapping::open_or_create(path, true)?;
    let fabric = crate::MemoryFabric::new()?;
    println!("SHM ring buffer serving at {}", path.display());

    let ctrl = mapping.ctrl();
    while ctrl.status.load(Ordering::Acquire) != STATUS_SHUTDOWN {
        let mut spins = 0;
        while ctrl.status.load(Ordering::Acquire) != STATUS_REQ_PENDING {
            if ctrl.status.load(Ordering::Relaxed) == STATUS_SHUTDOWN {
                return Ok(());
            }
            if spins < 2000 {
                std::hint::spin_loop();
                spins += 1;
            } else {
                thread::sleep(Duration::from_micros(100));
            }
        }

        let req_bytes = mapping.req_buf();
        let resp: Value = match serde_json::from_slice::<Value>(req_bytes) {
            Ok(req) => dispatch(&fabric, &req).unwrap_or_else(|e| json_error(&e)),
            Err(_) => json_error("request must be valid JSON"),
        };

        let resp_bytes = serde_json::to_vec(&resp).unwrap_or_else(|_| b"{\"ok\":false}".to_vec());
        let _ = mapping.write_resp(&resp_bytes, 0);
    }
    Ok(())
}

pub fn client_shm_call(path: &Path, req_bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mapping = ShmMapping::open_or_create(path, false)?;
    let ctrl = mapping.ctrl();
    if ctrl.magic != SHM_MAGIC || ctrl.version != SHM_VERSION {
        return Err("incompatible or uninitialized SHM ring buffer".into());
    }

    mapping.write_req(req_bytes)?;

    let mut spins = 0;
    while ctrl.status.load(Ordering::Acquire) != STATUS_RESP_READY {
        if spins < 5000 {
            std::hint::spin_loop();
            spins += 1;
        } else {
            thread::sleep(Duration::from_micros(50));
        }
    }

    let resp = mapping.resp_buf().to_vec();
    ctrl.status.store(STATUS_IDLE, Ordering::Release);
    Ok(resp)
}
