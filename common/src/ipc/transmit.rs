use std::thread;
use std::time::Duration;

use rustix::io;
use rustix::io::Errno;
use rustix::net;
use rustix::net::RecvFlags;

use super::serde::Cursor;
use super::serde::Deserialize;
use super::serde::Serialize;
use super::types2::ClearRequest;
use super::types2::Info;
use super::types2::Request;
use super::types2::Response;
use super::Animation;
use super::Answer;
use super::BgInfo;
use super::ClearReq;
use super::Client;
use super::ErrnoExt;
use super::ImageReq;
use super::ImgReq;
use super::IpcError;
use super::IpcErrorKind;
use super::IpcSocket;
use super::RequestRecv;
use super::RequestSend;
use super::Server;
use super::Transition;
use crate::ipc::types2::ImageRequest;
use crate::mmap::Mmap;
use crate::mmap::MmappedStr;

// could be enum
pub struct IpcMessage {
    code: Code,
    shm: Option<Mmap>,
}

impl From<RequestSend> for IpcMessage {
    fn from(value: RequestSend) -> Self {
        let code = match value {
            RequestSend::Ping => Code::ReqPing,
            RequestSend::Query => Code::ReqQuery,
            RequestSend::Clear(_) => Code::ReqClear,
            RequestSend::Img(_) => Code::ReqImg,
            RequestSend::Kill => Code::ReqKill,
        };

        let shm = match value {
            RequestSend::Clear(mem) | RequestSend::Img(mem) => Some(mem),
            _ => None,
        };

        Self { code, shm }
    }
}

impl From<Answer> for IpcMessage {
    fn from(value: Answer) -> Self {
        let code = match value {
            Answer::Ok => Code::ResOk,
            Answer::Ping(true) => Code::ResConfigured,
            Answer::Ping(false) => Code::ResAwait,
            Answer::Info(_) => Code::ResInfo,
        };

        let shm = if let Answer::Info(infos) = value {
            let len = 1 + infos
                .iter()
                .map(|info| info.serialized_size())
                .sum::<usize>();
            let mut mmap = Mmap::create(len);
            let bytes = mmap.slice_mut();

            bytes[0] = infos.len() as u8;
            let mut i = 1;

            for info in infos.iter() {
                i += info.serialize(&mut bytes[i..]);
            }

            Some(mmap)
        } else {
            None
        };

        Self { code, shm }
    }
}

// TODO: remove this ugly mess
impl From<IpcMessage> for RequestRecv {
    fn from(value: IpcMessage) -> Self {
        match value.code {
            Code::ReqPing => Self::Ping,
            Code::ReqQuery => Self::Query,
            Code::ReqClear => {
                let mmap = value.shm.unwrap();
                let bytes = mmap.slice();
                let len = bytes[0] as usize;
                let mut outputs = Vec::with_capacity(len);
                let mut i = 1;
                for _ in 0..len {
                    let output = MmappedStr::new(&mmap, &bytes[i..]);
                    i += 4 + output.str().len();
                    outputs.push(output);
                }
                let color = [bytes[i], bytes[i + 1], bytes[i + 2]];
                Self::Clear(ClearReq {
                    color,
                    outputs: outputs.into(),
                })
            }
            Code::ReqImg => {
                let mmap = value.shm.unwrap();
                let bytes = mmap.slice();
                let transition = Transition::deserialize(&bytes[0..]);
                let len = bytes[51] as usize;

                let mut imgs = Vec::with_capacity(len);
                let mut outputs = Vec::with_capacity(len);
                let mut animations = Vec::with_capacity(len);

                let mut i = 52;
                for _ in 0..len {
                    let (img, offset) = ImgReq::deserialize(&mmap, &bytes[i..]);
                    i += offset;
                    imgs.push(img);

                    let n_outputs = bytes[i] as usize;
                    i += 1;
                    let mut out = Vec::with_capacity(n_outputs);
                    for _ in 0..n_outputs {
                        let output = MmappedStr::new(&mmap, &bytes[i..]);
                        i += 4 + output.str().len();
                        out.push(output);
                    }
                    outputs.push(out.into());

                    if bytes[i] == 1 {
                        let (animation, offset) = Animation::deserialize(&mmap, &bytes[i + 1..]);
                        i += offset;
                        animations.push(animation);
                    }
                    i += 1;
                }

                Self::Img(ImageReq {
                    transition,
                    imgs: imgs.into(),
                    outputs: outputs.into(),
                    animations: if animations.is_empty() {
                        None
                    } else {
                        Some(animations.into())
                    },
                })
            }
            Code::ReqKill => Self::Kill,
            _ => Self::Kill,
        }
    }
}

