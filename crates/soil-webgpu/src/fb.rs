//! `/dev/fbN` presenter for the CAT S22 panels.
//!
//! The outer display is a plain 16-bpp framebuffer (`panel-mipi-dbi-spi`,
//! 128x128, stride 256 bytes). The inner one is 24-bpp at 480x640, stride
//! 1440. Neither has a window system or a DRM modeset behind it, and the guest
//! has already rendered the frame, so it is converted to the panel's pixel
//! format and written straight into the framebuffer.
//!
//! This is a device presenter, not a compositor. It owns exactly one panel and
//! presents one frame at a time; a host that needs z-order or hit testing uses
//! `canvas-winit` instead.

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::{CanvasFrame, CanvasGpuPresenter};

/// How the panel wants its pixels.
///
/// The depth is a property of the panel, not of this presenter, so it is read
/// from sysfs at open time and carried in the presenter from then on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    /// 16 bpp, 5-6-5.
    Rgb565,
    /// 24 bpp, three bytes per pixel, blue first — the order the S22 inner
    /// panel's framebuffer expects.
    Bgr888,
    /// 32 bpp with an unused high byte.
    Bgrx8888,
}

impl Format {
    fn from_bits(bits: u32) -> Option<Self> {
        match bits {
            16 => Some(Self::Rgb565),
            24 => Some(Self::Bgr888),
            32 => Some(Self::Bgrx8888),
            _ => None,
        }
    }

    fn bytes(self) -> usize {
        match self {
            Self::Rgb565 => 2,
            Self::Bgr888 => 3,
            Self::Bgrx8888 => 4,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Rgb565 => "RGB565",
            Self::Bgr888 => "BGR888",
            Self::Bgrx8888 => "BGRX8888",
        }
    }
}

pub struct FramebufferPresenter {
    file: File,
    path: PathBuf,
    width: u32,
    height: u32,
    stride: usize,
    format: Format,
}

impl FramebufferPresenter {
    /// Open a framebuffer device. `width`/`height` are the logical surface the
    /// Spore lays out against, which is the panel extent in device pixels.
    ///
    /// Geometry comes from sysfs rather than a flag: the driver is the only
    /// authority on the stride, and a wrong stride writes skewed rows instead
    /// of failing.
    pub fn open(path: impl AsRef<Path>, width: u32, height: u32) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("open {}", path.display()))?;
        let sysfs = Path::new("/sys/class/graphics")
            .join(path.file_name().unwrap_or_else(|| "fb0".as_ref()));
        let bits = read_u32(&sysfs.join("bits_per_pixel")).unwrap_or(16);
        let format = Format::from_bits(bits).with_context(|| {
            format!(
                "{}: {bits}-bpp framebuffer has no presenter (16, 24 and 32 bpp are supported)",
                path.display()
            )
        })?;
        let stride = read_u32(&sysfs.join("stride"))
            .map(|value| value as usize)
            .unwrap_or(width as usize * format.bytes());
        let least = width as usize * format.bytes();
        if stride < least {
            bail!(
                "{}: stride {stride} is narrower than {width} {} pixels ({least} bytes)",
                path.display(),
                format.name()
            );
        }
        Ok(Self {
            file,
            path,
            width,
            height,
            stride,
            format,
        })
    }
}

fn read_u32(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

impl CanvasGpuPresenter for FramebufferPresenter {
    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn present(&mut self, frame: CanvasFrame) -> Result<()> {
        anyhow::ensure!(
            frame.rgba.len() == (frame.width * frame.height * 4) as usize,
            "frame is not tightly packed RGBA8"
        );
        let mut buffer = vec![0u8; self.stride * self.height as usize];
        for y in 0..self.height {
            // Nearest neighbour, so a frame rendered at another extent still
            // fills the panel instead of leaving the rest of it stale.
            let source_y = (y as u64 * frame.height as u64 / self.height as u64) as u32;
            let row = y as usize * self.stride;
            for x in 0..self.width {
                let source_x = (x as u64 * frame.width as u64 / self.width as u64) as u32;
                let at = ((source_y * frame.width + source_x) * 4) as usize;
                let (r, g, b) = (
                    frame.rgba[at],
                    frame.rgba[at + 1],
                    frame.rgba[at + 2],
                );
                let at = row + x as usize * self.format.bytes();
                match self.format {
                    Format::Rgb565 => {
                        let pixel =
                            ((r as u16 >> 3) << 11) | ((g as u16 >> 2) << 5) | (b as u16 >> 3);
                        buffer[at] = (pixel & 0xff) as u8;
                        buffer[at + 1] = (pixel >> 8) as u8;
                    }
                    // Blue first, then green, then red: the byte order the
                    // panel's framebuffer takes on this SoC.
                    Format::Bgr888 => {
                        buffer[at] = b;
                        buffer[at + 1] = g;
                        buffer[at + 2] = r;
                    }
                    Format::Bgrx8888 => {
                        buffer[at] = b;
                        buffer[at + 1] = g;
                        buffer[at + 2] = r;
                        buffer[at + 3] = 0;
                    }
                }
            }
        }
        self.file
            .seek(SeekFrom::Start(0))
            .with_context(|| format!("seek {}", self.path.display()))?;
        self.file
            .write_all(&buffer)
            .with_context(|| format!("write {}", self.path.display()))?;
        self.file.flush().ok();
        Ok(())
    }
}
