//! Small CPU buffer pool to avoid per-frame heap churn on the capture path.

use std::ops::Deref;
use std::sync::Arc;

use parking_lot::Mutex;

const DEFAULT_KEEP: usize = 8;

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
            free.pop()
        }
        .unwrap_or_default();
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
