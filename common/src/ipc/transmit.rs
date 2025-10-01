use std::mem::MaybeUninit;
use std::thread;
use std::time::Duration;

use rustix::io;
use rustix::io::Errno;
use rustix::net;
use rustix::net::RecvFlags;

use super::Animation;
use super::Answer;
use super::BgInfo;
use super::ClearReq;
use super::ErrnoExt;
use super::ImageReq;
use super::ImgReq;
use super::IpcError;
use super::IpcErrorKind;
use super::IpcSocket;
use super::RequestRecv;
use super::RequestSend;
use super::Transition;
use crate::mmap::Mmap;
use crate::mmap::MmappedStr;

// could be enum
pub struct RawMsg {
    code: Code,
    shm: Option<Mmap>,
}

impl From<RequestSend> for RawMsg {
    fn from(value: RequestSend) -> Self {
        let code = match value {
            RequestSend::Ping => Code::ReqPing,
            RequestSend::Query => Code::ReqQuery,
            RequestSend::Clear(_) => Code::ReqClear,
            RequestSend::Img(_) => Code::ReqImg,
            RequestSend::Pause => Code::ReqPause,
            RequestSend::Kill => Code::ReqKill,
        };

        let shm = match value {
            RequestSend::Clear(mem) | RequestSend::Img(mem) => Some(mem),
            _ => None,
        };

        Self { code, shm }
    }
}

impl From<Answer> for RawMsg {
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
impl From<RawMsg> for RequestRecv {
    fn from(value: RawMsg) -> Self {
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
                let color = [bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]];
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
                    imgs,
                    outputs,
                    animations: if animations.is_empty() {
                        None
                    } else {
                        Some(animations)
                    },
                })
            }
            Code::ReqPause => Self::Pause,
            Code::ReqKill => Self::Kill,
            _ => Self::Kill,
        }
    }
}

impl From<RawMsg> for Answer {
    fn from(value: RawMsg) -> Self {
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

macro_rules! code {
    ($($name:ident $num:literal),* $(,)?) => {
        #[derive(Debug)]
        pub enum Code {
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

code! {
    ReqPing       0,
    ReqQuery      1,
    ReqClear      2,
    ReqImg        3,
    ReqKill       4,

    ResOk         5,
    ResConfigured 6,
    ResAwait      7,
    ResInfo       8,

    ReqPause      9,
}

impl TryFrom<u64> for Code {
    type Error = IpcError;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::from(value).ok_or(IpcError::new(IpcErrorKind::BadCode, Errno::DOM))
    }
}

// TODO: this along with `RawMsg` should be implementation detail
impl<T> IpcSocket<T> {
    pub fn send(&self, msg: RawMsg) -> io::Result<bool> {
        let mut payload = [0u8; 16];
        payload[0..8].copy_from_slice(&msg.code.into().to_ne_bytes());

        let mut ancillary_buf = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
        let mut ancillary = net::SendAncillaryBuffer::new(&mut ancillary_buf);

        let fd;
        if let Some(ref mmap) = msg.shm {
            payload[8..].copy_from_slice(&(mmap.len() as u64).to_ne_bytes());
            fd = [mmap.fd()];
            let msg = net::SendAncillaryMessage::ScmRights(&fd);
            ancillary.push(msg);
        }

        let iov = io::IoSlice::new(&payload[..]);
        net::sendmsg(
            self.as_fd(),
            &[iov],
            &mut ancillary,
            net::SendFlags::empty(),
        )
        .map(|written| written == payload.len())
    }

    pub fn recv(&self) -> Result<RawMsg, IpcError> {
        let mut buf = [0u8; 16];
        let mut ancillary_buf = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];

        let mut control = net::RecvAncillaryBuffer::new(&mut ancillary_buf);

        for _ in 0..5 {
            let iov = io::IoSliceMut::new(&mut buf);
            match net::recvmsg(self.as_fd(), &mut [iov], &mut control, RecvFlags::WAITALL) {
                Ok(_) => break,
                Err(Errno::WOULDBLOCK | Errno::INTR) => thread::sleep(Duration::from_millis(1)),
                Err(err) => return Err(err).context(IpcErrorKind::Read),
            }
        }

        let code = u64::from_ne_bytes(buf[0..8].try_into().unwrap()).try_into()?;
        let len = u64::from_ne_bytes(buf[8..16].try_into().unwrap()) as usize;

        let shm = if len == 0 {
            debug_assert!(
                !matches!(code, Code::ReqImg | Code::ReqClear | Code::ResInfo),
                "Received: Code {:?}, which should have sent a shm fd",
                code
            );
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
        Ok(RawMsg { code, shm })
    }
}
