pub use self::types::*;
use byteorder::{BigEndian, ByteOrder, WriteBytesExt};
use std::io::{Cursor, Error, ErrorKind, Result, Write};

mod types;

const MAX_CHUNK_LEN: usize = (1usize << 31) - 1;

pub struct Encoder<W: Write> {
    writer: W,
    width: u32,
    height: u32,
    sequence_number: u32,
}

impl<W: Write> Encoder<W> {
    pub fn new(mut writer: W, width: u32, height: u32) -> Result<Encoder<W>> {
        writer.write_all(b"\x89PNG\r\n\x1A\n")?;

        let mut encoder = Encoder {
            writer,
            width,
            height,
            sequence_number: 0,
        };

        {
            let mut data = [0u8; 13];
            {
                let mut writer = Cursor::new(&mut data[..]);
                writer.write_u32::<BigEndian>(width)?;
                writer.write_u32::<BigEndian>(height)?;
                writer.write_u8(8)?; // Bit depth
                writer.write_u8(ColorType::RGB as u8)?;
                writer.write_u8(CompressionMethod::Deflate as u8)?;
                writer.write_u8(FilterMethod::Adaptive as u8)?;
                writer.write_u8(InterlaceMethod::None as u8)?;
            }

            encoder.write_chunk(b"IHDR", &data)?;
        }

        // Set up the sRGB color space. The gAMA and cHRM chunks are for compatibility.
        encoder.write_chunk(b"sRGB", &[SrgbIntent::RelativeColorimetric as u8])?;
        {
            let mut data = [0u8; 4];
            BigEndian::write_u32(&mut data, 45455);
            encoder.write_chunk(b"gAMA", &data)?;
        }
        {
            let mut data = [0u8; 32];
            {
                let mut writer = Cursor::new(&mut data[..]);
                // White point X and Y
                writer.write_u32::<BigEndian>(31270)?;
                writer.write_u32::<BigEndian>(32900)?;
                // Red X and Y
                writer.write_u32::<BigEndian>(64000)?;
                writer.write_u32::<BigEndian>(33000)?;
                // Green X and Y
                writer.write_u32::<BigEndian>(30000)?;
                writer.write_u32::<BigEndian>(60000)?;
                // Blue X and Y
                writer.write_u32::<BigEndian>(15000)?;
                writer.write_u32::<BigEndian>(6000)?;
            }
            encoder.write_chunk(b"cHRM", &data)?;
        }

        Ok(encoder)
    }

    pub fn enable_animation(&mut self, num_frames: u32, num_plays: u32) -> Result<()> {
        let mut data = [0u8; 8];
        {
            let mut writer = Cursor::new(&mut data[..]);
            writer.write_u32::<BigEndian>(num_frames)?;
            writer.write_u32::<BigEndian>(num_plays)?;
        }
        self.write_chunk(b"acTL", &data)?;
        Ok(())
    }

    pub fn write_frame_control(&mut self, frame_control: FrameControl) -> Result<()> {
        let mut data = [0u8; 26];
        {
            let mut writer = Cursor::new(&mut data[..]);
            writer.write_u32::<BigEndian>(self.sequence_number)?;
            self.sequence_number += 1;
            writer.write_u32::<BigEndian>(frame_control.width)?;
            writer.write_u32::<BigEndian>(frame_control.height)?;
            writer.write_u32::<BigEndian>(frame_control.x_offset)?;
            writer.write_u32::<BigEndian>(frame_control.y_offset)?;
            writer.write_u16::<BigEndian>(frame_control.delay_num)?;
            writer.write_u16::<BigEndian>(frame_control.delay_den)?;
            writer.write_u8(frame_control.dispose_op as u8)?;
            writer.write_u8(frame_control.blend_op as u8)?;
        }
        self.write_chunk(b"fcTL", &data)?;
        Ok(())
    }

    fn filter_image_data(data: &[u8], width: u32, height: u32) -> Vec<u8> {
        let width = width as usize;
        let height = height as usize;

        let mut ret = Vec::with_capacity(data.len() + height);

        for i in 0..height {
            ret.push(FilterType::None as u8);
            ret.extend(&data[3 * width * i..3 * width * (i + 1)]);
        }

        ret
    }

    pub fn write_image(&mut self, data: &[u8], frame_control: Option<FrameControl>) -> Result<()> {
        if let Some(fctl) = frame_control {
            self.write_frame_control(fctl)?;
        }

        self.write_chunk(
            b"IDAT",
            &deflate::deflate_bytes_zlib_conf(
                &Self::filter_image_data(data, self.width, self.height),
                deflate::Compression::Best,
            ),
        )?;

        Ok(())
    }

