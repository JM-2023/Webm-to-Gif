use std::ffi::CStr;
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::{ptr, mem};
use std::sync::Once;

use color_eyre::Result;
use color_eyre::eyre::{ensure, eyre, Context};
use ffmpeg_sys_next as f;

mod error;
pub use error::*;
use imgref::ImgVec;
use rgb::{RGBA8, ComponentBytes};

macro_rules! c_str {
    ($s:literal) => {
        concat!($s, "\0").as_ptr() as *const std::os::raw::c_char
    };
}

macro_rules! to_str {
    ($ptr:expr) => {
        CStr::from_ptr($ptr).to_string_lossy()
    };
}

static INIT: Once = Once::new();

pub struct WebmContext {
    ptr: *mut f::AVFormatContext,
    _marker: PhantomData<&'static f::AVFormatContext>
}

pub struct WebmStream<'ctx> {
    ctx: &'ctx mut WebmContext,
    ptr: *mut f::AVStream
}

pub struct WebmDecoder<'ctx> {
    ctx: &'ctx mut WebmContext,
    stream: *mut f::AVStream,
    dec_ctx: *mut f::AVCodecContext,
    sws_ctx: Option<NonNull<f::SwsContext>>,
    packet: *mut f::AVPacket,
    frame: *mut f::AVFrame,
    info: Option<StreamInfo>,
    _marker1: PhantomData<&'static f::AVCodecContext>,
    _marker2: PhantomData<&'static f::AVPacket>,
    _marker3: PhantomData<&'static f::AVFrame>
}

unsafe impl Send for WebmContext {}
unsafe impl<'ctx> Send for WebmStream<'ctx> {}

#[allow(unused)]
pub enum VpxCodec {
    VP8,
    VP9
}

impl WebmContext {
    pub fn new(url: &CStr) -> Result<Self> {
        INIT.call_once(|| unsafe {
            f::av_log_set_level(f::AV_LOG_WARNING);
        });

        unsafe {
            let mut fmt_ctx: *mut f::AVFormatContext = ptr::null_mut();
            cvt(f::avformat_open_input(&mut fmt_ctx, url.as_ptr(), ptr::null_mut(), ptr::null_mut()))
                    .wrap_err("failed to open input")?;
            ensure!(!fmt_ctx.is_null(), "failed to read input");

            cvt(f::avformat_find_stream_info(fmt_ctx, ptr::null_mut())).wrap_err("failed to find stream info")?;
            Ok(Self {
                ptr: fmt_ctx,
                _marker: PhantomData
            })
        }
    }

    pub fn duration(&self) -> u64 {
        unsafe { (*self.ptr).duration as u64 }
    }

    pub fn best_stream(&mut self) -> Result<WebmStream> {
        unsafe {
            let stream_index = f::av_find_best_stream(self.ptr, f::AVMediaType::AVMEDIA_TYPE_VIDEO, -1, -1, ptr::null_mut(), 0);
            if stream_index < 0 {
                Err(AVError::from(stream_index)).wrap_err("failed to find the best video stream")
            } else {
                let stream = &mut **(*self.ptr).streams.add(stream_index as _);
                Ok(WebmStream {
                    ctx: self,
                    ptr: stream
                })
            }
        }
    }
}

impl<'ctx> WebmStream<'ctx> {
    pub fn fps(&self) -> (u32, u32) {
        unsafe {
            let n = &(*self.ptr).r_frame_rate;
            (n.num as _, n.den as _)
        }
    }

    pub fn decode(&mut self, codec: VpxCodec) -> Result<WebmDecoder> {
        unsafe {
            let (codec_name, display_name) = match codec {
                VpxCodec::VP8 => (c_str!("libvpx-8"), "libvpx-vp8"),
                VpxCodec::VP9 => (c_str!("libvpx-vp9"), "libvpx-vp9"),
            };
            let codec = f::avcodec_find_decoder_by_name(codec_name);
            ensure!(!codec.is_null(), "decoder {} not found", display_name);
            WebmDecoder::new(self.ctx, self.ptr, codec)
        }
    }
}

#[derive(Clone, Copy)]
struct StreamInfo {
    width: i32,
    height: i32,
    format: f::AVPixelFormat
}

impl<'ctx> WebmDecoder<'ctx> {
    unsafe fn new(ctx: &'ctx mut WebmContext, stream: *mut f::AVStream, codec: *const f::AVCodec) -> Result<Self> {
        let dec_ctx = f::avcodec_alloc_context3(codec);
        ensure!(!dec_ctx.is_null(), "failed to allocate codec context for {}", to_str!((*codec).name));

        cvt(f::avcodec_parameters_to_context(dec_ctx, (*stream).codecpar))
                .wrap_err("failed to copy codec parameters to decoder context")?;

        cvt(f::avcodec_open2(dec_ctx, codec, ptr::null_mut()))
                .wrap_err_with(|| eyre!("failed to open codec {}", to_str!((*codec).name)))?;

        let packet = f::av_packet_alloc();
        ensure!(!packet.is_null(), "failed to allocate packet");

        let frame = f::av_frame_alloc();
        ensure!(!frame.is_null(), "failed to allocate frame");

        Ok(Self {
            ctx,
            stream,
            dec_ctx,
            sws_ctx: None,
            packet,
            frame,
            info: None,
            _marker1: PhantomData,
            _marker2: PhantomData,
            _marker3: PhantomData
        })
    }

