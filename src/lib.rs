use std::{cell::UnsafeCell, future::Future, mem::MaybeUninit, ops::{Deref, DerefMut}, pin::Pin, ptr::NonNull, sync::atomic::{AtomicUsize, Ordering}, task::Waker};
use futures::{future::FusedFuture, task::noop_waker};

struct Inner<T: ?Sized> {
    count: AtomicUsize,
    waker: UnsafeCell<MaybeUninit<Waker>>,
    data: UnsafeCell<T>
}

// MARK: Unique

pub struct Unique<T: ?Sized>(*const Inner<T>);

unsafe impl<T: ?Sized + Send> Send for Unique<T> {}

unsafe impl<T: ?Sized + Sync> Sync for Unique<T> {}

impl<T> Unique<T> {
    pub fn new(t: T) -> Self {
        Unique(Box::into_raw(Box::new(Inner {
            count: 1.into(),
            waker: UnsafeCell::new(MaybeUninit::uninit()),
            data: UnsafeCell::new(t)
        })))
    }

    pub fn into_inner(self) -> T {
        let t = unsafe { Box::from_raw(self.0 as *mut Inner<T>) }.data.into_inner();
        std::mem::forget(self);
        t
    }
}

impl<T: ?Sized> Unique<T> {
    pub fn pin(unique: Self) -> Pin<Self> {
        unsafe { Pin::new_unchecked(unique) }
    }

    pub fn share(self) -> (Host<T>, Share<T>) {
        let inner = unsafe { (self.0 as *mut Inner<T>).as_mut().unwrap_unchecked() };
        *inner.count.get_mut() = 2;
        inner.waker.get_mut().write(noop_waker());
        let host = Host(Some(unsafe { NonNull::new_unchecked(self.0 as *mut Inner<T>) }));
        let share = Share(self.0);
        std::mem::forget(self);
        (host, share)
    }

    pub fn share_mut(self) -> (HostMut<T>, ShareMut<T>) {
        let inner = unsafe { (self.0 as *mut Inner<T>).as_mut().unwrap_unchecked() };
        *inner.count.get_mut() = 2;
        inner.waker.get_mut().write(noop_waker());
        let host = HostMut(Some(unsafe { NonNull::new_unchecked(self.0 as *mut Inner<T>) }));
        let share = ShareMut(self.0 as *mut Inner<T>);
        std::mem::forget(self);
        (host, share)
    }

    pub fn share_pinned(pin: Pin<Self>) -> (Host<T>, Share<T>) {
        let this = unsafe { Pin::into_inner_unchecked(pin) };
        let inner = unsafe { (this.0 as *mut Inner<T>).as_mut().unwrap_unchecked() };
        *inner.count.get_mut() = 2;
        inner.waker.get_mut().write(noop_waker());
        let host = Host(Some(unsafe { NonNull::new_unchecked(this.0 as *mut Inner<T>) }));
        let share = Share(this.0);
        std::mem::forget(this);
        (host, share)
    }

    pub fn share_pinned_mut(pin: Pin<Self>) -> (HostMut<T>, ShareMut<T>) {
        let this = unsafe { Pin::into_inner_unchecked(pin) };
        let inner = unsafe { (this.0 as *mut Inner<T>).as_mut().unwrap_unchecked() };
        *inner.count.get_mut() = 2;
        inner.waker.get_mut().write(noop_waker());
        let host = HostMut(Some(unsafe { NonNull::new_unchecked(this.0 as *mut Inner<T>) }));
        let share = ShareMut(this.0 as *mut Inner<T>);
        std::mem::forget(this);
        (host, share)
    }
} 

impl<T: ?Sized> Deref for Unique<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { self.0.as_ref().unwrap_unchecked().data.get().as_ref().unwrap_unchecked() }
    }
}

impl<T: ?Sized> DerefMut for Unique<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { (self.0 as *mut Inner<T>).as_mut().unwrap_unchecked() }.data.get_mut()
    }
}