    pub fn write_frame(&mut self, data: &[u8], frame_control: FrameControl) -> Result<()> {
        self.write_frame_control(frame_control)?;

        let mut data = deflate::deflate_bytes_zlib_conf(
            &Self::filter_image_data(data, frame_control.width, frame_control.height),
            deflate::Compression::Best,
        );
        let mut sequence_number = [0u8; 4];
        BigEndian::write_u32(&mut sequence_number, self.sequence_number);
        self.sequence_number += 1;
        for (i, &b) in sequence_number.iter().enumerate() {
            data.insert(i, b);
        }

        self.write_chunk(b"fdAT", &data)?;

        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        self.write_chunk(b"IEND", &[])?;

        Ok(())
    }

    fn chunk_crc(name: &[u8; 4], data: &[u8]) -> u32 {
        crc::crc32::update(
            crc::crc32::update(0, &crc::crc32::IEEE_TABLE, name),
            &crc::crc32::IEEE_TABLE,
            data,
        )
    }

    pub fn write_chunk(&mut self, name: &[u8; 4], data: &[u8]) -> Result<()> {
        if data.len() > MAX_CHUNK_LEN {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "chunk size exeeds max chunk size of 2^31 - 1",
            ));
        }

        self.writer.write_u32::<BigEndian>(data.len() as u32)?;
        self.writer.write_all(name)?;
        self.writer.write_all(data)?;
        self.writer
            .write_u32::<BigEndian>(Self::chunk_crc(name, data))?;

        Ok(())
    }
}

#[derive(Copy, Clone, Debug)]
pub struct FrameControl {
    pub width: u32,
    pub height: u32,
    pub x_offset: u32,
    pub y_offset: u32,
    pub delay_num: u16,
    pub delay_den: u16,
    pub dispose_op: DisposeOp,
    pub blend_op: BlendOp,
}

