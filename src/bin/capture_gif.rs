#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use gif::{Encoder, Frame, Repeat};

fn encode(directory: &Path, output: &Path, delay: u16) -> Result<(usize, u64)> {
    let mut files: Vec<PathBuf> = fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<_>>()?;
    files.retain(|filename| {
        filename
            .extension()
            .is_some_and(|extension| extension == "png")
    });
    files.sort();
    ensure!(!files.is_empty(), "No PNG frames found");
    let first = image::ImageReader::open(&files[0])?.decode()?.to_rgba8();
    let (width, height) = first.dimensions();
    ensure!(
        width <= 4096 && height <= 4096,
        "Frames must be at most 4096 pixels per dimension"
    );
    let destination = fs::File::create(output).context("Create GIF output")?;
    let mut encoder = Encoder::new(destination, width.try_into()?, height.try_into()?, &[])?;
    encoder.set_repeat(Repeat::Infinite)?;
    for filename in &files {
        let image = image::ImageReader::open(filename)?.decode()?.to_rgba8();
        ensure!(
            image.dimensions() == (width, height),
            "All frames must have the same dimensions"
        );
        let mut pixels = image.into_raw();
        let mut frame =
            Frame::from_rgba_speed(width.try_into()?, height.try_into()?, &mut pixels, 10);
        frame.delay = delay;
        encoder.write_frame(&frame)?;
    }
    drop(encoder);
    Ok((files.len(), fs::metadata(output)?.len()))
}

fn main() -> Result<()> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if !(2..=3).contains(&arguments.len()) {
        bail!("Usage: capture_gif FRAME_DIRECTORY OUTPUT.gif [DELAY_CENTISECONDS]");
    }
    let delay = arguments
        .get(2)
        .map_or(Ok(85), |argument| argument.parse::<u16>())?;
    ensure!(delay > 0, "Frame delay must be positive");
    let (frames, bytes) = encode(Path::new(&arguments[0]), Path::new(&arguments[1]), delay)?;
    println!(
        "Encoded {frames} browser frames into {bytes} bytes: {}",
        arguments[1]
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gif_encoder_creates_a_looping_animation_from_real_png_frames() {
        let directory = tempfile::tempdir().unwrap();
        for (index, color) in [[32, 42, 45, 255], [255, 206, 84, 255]]
            .into_iter()
            .enumerate()
        {
            image::RgbaImage::from_pixel(16, 16, image::Rgba(color))
                .save(directory.path().join(format!("{index:02}.png")))
                .unwrap();
        }
        let output = directory.path().join("demo.gif");
        let (frames, bytes) = encode(directory.path(), &output, 85).unwrap();
        assert_eq!(frames, 2);
        assert!(bytes > 0);
        let mut decoder = gif::DecodeOptions::new()
            .read_info(fs::File::open(output).unwrap())
            .unwrap();
        let mut count = 0;
        while let Some(frame) = decoder.read_next_frame().unwrap() {
            assert_eq!(frame.delay, 85);
            count += 1;
        }
        assert_eq!(count, 2);
    }
}