impl<T: ?Sized> Drop for Unique<T> {
    fn drop(&mut self) {
        drop(unsafe { Box::from_raw(self.0 as *mut Inner<T>) })
    }
}

// MARK: Share / ShareMut

pub struct Share<T: ?Sized>(*const Inner<T>);

unsafe impl<T: ?Sized + Send + Sync> Send for Share<T> {}

unsafe impl<T: ?Sized + Send + Sync> Sync for Share<T> {}

impl<T: ?Sized> Deref for Share<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { self.0.as_ref().unwrap_unchecked().data.get().as_ref().unwrap_unchecked() }
    }
}

impl<T: ?Sized> Clone for Share<T> {
    fn clone(&self) -> Self {
        let count = &unsafe { self.0.as_ref().unwrap_unchecked() }.count;
        loop {
            match count.load(Ordering::Relaxed) {
                0 => continue, // locked
                n => match count.compare_exchange_weak(n, n+1, Ordering::Relaxed, Ordering::Relaxed) {
                    Ok(_) => break,
                    Err(_) => continue,
                }
            }
            #[expect(unreachable_code)]
            {
                unreachable!()
            }
        }
        Share(self.0)
    }
}

impl<T: ?Sized> Drop for Share<T> {
    fn drop(&mut self) {
        let inner = unsafe { self.0.as_ref().unwrap_unchecked() };
        loop {
            match inner.count.load(Ordering::Relaxed) {
                0 => continue, // locked
                1 => break drop(unsafe { Box::from_raw(inner as *const Inner<T> as *mut Inner<T>) }),
                2 => match inner.count.compare_exchange_weak(2, 0, Ordering::AcqRel, Ordering::Relaxed) {
                    Ok(_) => {
                        unsafe { inner
                            .waker
                            .get()
                            .as_mut()
                            .unwrap_unchecked()
                            .assume_init_read()
                            .wake();
                        };
                        inner.count.store(1, Ordering::Release);
                        break;
                    },
                    Err(_) => continue,
                },
                n => match inner.count.compare_exchange_weak(n, n-1, Ordering::Release, Ordering::Relaxed) {
                    Ok(_) => break,
                    Err(_) => continue,
                }
            }
            #[expect(unreachable_code)]
            {
                unreachable!()
            }
        }
    }
}

pub struct ShareMut<T: ?Sized>(*mut Inner<T>);

unsafe impl<T: ?Sized + Send> Send for ShareMut<T> {}

unsafe impl<T: ?Sized + Sync> Sync for ShareMut<T> {}

impl<T: ?Sized> ShareMut<T> {
    pub fn into_share(self) -> Share<T> {
        let share = Share(self.0);
        std::mem::forget(self);
        share
    }

    pub fn pinned_into_share(pin: Pin<Self>) -> Share<T> {
        unsafe { Pin::into_inner_unchecked(pin) }.into_share()
    }
}

impl<T: ?Sized> Deref for ShareMut<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { self.0.as_ref().unwrap_unchecked().data.get().as_ref().unwrap_unchecked() }
    }
}

impl<T: ?Sized> DerefMut for ShareMut<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.0.as_ref().unwrap_unchecked().data.get().as_mut().unwrap_unchecked() }
    }
}

impl<T: ?Sized> Drop for ShareMut<T> {
    fn drop(&mut self) {
        let inner = unsafe { self.0.as_ref().unwrap_unchecked() };
        loop {
            match inner.count.load(Ordering::Relaxed) {
                0 => continue, // locked
                1 => break drop(unsafe { Box::from_raw(inner as *const Inner<T> as *mut Inner<T>) }),
                2 => match inner.count.compare_exchange_weak(2, 0, Ordering::AcqRel, Ordering::Relaxed) {
                    Ok(_) => {
                        unsafe { inner
                            .waker
                            .get()
                            .as_mut()
                            .unwrap_unchecked()
                            .assume_init_read()
                            .wake();
                        };
                        inner.count.store(1, Ordering::Release);
                        break;
                    },
                    Err(_) => continue,
                },
                _ => unreachable!()
            }
            #[expect(unreachable_code)]
            {
                unreachable!()
            }
        }
    }
}