impl From<IpcMessage> for Answer {
    fn from(value: IpcMessage) -> Self {
        match value.code {
            Code::ResOk => Self::Ok,
            Code::ResConfigured => Self::Ping(true),
            Code::ResAwait => Self::Ping(false),
            Code::ResInfo => {
                let mmap = value.shm.unwrap();
                let bytes = mmap.slice();
                let len = bytes[0] as usize;
                let mut bg_infos = Vec::with_capacity(len);

                let mut i = 1;
                for _ in 0..len {
                    let (info, offset) = BgInfo::deserialize(&bytes[i..]);
                    i += offset;
                    bg_infos.push(info);
                }

                Self::Info(bg_infos.into())
            }
            _ => panic!("Received malformed answer from daemon"),
        }
    }
}
// TODO: end remove ugly mess block

macro_rules! msg {
    ($($name:ident $num:literal),* $(,)?) => {
        #[derive(Copy, Clone, PartialEq, Eq)]
        enum Code {
            $($name,)*
        }

        impl Code {
            const fn into(self) -> u64 {
                match self {
                     $(Self::$name => $num,)*
                }
            }

            const fn from(num: u64) -> Option<Self> {
                 match num {
                     $($num => Some(Self::$name),)*
                     _ => None
                 }
            }
        }
    };
}

msg! {
    ReqPing       0,
    ReqQuery      1,
    ReqClear      2,
    ReqImg        3,
    ReqKill       4,

    ResOk         5,
    ResConfigured 6,
    ResAwait      7,
    ResInfo       8,
}

impl TryFrom<u64> for Code {
    type Error = IpcError;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::from(value).ok_or(IpcError::new(IpcErrorKind::BadCode, Errno::DOM))
    }
}

// TODO: this along with `RawMsg` should be implementation detail
impl<T> IpcSocket<T> {
    pub fn send(&self, msg: IpcMessage) -> io::Result<bool> {
        let mut payload = [0u8; 16];
        let mut buf = Cursor::new(payload.as_mut_slice());
        msg.code.into().serialize(&mut buf);

        let mut ancillary = [0u8; rustix::cmsg_space!(ScmRights(1))];
        let mut ancillary = net::SendAncillaryBuffer::new(&mut ancillary);

        let fd;
        if let Some(ref mmap) = msg.shm {
            debug_assert!(
                matches!(msg.code, Code::ReqClear | Code::ReqImg | Code::ResInfo),
                "`Mmap` received but not requested"
            );
            (mmap.len() as u64).serialize(&mut buf);
            fd = [mmap.fd()];
            let msg = net::SendAncillaryMessage::ScmRights(&fd);
            ancillary.push(msg);
        }

        let payload = buf.finish();
        let iov = io::IoSlice::new(payload);
        net::sendmsg(
            self.as_fd(),
            &[iov],
            &mut ancillary,
            net::SendFlags::empty(),
        )
        .map(|written| written == payload.len())
    }

    pub fn recv(&self) -> Result<IpcMessage, IpcError> {
        let mut buf = [0u8; 16];
        let mut ancillary = [0u8; rustix::cmsg_space!(ScmRights(1))];

        let mut control = net::RecvAncillaryBuffer::new(&mut ancillary);

        for _ in 0..5 {
            let iov = io::IoSliceMut::new(&mut buf);
            match net::recvmsg(self.as_fd(), &mut [iov], &mut control, RecvFlags::WAITALL) {
                Ok(_) => break,
                Err(Errno::WOULDBLOCK | Errno::INTR) => thread::sleep(Duration::from_millis(1)),
                Err(err) => return Err(err).context(IpcErrorKind::Read),
            }
        }

        let mut buf = Cursor::new(buf.as_slice());
        let code = u64::deserialize(&mut buf).try_into()?;
        let len = u64::deserialize(&mut buf) as usize;

        let shm = if len == 0 {
            debug_assert!(matches!(
                code,
                Code::ReqClear | Code::ReqImg | Code::ResInfo
            ));
            None
        } else {
            let file = control
                .drain()
                .next()
                .and_then(|msg| match msg {
                    net::RecvAncillaryMessage::ScmRights(mut iter) => iter.next(),
                    _ => None,
                })
                .ok_or(Errno::BADMSG)
                .context(IpcErrorKind::MalformedMsg)?;
            Some(Mmap::from_fd(file, len))
        };
        Ok(IpcMessage { code, shm })
    }
}

