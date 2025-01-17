//! Primitives for working with TCP
//!
//! The types provided in this module are non-blocking by default and are
//! designed to be portable across all supported Mio platforms. As long as the
//! [portability guidelines] are followed, the behavior should be identical no
//! matter the target platform.
//!
//! [portability guidelines]: ../struct.Poll.html#portability

#[cfg(debug_assertions)]
use crate::poll::SelectorId;
use crate::{event, sys, Interests, Registry, Token};

use net2::TcpBuilder;
use std::fmt;
use std::io::{self, IoSlice, IoSliceMut, Read, Write};
use std::net::{self, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::time::Duration;

/*
 *
 * ===== TcpStream =====
 *
 */

/// A non-blocking TCP stream between a local socket and a remote socket.
///
/// The socket will be closed when the value is dropped.
///
/// # Examples
///
/// ```
/// # use std::net::TcpListener;
/// # use std::error::Error;
/// #
/// # fn main() -> Result<(), Box<dyn Error>> {
/// #     let _listener = TcpListener::bind("127.0.0.1:34254")?;
/// use mio::{Events, Interests, Poll, Token};
/// use mio::net::TcpStream;
/// use std::time::Duration;
///
/// let stream = TcpStream::connect("127.0.0.1:34254".parse()?)?;
///
/// let mut poll = Poll::new()?;
/// let registry = poll.registry().clone();
/// let mut events = Events::with_capacity(128);
///
/// // Register the socket with `Poll`
/// registry.register(&stream, Token(0), Interests::WRITABLE)?;
///
/// poll.poll(&mut events, Some(Duration::from_millis(100)))?;
///
/// // The socket might be ready at this point
/// #     Ok(())
/// # }
/// ```
pub struct TcpStream {
    sys: sys::TcpStream,
    #[cfg(debug_assertions)]
    selector_id: SelectorId,
}

use std::net::Shutdown;

fn set_nonblocking(stream: &net::TcpStream) -> io::Result<()> {
    stream.set_nonblocking(true)
}

impl TcpStream {
    /// Create a new TCP stream and issue a non-blocking connect to the
    /// specified address.
    ///
    /// This convenience method is available and uses the system's default
    /// options when creating a socket which is then connected. If fine-grained
    /// control over the creation of the socket is desired, you can use
    /// `net2::TcpBuilder` to configure a socket and then pass its socket to
    /// `TcpStream::connect_stream` to transfer ownership into mio and schedule
    /// the connect operation.
    pub fn connect(addr: SocketAddr) -> io::Result<TcpStream> {
        let sock = match addr {
            SocketAddr::V4(..) => TcpBuilder::new_v4(),
            SocketAddr::V6(..) => TcpBuilder::new_v6(),
        }?;
        // Required on Windows for a future `connect_overlapped` operation to be
        // executed successfully.
        if cfg!(windows) {
            sock.bind(inaddr_any(addr))?;
        }
        TcpStream::connect_stream(sock.to_tcp_stream()?, addr)
    }

    /// Creates a new `TcpStream` from the pending socket inside the given
    /// `std::net::TcpBuilder`, connecting it to the address specified.
    ///
    /// This constructor allows configuring the socket before it's actually
    /// connected, and this function will transfer ownership to the returned
    /// `TcpStream` if successful. An unconnected `TcpStream` can be created
    /// with the `net2::TcpBuilder` type (and also configured via that route).
    ///
    /// The platform specific behavior of this function looks like:
    ///
    /// * On Unix, the socket is placed into nonblocking mode and then a
    ///   `connect` call is issued.
    ///
    /// * On Windows, the address is stored internally and the connect operation
    ///   is issued when the returned `TcpStream` is registered with an event
    ///   loop. Note that on Windows you must `bind` a socket before it can be
    ///   connected, so if a custom `TcpBuilder` is used it should be bound
    ///   (perhaps to `INADDR_ANY`) before this method is called.
    pub fn connect_stream(stream: net::TcpStream, addr: SocketAddr) -> io::Result<TcpStream> {
        Ok(TcpStream {
            sys: sys::TcpStream::connect(stream, addr)?,
            #[cfg(debug_assertions)]
            selector_id: SelectorId::new(),
        })
    }

    /// Creates a new `TcpStream` from a standard `net::TcpStream`.
    ///
    /// This function is intended to be used to wrap a TCP stream from the
    /// standard library in the mio equivalent. The conversion here will
    /// automatically set `stream` to nonblocking and the returned object should
    /// be ready to get associated with an event loop.
    ///
    /// Note that the TCP stream here will not have `connect` called on it, so
    /// it should already be connected via some other means (be it manually, the
    /// net2 crate, or the standard library).
    pub fn from_stream(stream: net::TcpStream) -> io::Result<TcpStream> {
        set_nonblocking(&stream)?;

        Ok(TcpStream {
            sys: sys::TcpStream::from_stream(stream),
            #[cfg(debug_assertions)]
            selector_id: SelectorId::new(),
        })
    }

    /// Returns the socket address of the remote peer of this TCP connection.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.sys.peer_addr()
    }

    /// Returns the socket address of the local half of this TCP connection.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.sys.local_addr()
    }

    /// Creates a new independently owned handle to the underlying socket.
    ///
    /// The returned `TcpStream` is a reference to the same stream that this
    /// object references. Both handles will read and write the same stream of
    /// data, and options set on one stream will be propagated to the other
    /// stream.
    pub fn try_clone(&self) -> io::Result<TcpStream> {
        self.sys.try_clone().map(|s| TcpStream {
            sys: s,
            #[cfg(debug_assertions)]
            selector_id: self.selector_id.clone(),
        })
    }

    /// Shuts down the read, write, or both halves of this connection.
    ///
    /// This function will cause all pending and future I/O on the specified
    /// portions to return immediately with an appropriate value (see the
    /// documentation of `Shutdown`).
    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        self.sys.shutdown(how)
    }

    /// Sets the value of the `TCP_NODELAY` option on this socket.
    ///
    /// If set, this option disables the Nagle algorithm. This means that
    /// segments are always sent as soon as possible, even if there is only a
    /// small amount of data. When not set, data is buffered until there is a
    /// sufficient amount to send out, thereby avoiding the frequent sending of
    /// small packets.
    pub fn set_nodelay(&self, nodelay: bool) -> io::Result<()> {
        self.sys.set_nodelay(nodelay)
    }

    /// Gets the value of the `TCP_NODELAY` option on this socket.
    ///
    /// For more information about this option, see [`set_nodelay`][link].
    ///
    /// [link]: #method.set_nodelay
    pub fn nodelay(&self) -> io::Result<bool> {
        self.sys.nodelay()
    }

    /// Sets the value of the `SO_RCVBUF` option on this socket.
    ///
    /// Changes the size of the operating system's receive buffer associated
    /// with the socket.
    pub fn set_recv_buffer_size(&self, size: usize) -> io::Result<()> {
        self.sys.set_recv_buffer_size(size)
    }

    /// Gets the value of the `SO_RCVBUF` option on this socket.
    ///
    /// For more information about this option, see
    /// [`set_recv_buffer_size`][link].
    ///
    /// [link]: #method.set_recv_buffer_size
    pub fn recv_buffer_size(&self) -> io::Result<usize> {
        self.sys.recv_buffer_size()
    }

    /// Sets the value of the `SO_SNDBUF` option on this socket.
    ///
    /// Changes the size of the operating system's send buffer associated with
    /// the socket.
    pub fn set_send_buffer_size(&self, size: usize) -> io::Result<()> {
        self.sys.set_send_buffer_size(size)
    }

    /// Gets the value of the `SO_SNDBUF` option on this socket.
    ///
    /// For more information about this option, see
    /// [`set_send_buffer_size`][link].
    ///
    /// [link]: #method.set_send_buffer_size
    pub fn send_buffer_size(&self) -> io::Result<usize> {
        self.sys.send_buffer_size()
    }

    /// Sets whether keepalive messages are enabled to be sent on this socket.
    ///
    /// On Unix, this option will set the `SO_KEEPALIVE` as well as the
    /// `TCP_KEEPALIVE` or `TCP_KEEPIDLE` option (depending on your platform).
    /// On Windows, this will set the `SIO_KEEPALIVE_VALS` option.
    ///
    /// If `None` is specified then keepalive messages are disabled, otherwise
    /// the duration specified will be the time to remain idle before sending a
    /// TCP keepalive probe.
    ///
    /// Some platforms specify this value in seconds, so sub-second
    /// specifications may be omitted.
    pub fn set_keepalive(&self, keepalive: Option<Duration>) -> io::Result<()> {
        self.sys.set_keepalive(keepalive)
    }

    /// Returns whether keepalive messages are enabled on this socket, and if so
    /// the duration of time between them.
    ///
    /// For more information about this option, see [`set_keepalive`][link].
    ///
    /// [link]: #method.set_keepalive
    pub fn keepalive(&self) -> io::Result<Option<Duration>> {
        self.sys.keepalive()
    }

    /// Sets the value for the `IP_TTL` option on this socket.
    ///
    /// This value sets the time-to-live field that is used in every packet sent
    /// from this socket.
    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        self.sys.set_ttl(ttl)
    }

    /// Gets the value of the `IP_TTL` option for this socket.
    ///
    /// For more information about this option, see [`set_ttl`][link].
    ///
    /// [link]: #method.set_ttl
    pub fn ttl(&self) -> io::Result<u32> {
        self.sys.ttl()
    }

    /// Sets the value for the `SO_LINGER` option on this socket.
    pub fn set_linger(&self, dur: Option<Duration>) -> io::Result<()> {
        self.sys.set_linger(dur)
    }

    /// Gets the value of the `SO_LINGER` option on this socket.
    ///
    /// For more information about this option, see [`set_linger`][link].
    ///
    /// [link]: #method.set_linger
    pub fn linger(&self) -> io::Result<Option<Duration>> {
        self.sys.linger()
    }

    /// Get the value of the `SO_ERROR` option on this socket.
    ///
    /// This will retrieve the stored error in the underlying socket, clearing
    /// the field in the process. This can be useful for checking errors between
    /// calls.
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.sys.take_error()
    }

    /// Receives data on the socket from the remote address to which it is
    /// connected, without removing that data from the queue. On success,
    /// returns the number of bytes peeked.
    ///
    /// Successive calls return the same data. This is accomplished by passing
    /// `MSG_PEEK` as a flag to the underlying recv system call.
    pub fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.sys.peek(buf)
    }
}