// MARK: Host / HostMut

pub struct Host<T: ?Sized>(Option<NonNull<Inner<T>>>);

unsafe impl<T: ?Sized + Send + Sync> Send for Host<T> {}

unsafe impl<T: ?Sized + Send + Sync> Sync for Host<T> {}

impl<T: ?Sized> Host<T> {
    pub fn count_checked(&self) -> Option<usize> {
        Some(unsafe { self.0.as_ref()?.as_ref().count.load(Ordering::Relaxed) })
    }

    pub fn count(&self) -> usize {
        self.count_checked().unwrap()
    }

    pub fn get_checked(&self) -> Option<&T> {
        Some(unsafe { self.0.as_ref()?.as_ref().data.get().as_ref().unwrap_unchecked() })
    }

    pub fn get(&self) -> &T {
        self.get_checked().unwrap()
    }

    pub fn share_checked(&self) -> Option<Share<T>> {
        let count = &unsafe { self.0.as_ref()?.as_ref() }.count;
        loop {
            match count.load(Ordering::Relaxed) {
                0 => continue, // locked
                n => match count.compare_exchange_weak(n, n+1, Ordering::Relaxed, Ordering::Relaxed) {
                    Ok(_) => break,
                    Err(_) => continue,
                }
            }
            #[expect(unreachable_code)]
            {
                unreachable!()
            }
        }
        Some(Share(unsafe { self.0.as_ref().unwrap_unchecked().as_ptr() }))
    }

    pub fn share(&self) -> Share<T> {
        self.share_checked().unwrap()
    }

    pub fn into_host_mut(self) -> HostMut<T> {
        let host = HostMut(self.0);
        std::mem::forget(self);
        host
    }
}

impl<T: ?Sized> Future for Host<T> {
    type Output = Unique<T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        let inner = unsafe { self.0.as_ref().unwrap().as_ref() };
        loop {
            match inner.count.load(Ordering::Relaxed) {
                0 => {
                    while inner.count.load(Ordering::Acquire) != 1 {}
                    break std::task::Poll::Ready(Unique(unsafe { self.0.take().unwrap_unchecked() }.as_ptr()));
                }
                1 => break std::task::Poll::Ready(Unique(unsafe { self.0.take().unwrap_unchecked() }.as_ptr())),
                n => match inner.count.compare_exchange_weak(n, 0, Ordering::Acquire, Ordering::Relaxed) {
                    Ok(_) => {
                        let waker = unsafe { inner.waker.get().as_mut().unwrap_unchecked() };
                        if !unsafe{ waker.assume_init_ref() }.will_wake(cx.waker()) {
                            drop(unsafe { waker.assume_init_read() });
                            waker.write(cx.waker().clone());
                        }
                        inner.count.store(n, Ordering::Release);
                        break std::task::Poll::Pending;
                    },
                    Err(_) => continue,
                }
            }
            #[expect(unreachable_code)]
            {
                unreachable!()
            }
        }
    }
}

impl<T: ?Sized> FusedFuture for Host<T> {
    fn is_terminated(&self) -> bool {
        self.0.is_none()
    }
}

impl<T: ?Sized> Drop for Host<T> {
    fn drop(&mut self) {
        let Some(inner) = (unsafe { self.0.as_ref().map(|ptr| ptr.as_ref()) }) else { return };
        loop {
            match inner.count.load(Ordering::Relaxed) {
                0 => continue, // locked
                1 => break drop(unsafe { Box::from_raw(inner as *const Inner<T> as *mut Inner<T>) }),
                2 => match inner.count.compare_exchange_weak(2, 0, Ordering::AcqRel, Ordering::Relaxed) {
                    Ok(_) => {
                        unsafe { inner
                            .waker
                            .get()
                            .as_mut()
                            .unwrap_unchecked()
                            .assume_init_read()
                            .wake();
                        };
                        inner.count.store(1, Ordering::Release);
                        break;
                    },
                    Err(_) => continue,
                },
                n => match inner.count.compare_exchange_weak(n, n-1, Ordering::Release, Ordering::Relaxed) {
                    Ok(_) => break,
                    Err(_) => continue,
                }
            }
            #[expect(unreachable_code)]
            {
                unreachable!()
            }
        }
    }
}

