//! Receiving file descriptors over D-Bus.
//!
//! A D-Bus value of type `h` is not a descriptor. It is an *index* into a list
//! of descriptors that travelled beside the message, in the ancillary data of
//! the `sendmsg` that carried it — `SCM_RIGHTS`, the Unix-socket mechanism for
//! passing an open file from one process to another.
//!
//! That has a consequence worth stating plainly: a reader using `read()` does
//! not merely ignore those descriptors, it *destroys* them. The kernel has
//! nowhere to put ancillary data when no control buffer is supplied, so it
//! discards it and closes the descriptors. There is no error and nothing in
//! the message body to suggest anything went missing.
//!
//! So receiving a descriptor is not a decoding change, it is a transport
//! change: the socket has to be read with `recvmsg` and a control buffer from
//! the very first byte, whether or not anything is expecting a descriptor yet.
//!
//! # Matching descriptors to messages
//!
//! D-Bus runs over a stream socket, so a `recvmsg` boundary is not a message
//! boundary — one read can span two messages, or half of one. Descriptors,
//! however, arrive attached to the read that carried the bytes they belong to,
//! and the specification requires a sender to attach them to the message whose
//! header declares them.
//!
//! The rule that follows is simple and is what this implements: descriptors go
//! into a queue in arrival order, and each parsed message takes the number its
//! `UNIX_FDS` header field declares off the front. A message declaring none
//! takes none, so an unrelated message in between cannot steal them.

use std::ffi::c_void;
use std::os::fd::RawFd;

#[repr(C)]
struct IoVec {
    base: *mut c_void,
    len: usize,
}

// `msghdr` and `cmsghdr` are libc's, laid out as the kernel expects. They are
// written out here for the same reason everything else in this crate is: the
// alternative is a dependency for two structs whose shape is fixed by the ABI.
#[repr(C)]
struct MsgHdr {
    name: *mut c_void,
    namelen: u32,
    iov: *mut IoVec,
    iovlen: usize,
    control: *mut c_void,
    controllen: usize,
    flags: i32,
}

#[repr(C)]
struct CmsgHdr {
    len: usize,
    level: i32,
    kind: i32,
}

const SOL_SOCKET: i32 = 1;
const SCM_RIGHTS: i32 = 1;
/// Enough for 16 descriptors, which is far more than any one D-Bus message
/// carries — BlueZ sends one.
const CONTROL_CAPACITY: usize = 256;

unsafe extern "C" {
    fn recvmsg(fd: RawFd, msg: *mut MsgHdr, flags: i32) -> isize;
    fn close(fd: RawFd) -> i32;
}

/// Round up to the alignment `cmsghdr` uses, which is the platform word.
const fn align(len: usize) -> usize {
    let a = std::mem::align_of::<usize>();
    (len + a - 1) & !(a - 1)
}

/// What one `recvmsg` produced.
pub struct Received {
    pub bytes: usize,
    pub fds: Vec<RawFd>,
}

/// Read from `fd`, collecting any descriptors that arrive with the data.
///
/// Returns `Ok(None)` if the call was interrupted, which the caller should
/// treat as "try again" rather than as an end of stream.
pub fn recv_with_fds(fd: RawFd, buffer: &mut [u8]) -> std::io::Result<Option<Received>> {
    let mut control = [0u8; CONTROL_CAPACITY];
    let mut iov = IoVec {
        base: buffer.as_mut_ptr() as *mut c_void,
        len: buffer.len(),
    };
    let mut header = MsgHdr {
        name: std::ptr::null_mut(),
        namelen: 0,
        iov: &mut iov,
        iovlen: 1,
        control: control.as_mut_ptr() as *mut c_void,
        controllen: control.len(),
        flags: 0,
    };

    let n = unsafe { recvmsg(fd, &mut header, 0) };
    if n < 0 {
        // `last_os_error` rather than a declared `__errno_location`: that
        // symbol is glibc's, and this file is compiled on the development
        // host too, where the equivalent is named something else.
        let e = std::io::Error::last_os_error();
        if e.kind() == std::io::ErrorKind::Interrupted {
            return Ok(None);
        }
        return Err(e);
    }

    // Walk the control messages. There is usually none; when BlueZ answers an
    // `AcquireWrite` there is exactly one, carrying one descriptor.
    let mut fds = Vec::new();
    let mut offset = 0usize;
    while offset + std::mem::size_of::<CmsgHdr>() <= header.controllen {
        // SAFETY: `offset` is within the control buffer, checked above.
        let cmsg = unsafe { &*(control.as_ptr().add(offset) as *const CmsgHdr) };
        if cmsg.len < std::mem::size_of::<CmsgHdr>() || offset + cmsg.len > header.controllen {
            break; // Malformed; stop rather than read past the buffer.
        }
        if cmsg.level == SOL_SOCKET && cmsg.kind == SCM_RIGHTS {
            let payload = cmsg.len - std::mem::size_of::<CmsgHdr>();
            let count = payload / std::mem::size_of::<RawFd>();
            let data = unsafe {
                control
                    .as_ptr()
                    .add(offset + std::mem::size_of::<CmsgHdr>())
            };
            for i in 0..count {
                // SAFETY: `count` was derived from the length the kernel set.
                let raw = unsafe { std::ptr::read_unaligned(data.cast::<RawFd>().add(i)) };
                fds.push(raw);
            }
        }
        offset += align(cmsg.len);
    }

    Ok(Some(Received {
        bytes: n as usize,
        fds,
    }))
}