    #[allow(unused_labels)]
    pub fn decode_frame(&mut self) -> Result<Option<(ImgVec<RGBA8>, f64)>> {
        unsafe {
            'read: loop {
                let ret = f::av_read_frame(self.ctx.ptr, self.packet);
                if ret < 0 {
                    if ret == f::AVERROR_EOF {
                        return Ok(None);
                    }
                    return Err(AVError::from(ret)).wrap_err("failed to read frame");
                }
                let _packet_unref = scopeguard::guard(self.packet, |p| f::av_packet_unref(p));

                if (*self.packet).stream_index != (*self.stream).index {
                    continue;
                }

                cvt(f::avcodec_send_packet(self.dec_ctx, self.packet)).wrap_err("failed to submit packet for decoding")?;

                'decode: loop {
                    let ret = f::avcodec_receive_frame(self.dec_ctx, self.frame);
                    if ret < 0 {
                        if ret == f::AVERROR(f::EAGAIN) || ret == f::AVERROR_EOF {
                            break;
                        }
                        return Err(AVError::from(ret)).wrap_err("failed to decode frame");
                    }
                    let _frame_unref = scopeguard::guard(self.frame, |p| f::av_frame_unref(p));
                    let frame = &*self.frame;

                    ensure!(frame.flags & f::AV_FRAME_FLAG_CORRUPT == 0, "failed to decode frame (corrupted)");
                    if frame.flags & f::AV_FRAME_FLAG_DISCARD != 0 {
                        continue;
                    }

                    ensure!(frame.pts >= 0, "negative pts");
                    let pts = frame.pts as u64;
                    let time_base = &(*self.stream).time_base;
                    let pts = (pts * time_base.num as u64) as f64 / time_base.den as f64;

                    return Ok(Some((self.convert_frame()?, pts)));
                }
            }
        }
    }

    unsafe fn convert_frame(&mut self) -> Result<ImgVec<RGBA8>> {
        let frame = &*self.frame;
        let width = frame.width;
        let height = frame.height;
        let format = mem::transmute::<_, f::AVPixelFormat>(frame.format);

        match self.info.as_ref() {
            Some(info) => {
                ensure!(info.width == width && info.height == height, "inconsistent width and height");
                ensure!(info.format == format, "inconsistent pixel format");
            },
            None => {
                self.info = Some(StreamInfo { width, height, format });
            },
        };

        let sws_ctx = match self.sws_ctx {
            Some(ctx) => ctx,
            None => {
                let ctx = f::sws_getContext(width, height, format, width, height,
                    f::AVPixelFormat::AV_PIX_FMT_RGBA, f::SWS_FAST_BILINEAR, ptr::null_mut(), ptr::null_mut(), ptr::null_mut());
                ensure!(!ctx.is_null(), "failed to create scale context for the conversion {width}x{height} {:?} to {:?}",
                    to_str!(f::av_get_pix_fmt_name(format)),
                    to_str!(f::av_get_pix_fmt_name(f::AVPixelFormat::AV_PIX_FMT_RGBA)));
                let ctx = NonNull::new_unchecked(ctx);
                self.sws_ctx = Some(ctx);
                ctx
            },
        }.as_mut();

        let mut rgba = Vec::<RGBA8>::with_capacity(width as usize * height as usize);
        let ret = f::sws_scale(
            sws_ctx,
            frame.data.as_ptr() as _,
            frame.linesize.as_ptr(),
            0,
            height,
            [rgba.as_bytes_mut().as_mut_ptr()].as_ptr(),
            [frame.width * 4].as_ptr(),
        );
        ensure!(ret > 0, "failed to convert pixel format to RGBA");
        rgba.set_len(rgba.capacity());

        #[cfg(feature = "debug_dump")]
        {
            use std::fs::{self, File};
            use image::{codecs::tga::TgaEncoder, ImageEncoder, ColorType};
            fs::remove_dir_all("dump").ok();
            fs::create_dir_all("dump")?;
            let enc = TgaEncoder::new(File::create(format!("dump/{}.tga", frame.pts))?);
            enc.write_image(rgba.as_bytes(), width as _, height as _, ColorType::Rgba8)?;
        }

        Ok(ImgVec::new(rgba, width as _, height as _))
    }
}

impl Drop for WebmContext {
    fn drop(&mut self) {
        unsafe {
            f::avformat_close_input(&mut self.ptr);
        }
    }
}

impl<'ctx> Drop for WebmDecoder<'ctx> {
    fn drop(&mut self) {
        unsafe {
            f::avcodec_free_context(&mut self.dec_ctx);
            f::av_packet_free(&mut self.packet);
            f::av_frame_free(&mut self.frame);
            if let Some(sws_ctx) = self.sws_ctx.take() {
                f::sws_freeContext(sws_ctx.as_ptr());
            }
        }
    }
}