pub struct HostMut<T: ?Sized>(Option<NonNull<Inner<T>>>);

unsafe impl<T: ?Sized + Send> Send for HostMut<T> {}

unsafe impl<T: ?Sized + Sync> Sync for HostMut<T> {}

impl<T: ?Sized> HostMut<T> {
    pub fn count_checked(&self) -> Option<usize> {
        Some(unsafe { self.0.as_ref()?.as_ref().count.load(Ordering::Relaxed) })
    }

    pub fn count(&self) -> usize {
        self.count_checked().unwrap()
    }
}

impl<T: ?Sized> Future for HostMut<T> {
    type Output = Unique<T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        let inner = unsafe { self.0.as_ref().unwrap().as_ref() };
        loop {
            match inner.count.load(Ordering::Relaxed) {
                0 => {
                    while inner.count.load(Ordering::Acquire) != 1 {}
                    break std::task::Poll::Ready(Unique(unsafe { self.0.take().unwrap_unchecked() }.as_ptr()));
                }
                1 => break std::task::Poll::Ready(Unique(unsafe { self.0.take().unwrap_unchecked() }.as_ptr())),
                n => match inner.count.compare_exchange_weak(n, 0, Ordering::Acquire, Ordering::Relaxed) {
                    Ok(_) => {
                        let waker = unsafe { inner.waker.get().as_mut().unwrap_unchecked() };
                        if !unsafe{ waker.assume_init_ref() }.will_wake(cx.waker()) {
                            drop(unsafe { waker.assume_init_read() });
                            waker.write(cx.waker().clone());
                        }
                        inner.count.store(n, Ordering::Release);
                        break std::task::Poll::Pending;
                    },
                    Err(_) => continue,
                }
            }
            #[expect(unreachable_code)]
            {
                unreachable!()
            }
        }
    }
}

impl<T: ?Sized> FusedFuture for HostMut<T> {
    fn is_terminated(&self) -> bool {
        self.0.is_none()
    }
}

impl<T: ?Sized> Drop for HostMut<T> {
    fn drop(&mut self) {
        let Some(inner) = (unsafe { self.0.as_ref().map(|ptr| ptr.as_ref()) }) else { return };
        loop {
            match inner.count.load(Ordering::Relaxed) {
                0 => continue, // locked
                1 => break drop(unsafe { Box::from_raw(inner as *const Inner<T> as *mut Inner<T>) }),
                2 => match inner.count.compare_exchange_weak(2, 0, Ordering::AcqRel, Ordering::Relaxed) {
                    Ok(_) => {
                        unsafe { inner
                            .waker
                            .get()
                            .as_mut()
                            .unwrap_unchecked()
                            .assume_init_read()
                            .wake();
                        };
                        inner.count.store(1, Ordering::Release);
                        break;
                    },
                    Err(_) => continue,
                },
                n => match inner.count.compare_exchange_weak(n, n-1, Ordering::Release, Ordering::Relaxed) {
                    Ok(_) => break,
                    Err(_) => continue,
                }
            }
            #[expect(unreachable_code)]
            {
                unreachable!()
            }
        }
    }
}

// MARK: HostPinned / HostPinnedMut

pub struct HostPinned<T: ?Sized>(Option<NonNull<Inner<T>>>);

unsafe impl<T: ?Sized + Send + Sync> Send for HostPinned<T> {}

unsafe impl<T: ?Sized + Send + Sync> Sync for HostPinned<T> {}

