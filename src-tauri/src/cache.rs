//! Images already fetched from the camera during this connection. Kept in memory so going back to
//! a photo, or downloading one you zoomed into, doesn't ask the slow camera again.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::{Arc, Mutex};

use tokio::sync::Mutex as AsyncMutex;

use crate::wifi::{Image, Size};

/// A photo's index and which copy of it.
type Key = (usize, Size);

/// Holds up to `limit` bytes of images; the oldest are dropped first.
pub struct ImageCache {
    limit: usize,
    stored: Mutex<Stored>,
    /// One lock per image, so overlapping requests for the same image share one fetch.
    in_flight: Mutex<HashMap<Key, Arc<AsyncMutex<()>>>>,
}

#[derive(Default)]
struct Stored {
    images: HashMap<Key, Arc<Image>>,
    oldest_first: VecDeque<Key>,
    bytes: usize,
}

impl ImageCache {
    pub fn new(limit: usize) -> Self {
        ImageCache {
            limit,
            stored: Mutex::default(),
            in_flight: Mutex::default(),
        }
    }

    pub fn get(&self, key: Key) -> Option<Arc<Image>> {
        self.stored.lock().unwrap().images.get(&key).cloned()
    }

    /// The cached image, or the result of `fetch`, which is then cached.
    pub async fn get_or_fetch<F, Fut>(&self, key: Key, fetch: F) -> Result<Arc<Image>, String>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Image, String>>,
    {
        let lock = self
            .in_flight
            .lock()
            .unwrap()
            .entry(key)
            .or_default()
            .clone();
        let _turn = lock.lock().await;
        if let Some(image) = self.get(key) {
            return Ok(image);
        }
        let image = Arc::new(fetch().await?);
        self.insert(key, image.clone());
        Ok(image)
    }

    fn insert(&self, key: Key, image: Arc<Image>) {
        let size = image.bytes.len();
        if size > self.limit {
            return;
        }
        let mut guard = self.stored.lock().unwrap();
        let stored = &mut *guard;
        if let Some(old) = stored.images.insert(key, image) {
            stored.bytes -= old.bytes.len();
            stored.oldest_first.retain(|k| *k != key);
        }
        stored.oldest_first.push_back(key);
        stored.bytes += size;
        while stored.bytes > self.limit {
            let Some(oldest) = stored.oldest_first.pop_front() else {
                break;
            };
            if let Some(old) = stored.images.remove(&oldest) {
                stored.bytes -= old.bytes.len();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(bytes: usize) -> Image {
        Image {
            bytes: vec![0; bytes],
            content_type: "image/jpeg".into(),
        }
    }

    #[tokio::test]
    async fn fetches_once_then_serves_from_memory() {
        let cache = ImageCache::new(100);
        let mut fetches = 0;
        for _ in 0..3 {
            cache
                .get_or_fetch((1, Size::Large), || {
                    fetches += 1;
                    async { Ok(image(10)) }
                })
                .await
                .unwrap();
        }
        assert_eq!(fetches, 1);
    }

    #[tokio::test]
    async fn drops_oldest_when_full() {
        let cache = ImageCache::new(25);
        for index in 1..=3 {
            cache
                .get_or_fetch((index, Size::Large), || async { Ok(image(10)) })
                .await
                .unwrap();
        }
        assert!(cache.get((1, Size::Large)).is_none());
        assert!(cache.get((2, Size::Large)).is_some());
        assert!(cache.get((3, Size::Large)).is_some());
    }

    #[tokio::test]
    async fn failed_fetch_is_not_cached() {
        let cache = ImageCache::new(100);
        let result = cache
            .get_or_fetch((1, Size::Original), || async {
                Err("camera gone".to_string())
            })
            .await;
        assert!(result.is_err());
        assert!(cache.get((1, Size::Original)).is_none());
    }
}