/// Close a descriptor this module handed out.
///
/// # Safety
/// `fd` must be one received here and not already closed.
pub unsafe fn close_raw(fd: RawFd) {
    unsafe { close(fd) };
}

/// Descriptors received but not yet claimed by a message.
///
/// Kept in arrival order. Anything still here when the connection closes is
/// closed with it: a descriptor nobody claimed is a leaked open file, and the
/// process has no other handle on it.
#[derive(Default)]
pub struct Pending {
    queue: std::collections::VecDeque<RawFd>,
}

impl Pending {
    pub fn push(&mut self, fds: impl IntoIterator<Item = RawFd>) {
        self.queue.extend(fds);
    }

    /// Take `count` descriptors for a message that declared that many.
    ///
    /// Fewer than asked for means the peer's header disagreed with what it
    /// sent, which is a protocol error on their side; whatever is available is
    /// returned rather than blocking for something that is not coming.
    pub fn take(&mut self, count: usize) -> Vec<RawFd> {
        (0..count).filter_map(|_| self.queue.pop_front()).collect()
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        for fd in self.queue.drain(..) {
            // Nobody claimed it, and nothing else knows it exists.
            unsafe { close(fd) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_messages_are_word_aligned() {
        let word = std::mem::size_of::<usize>();
        assert_eq!(align(1), word);
        assert_eq!(align(word), word);
        assert_eq!(align(word + 1), word * 2);
    }

    /// The layouts are the kernel's, not ours. Getting one wrong means reading
    /// a descriptor out of the wrong offset — which yields a plausible-looking
    /// small integer rather than an error.
    #[test]
    fn the_headers_match_the_kernel_abi() {
        // `cmsghdr` is a length, then two ints.
        assert_eq!(
            std::mem::size_of::<CmsgHdr>(),
            std::mem::size_of::<usize>() + 2 * std::mem::size_of::<i32>(),
        );
        assert_eq!(
            std::mem::align_of::<CmsgHdr>(),
            std::mem::align_of::<usize>()
        );
        // `iovec` is a pointer and a length.
        assert_eq!(
            std::mem::size_of::<IoVec>(),
            std::mem::size_of::<*mut c_void>() + std::mem::size_of::<usize>(),
        );
    }

    /// Dropping these is safe because the queue is empty, which both tests
    /// here assert before they end.
    ///
    /// It matters: the descriptors are made up — 10, 11, 12 are somebody
    /// else's open files in this process — so a `Drop` that reached one would
    /// close it. An empty queue closes nothing.
    ///
    /// They used to end by forgetting the queue instead, which avoided the
    /// same hazard by never running `Drop` at all, and leaked its buffer —
    /// which is what `./scripts/check.sh miri` had been reporting.
    ///
    /// Nothing else catches this, so the assertions are the check. Miri is not
    /// a second opinion here: it shims `close` rather than refusing it, so a
    /// queue left full passes there and only bites on a real Linux run.
    #[test]
    fn a_message_takes_only_what_it_declared() {
        let mut pending = Pending::default();
        pending.push([10, 11, 12]);
        assert_eq!(pending.len(), 3);

        assert_eq!(pending.take(0), Vec::<RawFd>::new(), "a message with none");
        assert_eq!(pending.take(1), vec![10], "takes from the front");
        assert_eq!(pending.take(2), vec![11, 12]);
        assert!(pending.is_empty());

        // Asking for more than arrived yields what there is rather than
        // hanging for something the peer is not going to send.
        assert_eq!(pending.take(4), Vec::<RawFd>::new());
        assert!(
            pending.is_empty(),
            "or dropping it would close 10, 11 and 12"
        );
    }

    /// Order is the whole basis for matching descriptors to messages.
    #[test]
    fn descriptors_come_out_in_the_order_they_arrived() {
        let mut pending = Pending::default();
        pending.push([1, 2]);
        pending.push([3]);
        assert_eq!(pending.take(3), vec![1, 2, 3]);
        assert!(pending.is_empty(), "or dropping it would close 1, 2 and 3");
    }
}