fn inaddr_any(other: SocketAddr) -> SocketAddr {
    match other {
        SocketAddr::V4(..) => {
            let any = Ipv4Addr::new(0, 0, 0, 0);
            let addr = SocketAddrV4::new(any, 0);
            SocketAddr::V4(addr)
        }
        SocketAddr::V6(..) => {
            let any = Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 0);
            let addr = SocketAddrV6::new(any, 0, 0, 0);
            SocketAddr::V6(addr)
        }
    }
}

impl Read for TcpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&self.sys).read(buf)
    }

    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        (&self.sys).read_vectored(bufs)
    }
}

impl<'a> Read for &'a TcpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&self.sys).read(buf)
    }

    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        (&self.sys).read_vectored(bufs)
    }
}

impl Write for TcpStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&self.sys).write(buf)
    }

    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        (&self.sys).write_vectored(bufs)
    }

    fn flush(&mut self) -> io::Result<()> {
        (&self.sys).flush()
    }
}

impl<'a> Write for &'a TcpStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&self.sys).write(buf)
    }

    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        (&self.sys).write_vectored(bufs)
    }

    fn flush(&mut self) -> io::Result<()> {
        (&self.sys).flush()
    }
}

impl event::Source for TcpStream {
    fn register(&self, registry: &Registry, token: Token, interests: Interests) -> io::Result<()> {
        #[cfg(debug_assertions)]
        self.selector_id.associate_selector(registry)?;
        self.sys.register(registry, token, interests)
    }

