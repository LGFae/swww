use std::error::Error;
use std::fmt;

use rustix::io::Errno;

/// Failiures if IPC with added context
#[derive(Debug)]
pub struct IpcError {
    err: Errno,
    kind: IpcErrorKind,
}

impl IpcError {
    pub(crate) fn new(kind: IpcErrorKind, err: Errno) -> Self {
        Self { err, kind }
    }
}

#[derive(Debug)]
pub enum IpcErrorKind {
    /// Failed to create file descriptor
    Socket,
    /// Failed to connect to socket
    Connect,
    /// Binding on socket failed
    Bind,
    /// Listening on socket failed
    Listen,
    /// Socket file wasn't found
    NoSocketFile,
    /// Socket timeout couldn't be set
    SetTimeout,
    /// IPC contained invalid identification code
    BadCode,
    /// IPC payload was broken
    MalformedMsg,
    /// Reading socket failed
    Read,
}

impl IpcErrorKind {
    fn description(&self) -> &'static str {
        match self {
            Self::Socket => "failed to create socket file descriptor",
            Self::Connect => "failed to connect to socket",
            Self::Bind => "failed to bind to socket",
            Self::Listen => "failed to listen on socket",
            Self::NoSocketFile => "Socket file not found. Make sure swww-daemon is running, and that the --namespace argument matches for the client and the daemon",
            Self::SetTimeout => "failed to set read timeout for socket",
            Self::BadCode => "invalid message code",
            Self::MalformedMsg => "malformed ancillary message",
            Self::Read => "failed to receive message",
        }
    }
}

impl fmt::Display for IpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.kind.description())
    }
}

impl Error for IpcError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.err)
    }
}

/// Simplify generating [`IpcError`]s from [`Errno`]
pub(crate) trait ErrnoExt {
    type Output;
    fn context(self, kind: IpcErrorKind) -> Self::Output;
}

impl ErrnoExt for Errno {
    type Output = IpcError;
    fn context(self, kind: IpcErrorKind) -> Self::Output {
        IpcError::new(kind, self)
    }
}

impl<T> ErrnoExt for Result<T, Errno> {
    type Output = Result<T, IpcError>;
    fn context(self, kind: IpcErrorKind) -> Self::Output {
        self.map_err(|error| error.context(kind))
    }
}
