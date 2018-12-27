#![allow(dead_code)]

#[derive(Copy, Clone, Debug)]
#[repr(u8)]
pub enum ColorType {
    Grayscale = 0,
    RGB = 2,
    Palette = 3,
    GrayscaleWithAlpha = 4,
    RGBA = 6,
}

#[derive(Copy, Clone, Debug)]
#[repr(u8)]
pub enum CompressionMethod {
    Deflate = 0,
}

#[derive(Copy, Clone, Debug)]
#[repr(u8)]
pub enum FilterMethod {
    Adaptive = 0,
}

#[derive(Copy, Clone, Debug)]
#[repr(u8)]
pub enum FilterType {
    None = 0,
    Sub = 1,
    Up = 2,
    Average = 3,
    Paeth = 4,
}

#[derive(Copy, Clone, Debug)]
#[repr(u8)]
pub enum InterlaceMethod {
    None = 0,
    Adam7 = 1,
}

#[derive(Copy, Clone, Debug)]
#[repr(u8)]
pub enum Unit {
    Unknown = 0,
    Meter = 1,
}

#[derive(Copy, Clone, Debug)]
#[repr(u8)]
pub enum SrgbIntent {
    Perceptual = 0,
    RelativeColorimetric = 1,
    Saturation = 2,
    AbsoluteColorimetric = 3,
}

#[derive(Copy, Clone, Debug)]
#[repr(u8)]
pub enum DisposeOp {
    None = 0,
    Background = 1,
    Previous = 2,
}

#[derive(Copy, Clone, Debug)]
#[repr(u8)]
pub enum BlendOp {
    Source = 0,
    Over = 1,
}