impl<T: ?Sized> HostPinned<T> {
    pub fn from_unpinned(host: Host<T>) -> Self {
        let host_pinned = HostPinned(host.0);
        std::mem::forget(host);
        host_pinned
    }

    pub fn count_checked(&self) -> Option<usize> {
        Some(unsafe { self.0.as_ref()?.as_ref().count.load(Ordering::Relaxed) })
    }

    pub fn count(&self) -> usize {
        self.count_checked().unwrap()
    }

    pub fn get_checked(&self) -> Option<&T> {
        Some(unsafe { self.0.as_ref()?.as_ref().data.get().as_ref().unwrap_unchecked() })
    }

    pub fn get(&self) -> &T {
        self.get_checked().unwrap()
    }

    pub fn share_checked(&self) -> Option<Share<T>> {
        let count = &unsafe { self.0.as_ref()?.as_ref() }.count;
        loop {
            match count.load(Ordering::Relaxed) {
                0 => continue, // locked
                n => match count.compare_exchange_weak(n, n+1, Ordering::Relaxed, Ordering::Relaxed) {
                    Ok(_) => break,
                    Err(_) => continue,
                }
            }
            #[expect(unreachable_code)]
            {
                unreachable!()
            }
        }
        Some(Share(unsafe { self.0.as_ref().unwrap_unchecked().as_ptr() }))
    }

    pub fn share(&self) -> Share<T> {
        self.share_checked().unwrap()
    }

    pub fn into_host_mut(self) -> HostPinnedMut<T> {
        let host = HostPinnedMut(self.0);
        std::mem::forget(self);
        host
    }
}

impl<T: ?Sized> Future for HostPinned<T> {
    type Output = Pin<Unique<T>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        let inner = unsafe { self.0.as_ref().unwrap().as_ref() };
        loop {
            match inner.count.load(Ordering::Relaxed) {
                0 => {
                    while inner.count.load(Ordering::Acquire) != 1 {}
                    break std::task::Poll::Ready(unsafe { Pin::new_unchecked(Unique(self.0.take().unwrap_unchecked().as_ptr())) });
                }
                1 => break std::task::Poll::Ready(unsafe { Pin::new_unchecked(Unique(self.0.take().unwrap_unchecked().as_ptr())) }),
                n => match inner.count.compare_exchange_weak(n, 0, Ordering::Acquire, Ordering::Relaxed) {
                    Ok(_) => {
                        let waker = unsafe { inner.waker.get().as_mut().unwrap_unchecked() };
                        if !unsafe{ waker.assume_init_ref() }.will_wake(cx.waker()) {
                            drop(unsafe { waker.assume_init_read() });
                            waker.write(cx.waker().clone());
                        }
                        inner.count.store(n, Ordering::Release);
                        break std::task::Poll::Pending;
                    },
                    Err(_) => continue,
                }
            }
            #[expect(unreachable_code)]
            {
                unreachable!()
            }
        }
    }
}

impl<T: ?Sized> FusedFuture for HostPinned<T> {
    fn is_terminated(&self) -> bool {
        self.0.is_none()
    }
}

impl<T: ?Sized> Drop for HostPinned<T> {
    fn drop(&mut self) {
        let Some(inner) = (unsafe { self.0.as_ref().map(|ptr| ptr.as_ref()) }) else { return };
        loop {
            match inner.count.load(Ordering::Relaxed) {
                0 => continue, // locked
                1 => break drop(unsafe { Box::from_raw(inner as *const Inner<T> as *mut Inner<T>) }),
                2 => match inner.count.compare_exchange_weak(2, 0, Ordering::AcqRel, Ordering::Relaxed) {
                    Ok(_) => {
                        unsafe { inner
                            .waker
                            .get()
                            .as_mut()
                            .unwrap_unchecked()
                            .assume_init_read()
                            .wake();
                        };
                        inner.count.store(1, Ordering::Release);
                        break;
                    },
                    Err(_) => continue,
                },
                n => match inner.count.compare_exchange_weak(n, n-1, Ordering::Release, Ordering::Relaxed) {
                    Ok(_) => break,
                    Err(_) => continue,
                }
            }
            #[expect(unreachable_code)]
            {
                unreachable!()
            }
        }
    }
}

