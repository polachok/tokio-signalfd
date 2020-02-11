use std::io::{self, Result};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};

use libc;

use mio::unix::EventedFd;
use mio::{self, Evented, PollOpt, Ready, Token};

use futures::{try_ready, Async, Poll, Stream};
use tokio_io::AsyncRead;
use tokio_reactor::PollEvented;

pub use libc::{SIGINT, SIGTERM};

#[repr(C)]
struct signalfd_siginfo {
    ssi_signo: u32,
    _dont_care: [u8; 124],
}

struct Inner(RawFd);

impl Inner {
    fn new(signals: &[libc::c_int]) -> Result<Self> {
        unsafe {
            let mut sig_set = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
            if libc::sigemptyset(sig_set.as_mut_ptr()) < 0 {
                return Err(io::Error::last_os_error());
            }
            let mut sig_set = sig_set.assume_init();
            for signal in signals {
                if libc::sigaddset(&mut sig_set, *signal) < 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            if libc::pthread_sigmask(libc::SIG_BLOCK, &sig_set, std::ptr::null_mut()) < 0 {
                return Err(io::Error::last_os_error());
            }
            let fd = libc::signalfd(-1, &sig_set, libc::SFD_NONBLOCK | libc::SFD_CLOEXEC);
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Inner(fd))
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}

impl io::Read for Inner {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let rv =
            unsafe { libc::read(self.0, buf.as_mut_ptr() as *mut std::ffi::c_void, buf.len()) };
        if rv < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(rv as usize)
    }
}

impl Evented for Inner {
    fn register(
        &self,
        poll: &mio::Poll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> Result<()> {
        poll.register(&EventedFd(&self.0), token, interest, opts)
    }

    fn reregister(
        &self,
        poll: &mio::Poll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> Result<()> {
        poll.reregister(&EventedFd(&self.0), token, interest, opts)
    }

    fn deregister(&self, poll: &mio::Poll) -> Result<()> {
        poll.deregister(&EventedFd(&self.0))
    }
}

pub struct SignalFd(PollEvented<Inner>);

impl SignalFd {
    pub fn new(signals: &[i32]) -> Result<Self> {
        let inner = Inner::new(signals)?;
        Ok(SignalFd(PollEvented::new(inner)))
    }
}

impl AsRawFd for SignalFd {
    fn as_raw_fd(&self) -> RawFd {
        self.0.get_ref().0
    }
}

impl FromRawFd for SignalFd {
    unsafe fn from_raw_fd(fd: RawFd) -> Self {
        SignalFd(PollEvented::new(Inner(fd)))
    }
}

impl io::Read for SignalFd {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.0.read(buf)
    }
}

impl AsyncRead for SignalFd {
    fn poll_read(&mut self, buf: &mut [u8]) -> Poll<usize, io::Error> {
        self.0.poll_read(buf)
    }
}

impl Stream for SignalFd {
    type Item = i32;
    type Error = io::Error;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        let mut buf = [0; std::mem::size_of::<signalfd_siginfo>()];

        try_ready!(self.0.poll_read_ready(Ready::readable()));

        match self.poll_read(&mut buf)? {
            Async::NotReady => Ok(Async::NotReady),
            Async::Ready(count) => {
                assert_eq!(count, std::mem::size_of::<signalfd_siginfo>());
                let mut signum = [0; 4];
                signum.copy_from_slice(&buf[0..4]);
                let signum = i32::from_ne_bytes(signum);
                Ok(Async::Ready(Some(signum)))
            }
        }
    }
}

mod tests {
    #[test]
    fn it_works() {
        use super::*;
        use tokio::prelude::*;

        let signals = SignalFd::new(&[SIGINT, SIGTERM]).unwrap();
        let fut = future::lazy(move || {
            unsafe {
                libc::raise(SIGINT);
            }
            signals
                .map_err(|err| panic!(err))
                .for_each(|signal| {
                    assert_eq!(signal, SIGINT);
                    Err("ok")
                })
                .map_err(|err| assert_eq!(err, "ok"))
        });
        tokio::run(fut);
    }
}
