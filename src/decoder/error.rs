use core::fmt;
use std::os::raw::{c_int, c_char};
use std::ffi::CStr;

use ffmpeg_sys_next as f;

#[inline]
pub fn cvt(ret: c_int) -> Result<(), AVError> {
    if ret <= 0 {
        Ok(())
    } else {
        Err(AVError::from(ret))
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum AVError {
    Bug,
    Bug2,
    Unknown,
    Experimental,
    BufferTooSmall,
    Eof,
    Exit,
    External,
    InvalidData,
    PatchWelcome,

    InputChanged,
    OutputChanged,

    BsfNotFound,
    DecoderNotFound,
    DemuxerNotFound,
    EncoderNotFound,
    OptionNotFound,
    MuxerNotFound,
    FilterNotFound,
    ProtocolNotFound,
    StreamNotFound,

    HttpBadRequest,
    HttpUnauthorized,
    HttpForbidden,
    HttpNotFound,
    HttpOther4xx,
    HttpServerError,

    /// For AVERROR(e) wrapping POSIX error codes, e.g. AVERROR(EAGAIN).
    Other(c_int),
}

macro_rules! impl_errors {
    ( $($name:ident => $tag:ident),* $(,)? ) => {
        impl From<c_int> for AVError {
            #[inline]
            fn from(val: c_int) -> Self {
                match val {
                    $(f::$tag => Self::$name),*,
                    e => Self::Other(f::AVUNERROR(e)),
                }
            }
        }

        impl From<AVError> for c_int {
            #[inline]
            fn from(val: AVError) -> Self {
                match val {
                    $(AVError::$name => f::$tag),*,
                    AVError::Other(errno) => f::AVERROR(errno),
                }
            }
        }

        impl AVError {
            #[inline]
            fn get_str(&self) -> &'static str {
                use std::lazy::SyncOnceCell;

                #[cold]
                fn init(errnum: c_int, cell: &SyncOnceCell<&'static str>, buf: &mut [c_char; f::AV_ERROR_MAX_STRING_SIZE]) -> &'static str {
                    cell.get_or_init(|| unsafe {
                        f::av_strerror(errnum, buf.as_mut_ptr(), buf.len());
                        std::str::from_utf8_unchecked(CStr::from_ptr(buf.as_ptr()).to_bytes())
                    })
                }

                $(
                    #[allow(non_upper_case_globals)]
                    static $name: SyncOnceCell<&'static str> = SyncOnceCell::new()
                );*;

                match *self {
                    $(
                        Self::$name => {
                            match $name.get() {
                                Some(s) => *s,
                                None => {
                                    static mut BUF: [c_char; f::AV_ERROR_MAX_STRING_SIZE] = [0; f::AV_ERROR_MAX_STRING_SIZE];
                                    init(f::$tag, &$name, unsafe { &mut BUF })
                                }
                            }
                        }
                    ),*,
                    Self::Other(errno) => unsafe {
                        std::str::from_utf8_unchecked(CStr::from_ptr(libc::strerror(errno)).to_bytes())
                    }
                }
            }
        }
    };
}

impl_errors! {
    BsfNotFound => AVERROR_BSF_NOT_FOUND,
    Bug => AVERROR_BUG,
    BufferTooSmall => AVERROR_BUFFER_TOO_SMALL,
    DecoderNotFound => AVERROR_DECODER_NOT_FOUND,
    DemuxerNotFound => AVERROR_DEMUXER_NOT_FOUND,
    EncoderNotFound => AVERROR_ENCODER_NOT_FOUND,
    Eof => AVERROR_EOF,
    Exit => AVERROR_EXIT,
    External => AVERROR_EXTERNAL,
    FilterNotFound => AVERROR_FILTER_NOT_FOUND,
    InvalidData => AVERROR_INVALIDDATA,
    MuxerNotFound => AVERROR_MUXER_NOT_FOUND,
    OptionNotFound => AVERROR_OPTION_NOT_FOUND,
    PatchWelcome => AVERROR_PATCHWELCOME,
    ProtocolNotFound => AVERROR_PROTOCOL_NOT_FOUND,
    StreamNotFound => AVERROR_STREAM_NOT_FOUND,
    Bug2 => AVERROR_BUG2,
    Unknown => AVERROR_UNKNOWN,
    Experimental => AVERROR_EXPERIMENTAL,
    InputChanged => AVERROR_INPUT_CHANGED,
    OutputChanged => AVERROR_OUTPUT_CHANGED,
    HttpBadRequest => AVERROR_HTTP_BAD_REQUEST,
    HttpUnauthorized => AVERROR_HTTP_UNAUTHORIZED,
    HttpForbidden => AVERROR_HTTP_FORBIDDEN,
    HttpNotFound => AVERROR_HTTP_NOT_FOUND,
    HttpOther4xx => AVERROR_HTTP_OTHER_4XX,
    HttpServerError => AVERROR_HTTP_SERVER_ERROR,
}

impl fmt::Display for AVError {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.get_str())
    }
}

impl fmt::Debug for AVError {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("AVError({}: {})", f::AVUNERROR((*self).into()), self.get_str()))
    }
}

impl std::error::Error for AVError {}
