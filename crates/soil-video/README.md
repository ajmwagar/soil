# soil-video

The host side of video: an adapter that can decode, a device with video support,
and frames that arrive as `wgpu` textures.

It is thin on purpose. Hardware decoding, the zero-copy path (VideoToolbox and
IOSurface on Apple, Vulkan Video elsewhere) and the `VideoTexture` type all belong
to [`gpu-video` 0.5](https://crates.io/crates/gpu-video) — this crate asks for the
right device and gives it to a caller that has a surface.

```rust
let mut host = soil_video::Host::open()?;
println!("decoding with {}", host.describe());
let mut decoder = host.decode_h264(|frame| {
    // frame.data is a VideoTexture: wgpu textures holding NV12 planes
})?;
```

## Why a device of its own

A decoder is not a texture upload. Hardware decode needs features — and on Vulkan
queue families — that a render device did not ask for, and asking for them there
makes every other pipeline in the process pay for a capability only this path
uses. Textures from the video device stay on it; a frame crosses to the renderer
as something it can sample, or not at all.

## Not wired yet

The surface. Frames come out as texture views ready for a bind group; putting them
into `soil:present` belongs with whoever owns that contract.
