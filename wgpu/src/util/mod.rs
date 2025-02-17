//! Utility structures and functions that are built on top of the main `wgpu` API.
//!
//! Nothing in this module is a part of the WebGPU API specification;
//! they are unique to the `wgpu` library.

mod belt;
mod device;
mod encoder;
mod indirect;
mod init;

use std::ops::{Add, Rem, Sub};
use std::sync::Arc;
use std::{
    borrow::Cow,
    mem::{align_of, size_of},
    ptr::copy_nonoverlapping,
};

pub use belt::StagingBelt;
pub use device::{BufferInitDescriptor, DeviceExt};
pub use encoder::RenderEncoder;
pub use indirect::*;
pub use init::*;

/// Treat the given byte slice as a SPIR-V module.
///
/// # Panic
///
/// This function panics if:
///
/// - Input length isn't multiple of 4
/// - Input is longer than [`usize::max_value`]
/// - SPIR-V magic number is missing from beginning of stream
#[cfg(feature = "spirv")]
pub fn make_spirv(data: &[u8]) -> super::ShaderSource {
    super::ShaderSource::SpirV(make_spirv_raw(data))
}

/// Version of make_spirv intended for use with [`Device::create_shader_module_spirv`].
/// Returns raw slice instead of ShaderSource.
///
/// [`Device::create_shader_module_spirv`]: crate::Device::create_shader_module_spirv
pub fn make_spirv_raw(data: &[u8]) -> Cow<[u32]> {
    const MAGIC_NUMBER: u32 = 0x0723_0203;
    assert_eq!(
        data.len() % size_of::<u32>(),
        0,
        "data size is not a multiple of 4"
    );

    //If the data happens to be aligned, directly use the byte array,
    // otherwise copy the byte array in an owned vector and use that instead.
    let words = if data.as_ptr().align_offset(align_of::<u32>()) == 0 {
        let (pre, words, post) = unsafe { data.align_to::<u32>() };
        debug_assert!(pre.is_empty());
        debug_assert!(post.is_empty());
        Cow::from(words)
    } else {
        let mut words = vec![0u32; data.len() / size_of::<u32>()];
        unsafe {
            copy_nonoverlapping(data.as_ptr(), words.as_mut_ptr() as *mut u8, data.len());
        }
        Cow::from(words)
    };

    assert_eq!(
        words[0], MAGIC_NUMBER,
        "wrong magic word {:x}. Make sure you are using a binary SPIRV file.",
        words[0]
    );

    words
}

/// CPU accessible buffer used to download data back from the GPU.
pub struct DownloadBuffer(
    Arc<super::Buffer>,
    Box<dyn crate::context::BufferMappedRange>,
);

impl DownloadBuffer {
    /// Asynchronously read the contents of a buffer.
    pub fn read_buffer(
        device: &super::Device,
        queue: &super::Queue,
        buffer: &super::BufferSlice,
        callback: impl FnOnce(Result<Self, super::BufferAsyncError>) + Send + 'static,
    ) {
        let size = match buffer.size {
            Some(size) => size.into(),
            None => buffer.buffer.map_context.lock().total_size - buffer.offset,
        };

        let download = Arc::new(device.create_buffer(&super::BufferDescriptor {
            size,
            usage: super::BufferUsages::COPY_DST | super::BufferUsages::MAP_READ,
            mapped_at_creation: false,
            label: None,
        }));

        let mut encoder =
            device.create_command_encoder(&super::CommandEncoderDescriptor { label: None });
        encoder.copy_buffer_to_buffer(buffer.buffer, buffer.offset, &download, 0, size);
        let command_buffer: super::CommandBuffer = encoder.finish();
        queue.submit(Some(command_buffer));

        download
            .clone()
            .slice(..)
            .map_async(super::MapMode::Read, move |result| {
                if let Err(e) = result {
                    callback(Err(e));
                    return;
                }

                let mapped_range = super::DynContext::buffer_get_mapped_range(
                    &*download.context,
                    &download.id,
                    download.data.as_ref(),
                    0..size,
                );
                callback(Ok(Self(download, mapped_range)));
            });
    }
}

impl std::ops::Deref for DownloadBuffer {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.1.slice()
    }
}

///
/// Aligns a `value` to an `alignment`.
///
/// Returns the first number greater than or equal to `value` that is also a
/// multiple of `alignment`. If `value` is already a multiple of `alignment`,
/// `value` will be returned.
///
/// # Examples
///
/// ```
/// # use wgpu::util::align_to;
/// assert_eq!(align_to(253, 16), 256);
/// assert_eq!(align_to(256, 16), 256);
/// assert_eq!(align_to(0, 16), 0);
/// ```
///
pub fn align_to<T>(value: T, alignment: T) -> T
where
    T: Add<Output = T> + Copy + Default + PartialEq<T> + Rem<Output = T> + Sub<Output = T>,
{
    let remainder = value % alignment;
    if remainder == T::default() {
        value
    } else {
        value + alignment - remainder
    }
}
