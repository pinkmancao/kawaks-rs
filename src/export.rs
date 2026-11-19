use std::io::{self, Seek, SeekFrom, Write};

use flate2::{write::ZlibEncoder, Compression};

use crate::{AUDIO_SAMPLE_RATE, FRAME_HEIGHT, FRAME_WIDTH};

/// Encode one 384×224 RGB888 frame in PNG format.
pub fn write_png(mut output: impl Write, rgb: &[u8]) -> io::Result<()> {
    let row_size = FRAME_WIDTH * 3;
    if rgb.len() != row_size * FRAME_HEIGHT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "incorrect RGB frame size",
        ));
    }
    let mut header = [0, 0, 0, 0, 0, 0, 0, 0, 8, 2, 0, 0, 0];
    header[..4].copy_from_slice(&(FRAME_WIDTH as u32).to_be_bytes());
    header[4..8].copy_from_slice(&(FRAME_HEIGHT as u32).to_be_bytes());

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    for row in rgb.chunks_exact(row_size) {
        encoder.write_all(&[0])?; // PNG filter type None
        encoder.write_all(row)?;
    }
    let pixels = encoder.finish()?;
    output.write_all(b"\x89PNG\r\n\x1a\n")?;
    png_chunk(&mut output, b"IHDR", &header)?;
    png_chunk(&mut output, b"IDAT", &pixels)?;
    png_chunk(&mut output, b"IEND", &[])?;
    output.flush()
}

fn png_chunk(output: &mut impl Write, kind: &[u8; 4], contents: &[u8]) -> io::Result<()> {
    let mut crc = crc32fast::Hasher::new();
    crc.update(kind);
    crc.update(contents);
    output.write_all(&(contents.len() as u32).to_be_bytes())?;
    output.write_all(kind)?;
    output.write_all(contents)?;
    output.write_all(&crc.finalize().to_be_bytes())
}

/// Stream interleaved stereo PCM to a standard little-endian WAV file.
/// Call `finish` to complete the RIFF and data sizes.
pub struct WavWriter<W> {
    output: W,
    start: u64,
    data_size: u32,
}

impl<W: Write + Seek> WavWriter<W> {
    pub fn new(mut output: W) -> io::Result<Self> {
        let start = output.stream_position()?;
        const BYTES_PER_FRAME: u32 = 4;
        let mut header = *b"RIFF\x00\x00\x00\x00WAVEfmt \x10\x00\x00\x00\x01\x00\x02\x00\x00\x00\x00\x00\x00\x00\x00\x00\x04\x00\x10\x00data\x00\x00\x00\x00";
        header[24..28].copy_from_slice(&AUDIO_SAMPLE_RATE.to_le_bytes());
        header[28..32].copy_from_slice(&(AUDIO_SAMPLE_RATE * BYTES_PER_FRAME).to_le_bytes());
        output.write_all(&header)?;
        Ok(Self {
            output,
            start,
            data_size: 0,
        })
    }

    pub fn write_samples(&mut self, pcm: &[[i16; 2]]) -> io::Result<()> {
        let next_size = u32::try_from(pcm.len())
            .ok()
            .and_then(|len| len.checked_mul(4))
            .and_then(|bytes| self.data_size.checked_add(bytes))
            .filter(|size| *size <= u32::MAX - 36)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "PCM exceeds WAV size limit")
            })?;
        let mut buffer = [0; 4096];
        for batch in pcm.chunks(buffer.len() / 4) {
            for (channels, bytes) in batch.iter().zip(buffer.chunks_exact_mut(4)) {
                bytes[..2].copy_from_slice(&channels[0].to_le_bytes());
                bytes[2..].copy_from_slice(&channels[1].to_le_bytes());
            }
            self.output.write_all(&buffer[..batch.len() * 4])?;
        }
        self.data_size = next_size;
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<W> {
        let end = self.output.stream_position()?;
        self.output.seek(SeekFrom::Start(self.start + 4))?;
        self.output
            .write_all(&(self.data_size + 36).to_le_bytes())?;
        self.output.seek(SeekFrom::Start(self.start + 40))?;
        self.output.write_all(&self.data_size.to_le_bytes())?;
        self.output.seek(SeekFrom::Start(end))?;
        self.output.flush()?;
        Ok(self.output)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read};

    use flate2::read::ZlibDecoder;

    use super::*;

    #[test]
    fn png_round_trips_rgb_rows() {
        let rgb: Vec<u8> = (0..FRAME_WIDTH * FRAME_HEIGHT * 3)
            .map(|i| i as u8)
            .collect();
        let mut png = Vec::new();
        write_png(&mut png, &rgb).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&png[16..24], &[0, 0, 1, 128, 0, 0, 0, 224]);
        let length = u32::from_be_bytes(png[33..37].try_into().unwrap()) as usize;
        assert_eq!(&png[37..41], b"IDAT");
        let mut rows = Vec::new();
        ZlibDecoder::new(&png[41..41 + length])
            .read_to_end(&mut rows)
            .unwrap();
        for (row, expected) in rows
            .chunks_exact(1 + FRAME_WIDTH * 3)
            .zip(rgb.chunks_exact(FRAME_WIDTH * 3))
        {
            assert_eq!(row[0], 0);
            assert_eq!(&row[1..], expected);
        }
        assert_eq!(rows.len(), (1 + FRAME_WIDTH * 3) * FRAME_HEIGHT);
    }

    #[test]
    fn wav_encodes_signed_stereo_and_updates_lengths() {
        let mut wav = WavWriter::new(Cursor::new(Vec::new())).unwrap();
        wav.write_samples(&[[-1, i16::MIN]]).unwrap();
        wav.write_samples(&[[1, i16::MAX]]).unwrap();
        let bytes = wav.finish().unwrap().into_inner();
        assert_eq!(&bytes[..12], b"RIFF,\x00\x00\x00WAVE");
        assert_eq!(&bytes[24..28], &AUDIO_SAMPLE_RATE.to_le_bytes());
        assert_eq!(
            &bytes[36..],
            b"data\x08\x00\x00\x00\xff\xff\x00\x80\x01\x00\xff\x7f"
        );
    }
}