    fn reregister(
        &self,
        registry: &Registry,
        token: Token,
        interests: Interests,
    ) -> io::Result<()> {
        self.sys.reregister(registry, token, interests)
    }

    fn deregister(&self, registry: &Registry) -> io::Result<()> {
        self.sys.deregister(registry)
    }
}

impl fmt::Debug for TcpStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.sys, f)
    }
}

/*
 *
 * ===== TcpListener =====
 *
 */

/// A structure representing a socket server
///
/// # Examples
///
/// ```
/// # use std::error::Error;
/// # fn main() -> Result<(), Box<dyn Error>> {
/// use mio::{Events, Interests, Poll, Token};
/// use mio::net::TcpListener;
/// use std::time::Duration;
///
/// let listener = TcpListener::bind("127.0.0.1:34255".parse()?)?;
///
/// let mut poll = Poll::new()?;
/// let registry = poll.registry().clone();
/// let mut events = Events::with_capacity(128);
///
/// // Register the socket with `Poll`
/// registry.register(&listener, Token(0), Interests::READABLE)?;
///
/// poll.poll(&mut events, Some(Duration::from_millis(100)))?;
///
/// // There may be a socket ready to be accepted
/// #     Ok(())
/// # }
/// ```
pub struct TcpListener {
    sys: sys::TcpListener,
    #[cfg(debug_assertions)]
    selector_id: SelectorId,
}

