//! Small CPU buffer pool to avoid per-frame heap churn on the capture path.

use std::ops::Deref;
use std::sync::Arc;

use parking_lot::Mutex;

/// Idle buffers retained after drop. Channel depth is 2 + ~1 in flight — 3 is enough.
/// Each 1080p BGRA slot is ~8 Mo; keeping 8 idle slots alone burned ~65 Mo RSS.
const DEFAULT_KEEP: usize = 3;
/// Soft ceiling for retained capacity (~1080p BGRA + pad). Larger frames still work;
/// we just refuse to park 4K-sized capacity after a downshift.
const CAP_CEILING: usize = 1920 * 1080 * 4 + 64 * 1024;

#[derive(Debug)]
pub struct BufferPool {
    free: Mutex<Vec<Vec<u8>>>,
    max_keep: usize,
}

impl BufferPool {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            free: Mutex::new(Vec::with_capacity(DEFAULT_KEEP)),
            max_keep: DEFAULT_KEEP,
        })
    }

    pub fn take(self: &Arc<Self>, len: usize) -> PooledBytes {
        let mut data = {
            let mut free = self.free.lock();
            // Prefer a free slot that already fits `len` to avoid grow+waste.
            let idx = free.iter().rposition(|b| b.capacity() >= len);
            match idx {
                Some(i) => free.swap_remove(i),
                None => free.pop().unwrap_or_default(),
            }
        };
        if data.capacity() < len {
            data = Vec::with_capacity(len);
        }
        data.clear();
        data.resize(len, 0);
        PooledBytes {
            data,
            pool: Some(Arc::clone(self)),
        }
    }

    fn recycle(&self, mut data: Vec<u8>) {
        data.clear();
        if data.capacity() > CAP_CEILING {
            data.shrink_to(CAP_CEILING);
        }
        let mut free = self.free.lock();
        if free.len() < self.max_keep {
            free.push(data);
        }
    }
}

impl Default for BufferPool {
    fn default() -> Self {
        Self {
            free: Mutex::new(Vec::new()),
            max_keep: DEFAULT_KEEP,
        }
    }
}

/// Owned frame bytes that return to the pool when the last `Arc` drops.
#[derive(Debug)]
pub struct PooledBytes {
    data: Vec<u8>,
    pool: Option<Arc<BufferPool>>,
}

impl PooledBytes {
    pub fn as_slice(&self) -> &[u8] {
        &self.data
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.data
    }

    pub fn into_arc(self) -> Arc<PooledBytes> {
        Arc::new(self)
    }
}

impl Deref for PooledBytes {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl AsRef<[u8]> for PooledBytes {
    fn as_ref(&self) -> &[u8] {
        &self.data
    }
}

impl Drop for PooledBytes {
    fn drop(&mut self) {
        if let Some(pool) = self.pool.take() {
            pool.recycle(std::mem::take(&mut self.data));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keep_limit_is_small() {
        assert!(DEFAULT_KEEP <= 3);
    }

    #[test]
    fn recycle_caps_idle_slots() {
        let pool = BufferPool::new();
        let mut held = Vec::new();
        for _ in 0..8 {
            held.push(pool.take(1024));
        }
        drop(held);
        let free = pool.free.lock();
        assert!(free.len() <= DEFAULT_KEEP);
    }

    #[test]
    fn recycle_shrinks_oversized_capacity() {
        let pool = BufferPool::new();
        let huge = pool.take(CAP_CEILING + 2 * 1024 * 1024);
        drop(huge);
        let free = pool.free.lock();
        assert_eq!(free.len(), 1);
        assert!(free[0].capacity() <= CAP_CEILING);
    }
}
