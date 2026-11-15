//! Video, decoded to GPU textures, on the crates that already do it.
//!
//! `gpu-video` owns hardware decoding and the zero-copy path into `wgpu`
//! (VideoToolbox and IOSurface on Apple, Vulkan Video elsewhere); a frame
//! arrives as a `VideoTexture` wrapping the `wgpu::Texture`s that hold its NV12
//! planes, and never round-trips through system memory. This crate is the host
//! side of that: it asks for an adapter that can *decode*, opens a device with
//! the video features those decoders need, and hands out frames a presenter can
//! sample.
//!
//! ```no_run
//! # fn main() -> anyhow::Result<()> {
//! let mut host = soil_video::Host::open()?;
//! println!("decoding with {}", host.describe());
//! # Ok(())
//! # }
//! ```
//!
//! # Why a device of its own
//!
//! A decoder is not a texture upload. Hardware decode needs features and, on
//! Vulkan, queue families a plain render device did not ask for; requesting them
//! on the render device would make every other pipeline in the process pay for a
//! capability only this path uses. So the host asks the adapter for a second
//! device with video support, and textures from it stay on it: a frame crosses
//! back to the renderer as a texture it can sample, or not at all.
//!
//! # What is not wired yet
//!
//! The surface. Frames come out as `wgpu::TextureView`s ready for a bind group;
//! putting them into `soil:present` is the next step and belongs with whoever
//! owns that contract, not here.

use anyhow::{Context, Result, bail};
use gpu_video::{VideoAdapterExt, VideoDeviceExt, parameters::VideoDeviceDescriptor};

/// One adapter that can decode, with the device it gave us.
pub struct Host {
    adapter_info: wgpu::AdapterInfo,
    device: wgpu::Device,
    queue: wgpu::Queue,
    video: gpu_video::VideoDevice,
}

impl Host {
    /// Find an adapter that decodes H.264 and open a video-capable device on it.
    ///
    /// H.264 is the floor because every hardware decoder has it; a build that
    /// wants HEVC or AV1 asks for the capability it needs rather than assuming
    /// the same adapter answers.
    pub fn open() -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        // Enumerating is async in wgpu 30: an adapter may have to be woken.
        let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
        let adapter = adapters
            .into_iter()
            .find(|adapter| {
                adapter
                    .video_adapter_info()
                    .is_some_and(|info| info.decode_capabilities.h264.is_some())
            })
            .context(
                "no adapter here decodes H.264 in hardware; a host with no video device \
                 is a host that cannot play video, and pretending otherwise would fail \
                 at the first frame",
            )?;
        let adapter_info = adapter.get_info();
        let (device, queue) = adapter
            .request_device_with_video_support(&VideoDeviceDescriptor::default())
            .map_err(|error| anyhow::anyhow!("request a video device on {adapter_info:?}: {error:#}"))?;
        let video = device
            .video()
            .context("the adapter accepted video support but produced no video device")?;
        Ok(Self {
            adapter_info,
            device,
            queue,
            video,
        })
    }

    /// What this host is decoding on, for a log line or an operator question.
    pub fn describe(&self) -> String {
        format!(
            "{} ({:?}, {:?})",
            self.adapter_info.name, self.adapter_info.device_type, self.adapter_info.backend
        )
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// The video device, for a caller that wants to open its own decoder on it.
    pub fn video(&self) -> &gpu_video::VideoDevice {
        &self.video
    }

    /// Decode H.264 into `wgpu` textures, one callback per frame.
    ///
    /// The callback is where a presenter takes the frame: keeping it light is
    /// the point, since the decoder is a GPU pipeline and a callback that blocks
    /// stalls the queue behind it.
    pub fn decode_h264<F>(&self, on_frame: F) -> Result<gpu_video::WgpuTexturesDecoderH264>
    where
        F: FnMut(gpu_video::OutputFrame<gpu_video::VideoTexture>) + Send + 'static,
    {
        self.video
            .create_wgpu_textures_decoder_h264(
                &self.queue,
                gpu_video::parameters::DecoderParameters::default(),
                on_frame,
            )
            .map_err(|error| anyhow::anyhow!("open the H.264 decoder: {error:#}"))
    }
}

/// Whether this machine can decode video at all, without opening a device.
///
/// A caller that has to choose between showing a player and showing a poster
/// frame should be able to ask cheaply; opening a device to find out is the
/// expensive way to learn the same thing.
pub fn available() -> bool {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()))
        .iter()
        .any(|adapter| {
            adapter
                .video_adapter_info()
                .is_some_and(|info| info.decode_capabilities.h264.is_some())
        })
}

/// Refuse early with a reason instead of failing at the first frame.
pub fn require() -> Result<()> {
    if available() {
        return Ok(());
    }
    bail!("this machine has no hardware H.264 decoder")
}