impl TcpListener {
    /// Convenience method to bind a new TCP listener to the specified address
    /// to receive new connections.
    ///
    /// This function will take the following steps:
    ///
    /// 1. Create a new TCP socket.
    /// 2. Set the `SO_REUSEADDR` option on the socket.
    /// 3. Bind the socket to the specified address.
    /// 4. Call `listen` on the socket to prepare it to receive new connections.
    ///
    /// If fine-grained control over the binding and listening process for a
    /// socket is desired then the `net2::TcpBuilder` methods can be used in
    /// combination with the `TcpListener::from_listener` method to transfer
    /// ownership into mio.
    pub fn bind(addr: SocketAddr) -> io::Result<TcpListener> {
        // Create the socket
        let sock = match addr {
            SocketAddr::V4(..) => TcpBuilder::new_v4(),
            SocketAddr::V6(..) => TcpBuilder::new_v6(),
        }?;

        // Set SO_REUSEADDR, but only on Unix (mirrors what libstd does)
        if cfg!(unix) {
            sock.reuse_address(true)?;
        }

        // Bind the socket
        sock.bind(addr)?;

        // listen
        let listener = sock.listen(1024)?;
        Ok(TcpListener {
            sys: sys::TcpListener::new(listener)?,
            #[cfg(debug_assertions)]
            selector_id: SelectorId::new(),
        })
    }

    /// Creates a new `TcpListener` from an instance of a
    /// `std::net::TcpListener` type.
    ///
    /// This function will set the `listener` provided into nonblocking mode on
    /// Unix, and otherwise the stream will just be wrapped up in an mio stream
    /// ready to accept new connections and become associated with an event
    /// loop.
    ///
    /// The address provided must be the address that the listener is bound to.
    pub fn from_std(listener: net::TcpListener) -> io::Result<TcpListener> {
        sys::TcpListener::new(listener).map(|s| TcpListener {
            sys: s,
            #[cfg(debug_assertions)]
            selector_id: SelectorId::new(),
        })
    }