#[test]
fn smoke_test() {
    let mut data = vec![];
    let mut encoder = Encoder::new(Cursor::new(&mut data), 8, 8).unwrap();
    encoder.enable_animation(2, 0).unwrap();
    encoder
        .write_image(
            &std::iter::repeat(())
                .take(64)
                .flat_map(|()| vec![0xFF, 0x00, 0x00])
                .collect::<Vec<u8>>(),
            Some(FrameControl {
                width: 8,
                height: 8,
                x_offset: 0,
                y_offset: 0,
                delay_num: 1,
                delay_den: 1,
                dispose_op: DisposeOp::None,
                blend_op: BlendOp::Source,
            }),
        )
        .unwrap();
    encoder
        .write_frame(
            &std::iter::repeat(())
                .take(64)
                .flat_map(|()| vec![0x00, 0x00, 0xFF])
                .collect::<Vec<u8>>(),
            FrameControl {
                width: 8,
                height: 8,
                x_offset: 0,
                y_offset: 0,
                delay_num: 1,
                delay_den: 1,
                dispose_op: DisposeOp::None,
                blend_op: BlendOp::Source,
            },
        )
        .unwrap();
    encoder.finish().unwrap();

    assert_eq!(
        data,
        &[
            // PNG file signature
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A,
            // IHDR chunk
            //   length: 13 bytes
            0x00, 0x00, 0x00, 0x0D, //   chunk type: IHDR
            0x49, 0x48, 0x44, 0x52, //   data:
            //     width: 8px
            0x00, 0x00, 0x00, 0x08, //     height: 8px
            0x00, 0x00, 0x00, 0x08, //     8 bits per sample
            0x08, //     RGB
            0x02, //     DEFLATE compression
            0x00, //     adaptive filtering
            0x00, //     no interlacing
            0x00, //   CRC checksum
            0x4B, 0x6D, 0x29, 0xDC, // sRGB chunk
            //   length: 1 byte
            0x00, 0x00, 0x00, 0x01, //   chunk type: sRGB
            0x73, 0x52, 0x47, 0x42,
            //   data:
            //     rendering intent: relative colorimetric
            0x01, //   CRC checksum
            0xD9, 0xC9, 0x2C, 0x7F, // gAMA chunk
            //   length: 4 bytes
            0x00, 0x00, 0x00, 0x04, //   chunk type: gAMA
            0x67, 0x41, 0x4D, 0x41, //   data:
            //     gamma: 0.45455
            0x00, 0x00, 0xB1, 0x8F, //   CRC checksum
            0x0B, 0xFC, 0x61, 0x05, // cHRM chunk
            //   length: 32 bytes
            0x00, 0x00, 0x00, 0x20, //   chunk type: cHRM
            0x63, 0x48, 0x52, 0x4D, //   data:
            //     white point X: 0.3127
            0x00, 0x00, 0x7A, 0x26, //     white point Y: 0.329
            0x00, 0x00, 0x80, 0x84, //     red X: 0.64
            0x00, 0x00, 0xFA, 0x00, //     red Y: 0.33
            0x00, 0x00, 0x80, 0xE8, //     green X: 0.3
            0x00, 0x00, 0x75, 0x30, //     green Y: 0.6
            0x00, 0x00, 0xEA, 0x60, //     blue X: 0.15
            0x00, 0x00, 0x3A, 0x98, //     blue Y: 0.06
            0x00, 0x00, 0x17, 0x70, //   CRC checksum
            0x9C, 0xBA, 0x51, 0x3C, // acTL chunk
            //   length: 8 bytes
            0x00, 0x00, 0x00, 0x08, //   chunk type: acTL
            0x61, 0x63, 0x54, 0x4C, //   data:
            //     2 frames
            0x00, 0x00, 0x00, 0x02, //     repeat forever
            0x00, 0x00, 0x00, 0x00, //   CRC checksum
            0xF3, 0x8D, 0x93, 0x70, // fcTL chunk
            //   length: 26 bytes
            0x00, 0x00, 0x00, 0x1A, //   chunk type: fcTL
            0x66, 0x63, 0x54, 0x4C, //   data:
            //     sequence number: 0
            0x00, 0x00, 0x00, 0x00, //     width: 8px
            0x00, 0x00, 0x00, 0x08, //     height: 8px
            0x00, 0x00, 0x00, 0x08, //     X offset: 0px
            0x00, 0x00, 0x00, 0x00, //     Y offset: 0px
            0x00, 0x00, 0x00, 0x00, //     delay: 1 / 1 seconds
            0x00, 0x01, 0x00, 0x01, //     dispose: none
            0x00, //     blend: source
            0x00, //   CRC checksum
            0xfe, 0x1b, 0x55, 0x83, // IDAT chunk
            //   length: 15 bytes
            0x00, 0x00, 0x00, 0x0F, //   chunk type: IDAT
            0x49, 0x44, 0x41, 0x54, //   data:
            0x78, 0x9C, 0x63, 0xF8, 0xCF, 0x80, 0x1D, 0x0D, 0x31, 0x09, 0x00, 0x28, 0xFF, 0x3F,
            0xC1, //   CRC checksum
            0x9D, 0xD1, 0xBA, 0x12, // fcTL chunk
            //   length: 26 bytes
            0x00, 0x00, 0x00, 0x1A, //   chunk type: fcTL
            0x66, 0x63, 0x54, 0x4C, //   data:
            //     sequence number: 1
            0x00, 0x00, 0x00, 0x01, //     width: 8px
            0x00, 0x00, 0x00, 0x08, //     height: 8px
            0x00, 0x00, 0x00, 0x08, //     X offset: 0px
            0x00, 0x00, 0x00, 0x00, //     Y offset: 0px
            0x00, 0x00, 0x00, 0x00, //     delay: 1 / 1 seconds
            0x00, 0x01, 0x00, 0x01, //     dispose: none
            0x00, //     blend: source
            0x00, //   CRC checksum
            0x65, 0x68, 0xBF, 0x57, // fdAT chunk
            //   length: 20 bytes
            0x00, 0x00, 0x00, 0x14, //   chunk type: fdAT
            0x66, 0x64, 0x41, 0x54, //   data:
            //     sequence number: 2
            0x00, 0x00, 0x00, 0x02, //     image data:
            0x78, 0x9C, 0x63, 0x60, 0x60, 0xF8, 0x8F, 0x03, 0x0D, 0x29, 0x09, 0x00, 0xA9, 0x70,
            0x3F, 0xC1, //   CRC checksum
            0x76, 0xD8, 0x64, 0xF3, // IEND chunk
            //   length: 0 bytes
            0x00, 0x00, 0x00, 0x00, //   chunk type: IEND
            0x49, 0x45, 0x4E, 0x44, //   data:
            //   CRC checksum
            0xAE, 0x42, 0x60, 0x82,
        ][..]
    );
}
