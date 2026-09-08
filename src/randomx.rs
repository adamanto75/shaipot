// Safe-ish wrapper over the vendored Shaicoin RandomX.
//
// Shaicoin's fork changes the Argon2 salt to "ShaicoinRandomX-v2\x01" and
// rebalances six instruction frequencies. RANDOMX_FLAG_V2 must also be set or
// the same input hashes differently - with no error anywhere. Verified against
// the known-answer vector in randomx/SHAICOIN-FORK.md; see tests below.

use std::os::raw::{c_int, c_ulong, c_void};
use std::sync::Arc;

#[allow(non_camel_case_types)] pub enum randomx_cache {}
#[allow(non_camel_case_types)] pub enum randomx_dataset {}
#[allow(non_camel_case_types)] pub enum randomx_vm {}

pub const RANDOMX_FLAG_LARGE_PAGES: c_int = 1;
pub const RANDOMX_FLAG_FULL_MEM: c_int = 4;
pub const RANDOMX_FLAG_V2: c_int = 128;

extern "C" {
    fn randomx_get_flags() -> c_int;
    fn randomx_alloc_cache(flags: c_int) -> *mut randomx_cache;
    fn randomx_init_cache(cache: *mut randomx_cache, key: *const c_void, key_size: usize);
    fn randomx_release_cache(cache: *mut randomx_cache);
    fn randomx_alloc_dataset(flags: c_int) -> *mut randomx_dataset;
    fn randomx_init_dataset(ds: *mut randomx_dataset, cache: *mut randomx_cache, start: c_ulong, count: c_ulong);
    fn randomx_dataset_item_count() -> c_ulong;
    fn randomx_release_dataset(ds: *mut randomx_dataset);
    fn randomx_create_vm(flags: c_int, cache: *mut randomx_cache, ds: *mut randomx_dataset) -> *mut randomx_vm;
    fn randomx_destroy_vm(vm: *mut randomx_vm);
    fn randomx_calculate_hash(vm: *mut randomx_vm, input: *const c_void, input_size: usize, output: *mut c_void);
}

/// Cache (+ optional dataset) for one RandomX key. Immutable once built, so it
/// is shared across mining threads; each thread makes its own VM from it.
pub struct RxKey {
    cache: *mut randomx_cache,
    dataset: *mut randomx_dataset,
    flags: c_int,
    pub seed: Vec<u8>,
    pub fast: bool,
    pub large_pages: bool,
}

// Safe: after construction these are only read, and randomx_create_vm /
// randomx_calculate_hash take them as const inputs.
unsafe impl Send for RxKey {}
unsafe impl Sync for RxKey {}

impl RxKey {
    pub fn new(seed: &[u8], want_fast: bool, init_threads: usize) -> Option<Arc<RxKey>> {
        unsafe {
            let base = randomx_get_flags() | RANDOMX_FLAG_V2;

            // Try huge pages first; they are worth 20-30% but need
            // vm.nr_hugepages on the host and are often unavailable.
            let mut large_pages = false;
            let mut flags = base | RANDOMX_FLAG_LARGE_PAGES;
            let mut cache = randomx_alloc_cache(flags);
            if cache.is_null() {
                flags = base;
                cache = randomx_alloc_cache(flags);
            } else {
                large_pages = true;
            }
            if cache.is_null() { return None; }
            randomx_init_cache(cache, seed.as_ptr() as *const c_void, seed.len());

            let mut dataset: *mut randomx_dataset = std::ptr::null_mut();
            let mut fast = false;
            if want_fast {
                let dflags = flags | RANDOMX_FLAG_FULL_MEM;
                dataset = randomx_alloc_dataset(dflags);
                if dataset.is_null() && large_pages {
                    // huge pages can satisfy the cache but not the 2.3 GB dataset
                    dataset = randomx_alloc_dataset(base | RANDOMX_FLAG_FULL_MEM);
                    if !dataset.is_null() { flags = base; large_pages = false; }
                }
                if !dataset.is_null() {
                    let total = randomx_dataset_item_count();
                    let n = init_threads.max(1) as c_ulong;
                    let per = total / n;
                    let ds_addr = dataset as usize;
                    let c_addr = cache as usize;
                    let mut handles = Vec::new();
                    for i in 0..n {
                        let start = i * per;
                        let count = if i == n - 1 { total - start } else { per };
                        handles.push(std::thread::spawn(move || {
                            randomx_init_dataset(ds_addr as *mut randomx_dataset,
                                                 c_addr as *mut randomx_cache, start, count);
                        }));
                    }
                    for h in handles { let _ = h.join(); }
                    flags |= RANDOMX_FLAG_FULL_MEM;
                    fast = true;
                }
            }

            Some(Arc::new(RxKey { cache, dataset, flags, seed: seed.to_vec(), fast, large_pages }))
        }
    }

    pub fn new_vm(self: &Arc<Self>) -> Option<RxVm> {
        unsafe {
            let vm = randomx_create_vm(self.flags, self.cache, self.dataset);
            if vm.is_null() { return None; }
            Some(RxVm { vm, _key: Arc::clone(self) })
        }
    }
}

impl Drop for RxKey {
    fn drop(&mut self) {
        unsafe {
            if !self.dataset.is_null() { randomx_release_dataset(self.dataset); }
            if !self.cache.is_null() { randomx_release_cache(self.cache); }
        }
    }
}

/// One VM, owned by one thread. Holds an Arc to its RxKey so the cache and
/// dataset cannot be freed while this VM still points at them.
pub struct RxVm { vm: *mut randomx_vm, _key: Arc<RxKey> }

impl RxVm {
    #[inline]
    pub fn hash(&mut self, input: &[u8]) -> [u8; 32] {
        let mut out = [0u8; 32];
        unsafe {
            randomx_calculate_hash(self.vm, input.as_ptr() as *const c_void,
                                   input.len(), out.as_mut_ptr() as *mut c_void);
        }
        out
    }
}

impl Drop for RxVm {
    fn drop(&mut self) { unsafe { randomx_destroy_vm(self.vm); } }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shaicoin_known_answer_vector() {
        let k = RxKey::new(b"shaicoin-randomx-kat-key-v1", false, 1).expect("cache");
        let mut vm = k.new_vm().expect("vm");
        let got: String = vm.hash(b"shaicoin-randomx-kat-input-v1").iter()
            .map(|b| format!("{:02x}", b)).collect();
        assert_eq!(got, "1e3ede7f49c31a77fe72bc54441f490da2a158a1d8ea7920c2bd46c80ea6bad2",
            "not the Shaicoin RandomX config - a stock build gives \
             d6f97cc88f167c7917c6982b7e8ca5f0d82b8cefbf0242d18b34a5edfe92f265");
    }
}