pub struct HostPinnedMut<T: ?Sized>(Option<NonNull<Inner<T>>>);

unsafe impl<T: ?Sized + Send> Send for HostPinnedMut<T> {}

unsafe impl<T: ?Sized + Sync> Sync for HostPinnedMut<T> {}

impl<T: ?Sized> HostPinnedMut<T> {
    pub fn from_unpinned(host: HostMut<T>) -> Self {
        let host_pinned = HostPinnedMut(host.0);
        std::mem::forget(host);
        host_pinned
    }

    pub fn count_checked(&self) -> Option<usize> {
        Some(unsafe { self.0.as_ref()?.as_ref().count.load(Ordering::Relaxed) })
    }

    pub fn count(&self) -> usize {
        self.count_checked().unwrap()
    }
}

impl<T: ?Sized> Future for HostPinnedMut<T> {
    type Output = Pin<Unique<T>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        let inner = unsafe { self.0.as_ref().unwrap().as_ref() };
        loop {
            match inner.count.load(Ordering::Relaxed) {
                0 => {
                    while inner.count.load(Ordering::Acquire) != 1 {}
                    break std::task::Poll::Ready(unsafe { Pin::new_unchecked(Unique(self.0.take().unwrap_unchecked().as_ptr())) });
                }
                1 => break std::task::Poll::Ready(unsafe { Pin::new_unchecked(Unique(self.0.take().unwrap_unchecked().as_ptr())) }),
                n => match inner.count.compare_exchange_weak(n, 0, Ordering::Acquire, Ordering::Relaxed) {
                    Ok(_) => {
                        let waker = unsafe { inner.waker.get().as_mut().unwrap_unchecked() };
                        if !unsafe{ waker.assume_init_ref() }.will_wake(cx.waker()) {
                            drop(unsafe { waker.assume_init_read() });
                            waker.write(cx.waker().clone());
                        }
                        inner.count.store(n, Ordering::Release);
                        break std::task::Poll::Pending;
                    },
                    Err(_) => continue,
                }
            }
            #[expect(unreachable_code)]
            {
                unreachable!()
            }
        }
    }
}

impl<T: ?Sized> FusedFuture for HostPinnedMut<T> {
    fn is_terminated(&self) -> bool {
        self.0.is_none()
    }
}

impl<T: ?Sized> Drop for HostPinnedMut<T> {
    fn drop(&mut self) {
        let Some(inner) = (unsafe { self.0.as_ref().map(|ptr| ptr.as_ref()) }) else { return };
        loop {
            match inner.count.load(Ordering::Relaxed) {
                0 => continue, // locked
                1 => break drop(unsafe { Box::from_raw(inner as *const Inner<T> as *mut Inner<T>) }),
                2 => match inner.count.compare_exchange_weak(2, 0, Ordering::AcqRel, Ordering::Relaxed) {
                    Ok(_) => {
                        unsafe { inner
                            .waker
                            .get()
                            .as_mut()
                            .unwrap_unchecked()
                            .assume_init_read()
                            .wake();
                        };
                        inner.count.store(1, Ordering::Release);
                        break;
                    },
                    Err(_) => continue,
                },
                n => match inner.count.compare_exchange_weak(n, n-1, Ordering::Release, Ordering::Relaxed) {
                    Ok(_) => break,
                    Err(_) => continue,
                }
            }
            #[expect(unreachable_code)]
            {
                unreachable!()
            }
        }
    }
}

#[cfg(test)]
mod test {
    use crate::Unique;

    #[tokio::test]
    async fn vibe_check() {
        let unique = Unique::new(0);
        let (host, mut share) = unique.share_mut();
        tokio::task::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(16)).await;
            *share += 1;
        });
        let unique = host.await;
        assert_eq!(unique.into_inner(), 1)
    }
}