impl From<Request<'_>> for IpcMessage {
    fn from(value: Request) -> Self {
        match value {
            Request::Ping => Self {
                code: Code::ReqPing,
                shm: None,
            },
            Request::Query => Self {
                code: Code::ReqQuery,
                shm: None,
            },
            Request::Kill => Self {
                code: Code::ReqKill,
                shm: None,
            },
            Request::Clear(clear) => {
                let mut mmap = Mmap::create(clear.size());
                clear.serialize(&mut Cursor::new(mmap.slice_mut()));
                Self {
                    code: Code::ReqClear,
                    shm: Some(mmap),
                }
            }
            Request::Img(image) => {
                let mut mmap = Mmap::create(image.size());
                image.serialize(&mut Cursor::new(mmap.slice_mut()));
                Self {
                    code: Code::ReqImg,
                    shm: Some(mmap),
                }
            }
        }
    }
}

impl<'a> From<&'a IpcMessage> for Request<'a> {
    fn from(value: &'a IpcMessage) -> Self {
        match value.code {
            Code::ReqPing => Self::Ping,
            Code::ReqQuery => Self::Query,
            Code::ReqKill => Self::Kill,
            Code::ReqClear => {
                let mmap = value.shm.as_ref().expect("clear request must contain data");
                let clear = ClearRequest::deserialize(&mut Cursor::new(mmap.slice()));
                Self::Clear(clear)
            }
            Code::ReqImg => {
                let mmap = value.shm.as_ref().expect("image request must contain data");
                let image = ImageRequest::deserialize(&mut Cursor::new(mmap.slice()));
                Self::Img(image)
            }
            _ => unreachable!("`Request` builder reached invalid state"),
        }
    }
}

impl From<Response> for IpcMessage {
    fn from(value: Response) -> Self {
        match value {
            Response::Ok => Self {
                code: Code::ResOk,
                shm: None,
            },
            Response::Ping(false) => Self {
                code: Code::ResAwait,
                shm: None,
            },
            Response::Ping(true) => Self {
                code: Code::ResConfigured,
                shm: None,
            },
            Response::Info(info) => {
                let mut mmap = Mmap::create(info.size());
                info.serialize(&mut Cursor::new(mmap.slice_mut()));
                Self {
                    code: Code::ReqImg,
                    shm: Some(mmap),
                }
            }
        }
    }
}

impl From<IpcMessage> for Response {
    fn from(value: IpcMessage) -> Self {
        match value.code {
            Code::ResOk => Self::Ok,
            Code::ResAwait => Self::Ping(false),
            Code::ResConfigured => Self::Ping(true),
            Code::ResInfo => {
                let mmap = value.shm.as_ref().expect("info request must contain data");
                let info = Box::<[Info]>::deserialize(&mut Cursor::new(mmap.slice()));
                Self::Info(info)
            }
            _ => unreachable!("`Response` builder reached invalid state"),
        }
    }
}

impl IpcSocket<Client> {
    /// Send blocking request to `Daemon`, awaiting response
    pub fn request(&self, request: Request) -> Result<Response, IpcError> {
        self.send(IpcMessage::from(request))
            .context(IpcErrorKind::MalformedMsg)?;
        self.recv().map(Response::from)
    }
}

impl IpcSocket<Server> {
    /// Handle incoming request with `handler`
    pub fn handle(&self, handler: impl FnOnce(Request) -> Response) -> Result<(), IpcError> {
        let socket = match net::accept(self.as_fd()) {
            Ok(stream) => Self::new(stream),
            Err(Errno::INTR | Errno::WOULDBLOCK) => return Ok(()),
            Err(err) => return Err(err).context(IpcErrorKind::Read),
        };
        let request = socket.recv()?;
        let response = handler(Request::from(&request));
        socket
            .send(IpcMessage::from(response))
            .map(|_| ())
            .context(IpcErrorKind::Bind)
    }
}