    /// Accepts a new `TcpStream`.
    ///
    /// This may return an `Err(e)` where `e.kind()` is
    /// `io::ErrorKind::WouldBlock`. This means a stream may be ready at a later
    /// point and one should wait for an event before calling `accept` again.
    ///
    /// If an accepted stream is returned, the remote address of the peer is
    /// returned along with it.
    pub fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        let (s, a) = self.accept_std()?;
        Ok((TcpStream::from_stream(s)?, a))
    }

    /// Accepts a new `std::net::TcpStream`.
    ///
    /// This method is the same as `accept`, except that it returns a TCP socket
    /// *in blocking mode* which isn't bound to `mio`. This can be later then
    /// converted to a `mio` type, if necessary.
    pub fn accept_std(&self) -> io::Result<(net::TcpStream, SocketAddr)> {
        self.sys.accept()
    }

    /// Returns the local socket address of this listener.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.sys.local_addr()
    }

    /// Creates a new independently owned handle to the underlying socket.
    ///
    /// The returned `TcpListener` is a reference to the same socket that this
    /// object references. Both handles can be used to accept incoming
    /// connections and options set on one listener will affect the other.
    pub fn try_clone(&self) -> io::Result<TcpListener> {
        self.sys.try_clone().map(|s| TcpListener {
            sys: s,
            #[cfg(debug_assertions)]
            selector_id: self.selector_id.clone(),
        })
    }

    /// Sets the value for the `IP_TTL` option on this socket.
    ///
    /// This value sets the time-to-live field that is used in every packet sent
    /// from this socket.
    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        self.sys.set_ttl(ttl)
    }

    /// Gets the value of the `IP_TTL` option for this socket.
    ///
    /// For more information about this option, see [`set_ttl`][link].
    ///
    /// [link]: #method.set_ttl
    pub fn ttl(&self) -> io::Result<u32> {
        self.sys.ttl()
    }

    /// Get the value of the `SO_ERROR` option on this socket.
    ///
    /// This will retrieve the stored error in the underlying socket, clearing
    /// the field in the process. This can be useful for checking errors between
    /// calls.
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.sys.take_error()
    }
}

impl event::Source for TcpListener {
    fn register(&self, registry: &Registry, token: Token, interests: Interests) -> io::Result<()> {
        #[cfg(debug_assertions)]
        self.selector_id.associate_selector(registry)?;
        self.sys.register(registry, token, interests)
    }

    fn reregister(
        &self,
        registry: &Registry,
        token: Token,
        interests: Interests,
    ) -> io::Result<()> {
        self.sys.reregister(registry, token, interests)
    }

    fn deregister(&self, registry: &Registry) -> io::Result<()> {
        self.sys.deregister(registry)
    }
}

impl fmt::Debug for TcpListener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.sys, f)
    }
}

/*
 *
 * ===== UNIX ext =====
 *
 */

#[cfg(unix)]
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};

#[cfg(unix)]
impl IntoRawFd for TcpStream {
    fn into_raw_fd(self) -> RawFd {
        self.sys.into_raw_fd()
    }
}

#[cfg(unix)]
impl AsRawFd for TcpStream {
    fn as_raw_fd(&self) -> RawFd {
        self.sys.as_raw_fd()
    }
}

#[cfg(unix)]
impl FromRawFd for TcpStream {
    unsafe fn from_raw_fd(fd: RawFd) -> TcpStream {
        TcpStream {
            sys: FromRawFd::from_raw_fd(fd),
            #[cfg(debug_assertions)]
            selector_id: SelectorId::new(),
        }
    }
}

#[cfg(unix)]
impl IntoRawFd for TcpListener {
    fn into_raw_fd(self) -> RawFd {
        self.sys.into_raw_fd()
    }
}

#[cfg(unix)]
impl AsRawFd for TcpListener {
    fn as_raw_fd(&self) -> RawFd {
        self.sys.as_raw_fd()
    }
}

#[cfg(unix)]
impl FromRawFd for TcpListener {
    unsafe fn from_raw_fd(fd: RawFd) -> TcpListener {
        TcpListener {
            sys: FromRawFd::from_raw_fd(fd),
            #[cfg(debug_assertions)]
            selector_id: SelectorId::new(),
        }
    }
}
