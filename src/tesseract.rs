use std::ffi::{CStr, c_char, c_int};
use std::io::Cursor;
use std::num::TryFromIntError;
use std::ptr::NonNull;
use std::str::Utf8Error;

use anyhow::{Context, Error};
use deadpool::managed::{Manager, Metrics, RecycleResult};
use image::{ImageFormat, ImageReader};
use tesseract_sys::{
    PageIteratorLevel, TessBaseAPI, TessOcrEngineMode_OEM_DEFAULT, TessPageIteratorLevel,
    TessResultIterator,
};

const ENG_TRAINEDDATA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/eng.traineddata"));

pub struct Tesseract {}

impl Tesseract {
    pub fn new() -> Self {
        Self {}
    }
}

impl Manager for Tesseract {
    type Type = TessBaseApi;

    type Error = TesseractError;

    async fn create(&self) -> Result<Self::Type, Self::Error> {
        let mut tesseract = TessBaseApi::create()?;
        tesseract.init(ENG_TRAINEDDATA, c"eng")?;

        Ok(tesseract)
    }

    async fn recycle(&self, obj: &mut Self::Type, _: &Metrics) -> RecycleResult<Self::Error> {
        obj.clear();

        Ok(())
    }
}

pub struct TessBaseApi {
    ptr: NonNull<TessBaseAPI>,
}

impl TessBaseApi {
    pub fn create() -> Result<Self, TesseractError> {
        Ok(Self {
            ptr: NonNull::new(unsafe { tesseract_sys::TessBaseAPICreate() })
                .ok_or(TesseractError::Create)?,
        })
    }

    pub fn init(&mut self, traineddata: &[u8], language: &CStr) -> Result<(), TesseractError> {
        let res = unsafe {
            tesseract_sys::TessBaseAPIInit5(
                self.ptr.as_ptr(),
                traineddata.as_ptr().cast::<c_char>(),
                traineddata.len().try_into().map_err(TesseractError::DataTooBig)?,
                language.as_ptr(),
                TessOcrEngineMode_OEM_DEFAULT,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
                0,
            )
        };

        if res == 0 { Ok(()) } else { Err(TesseractError::Init) }
    }

    pub fn clear(&mut self) {
        unsafe { tesseract_sys::TessBaseAPIClear(self.ptr.as_ptr()) }
    }

    pub fn set_image(
        &mut self,
        image_data: &[u8],
        width: c_int,
        height: c_int,
        bytes_per_pixel: c_int,
        bytes_per_line: c_int,
    ) -> Result<(), TesseractError> {
        if width <= 0 || height <= 0 {
            return Err(TesseractError::InvalidImageDimensions);
        }
        if bytes_per_pixel <= 0 {
            return Err(TesseractError::InvalidImageDepth);
        }
        if width * bytes_per_pixel > bytes_per_line {
            return Err(TesseractError::InvalidImageStride);
        }
        if (height * bytes_per_line) as usize > image_data.len() {
            return Err(TesseractError::InvalidImageData);
        }

        unsafe {
            tesseract_sys::TessBaseAPISetImage(
                self.ptr.as_ptr(),
                image_data.as_ptr(),
                width,
                height,
                bytes_per_pixel,
                bytes_per_line,
            );
        }

        Ok(())
    }

    pub fn recognize(&mut self) -> Result<(), TesseractError> {
        let res =
            unsafe { tesseract_sys::TessBaseAPIRecognize(self.ptr.as_ptr(), std::ptr::null_mut()) };

        if res == 0 { Ok(()) } else { Err(TesseractError::Ocr) }
    }

    pub fn get_iterator(&mut self) -> Result<ResultIterator<'_>, TesseractError> {
        let ptr = unsafe { tesseract_sys::TessBaseAPIGetIterator(self.ptr.as_ptr()) };
        let ptr = NonNull::new(ptr).ok_or(TesseractError::GetIterator)?;

        Ok(ResultIterator { ptr, _tess: self })
    }
}

impl Drop for TessBaseApi {
    fn drop(&mut self) {
        unsafe { tesseract_sys::TessBaseAPIDelete(self.ptr.as_ptr()) }
    }
}

unsafe impl Send for TessBaseApi {}

pub struct ResultIterator<'a> {
    ptr: NonNull<TessResultIterator>,
    _tess: &'a mut TessBaseApi,
}

impl ResultIterator<'_> {
    pub fn confidence(&self, level: PageIteratorLevel) -> f32 {
        unsafe {
            tesseract_sys::TessResultIteratorConfidence(
                self.ptr.as_ptr(),
                level as TessPageIteratorLevel,
            )
        }
    }

    pub fn text(&self, level: PageIteratorLevel) -> Result<String, TesseractError> {
        unsafe {
            let ptr = tesseract_sys::TessResultIteratorGetUTF8Text(
                self.ptr.as_ptr(),
                level as TessPageIteratorLevel,
            );

            if !ptr.is_null() {
                let string =
                    CStr::from_ptr(ptr).to_str().map(String::from).map_err(TesseractError::Utf8);

                tesseract_sys::TessDeleteText(ptr);

                string
            } else {
                return Err(TesseractError::GetText);
            }
        }
    }

    pub fn is_at_final_element(
        &self,
        level: PageIteratorLevel,
        element: PageIteratorLevel,
    ) -> bool {
        unsafe {
            tesseract_sys::TessPageIteratorIsAtFinalElement(
                tesseract_sys::TessResultIteratorGetPageIteratorConst(self.ptr.as_ptr()),
                level as TessPageIteratorLevel,
                element as TessPageIteratorLevel,
            ) != 0
        }
    }

    pub fn next(&mut self, level: PageIteratorLevel) -> bool {
        unsafe {
            tesseract_sys::TessResultIteratorNext(self.ptr.as_ptr(), level as TessPageIteratorLevel)
                != 0
        }
    }
}

impl Drop for ResultIterator<'_> {
    fn drop(&mut self) {
        unsafe {
            tesseract_sys::TessResultIteratorDelete(self.ptr.as_ptr());
        }
    }
}

#[derive(Debug)]
pub enum TesseractError {
    Create,
    Init,
    DataTooBig(TryFromIntError),
    InvalidImageDimensions,
    InvalidImageDepth,
    InvalidImageStride,
    InvalidImageData,
    Ocr,
    GetIterator,
    GetText,
    Utf8(Utf8Error),
}

impl std::fmt::Display for TesseractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Create => f.write_str("failed to create a TessBaseAPI instance"),
            Self::Init => f.write_str("failed to initialize a TessBaseAPI instance"),
            Self::DataTooBig(error) => write!(f, "trained model too big: {error}"),
            Self::InvalidImageDimensions => f.write_str("invalid image dimensions"),
            Self::InvalidImageDepth => f.write_str("invalid image depth (bytes-per-pixel)"),
            Self::InvalidImageStride => f.write_str("invalid image stride (bytes-per-line)"),
            Self::InvalidImageData => f.write_str("invalid image data size"),
            Self::Ocr => f.write_str("failed to OCR the image"),
            Self::GetIterator => f.write_str("failed to create a result iterator"),
            Self::GetText => f.write_str("failed to get the text of the current object"),
            Self::Utf8(error) => write!(f, "Tesseract returned invalid UTF-8: {error}"),
        }
    }
}

impl std::error::Error for TesseractError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::DataTooBig(error) => Some(error),
            Self::Utf8(error) => Some(error),
            _ => None,
        }
    }
}

pub fn extract_text(
    tesseract: &mut TessBaseApi,
    image: &[u8],
) -> Result<(ImageFormat, String), Error> {
    let loader = ImageReader::new(Cursor::new(image))
        .with_guessed_format()
        .context("failed to determine the image format")?;
    let Some(format) = loader.format() else {
        anyhow::bail!("failed to determine the image format");
    };
    let image = loader.decode().context("failed to decode the image")?.into_rgb8();

    let layout = image.sample_layout();

    tesseract
        .set_image(
            image.as_raw(),
            layout.width.try_into().context("width too big")?,
            layout.height.try_into().context("height too big")?,
            layout.width_stride.try_into().context("pixel stride too big")?,
            layout.height_stride.try_into().context("line stride too big")?,
        )
        .context("failed to set the image to be OCRd")?;

    tesseract.recognize().context("failed to OCR the image")?;

    let mut iter = tesseract.get_iterator().context("failed to get the iterator")?;

    let mut text = String::new();
    let mut last_was_newline = false;

    loop {
        let confidence = iter.confidence(PageIteratorLevel::RIL_WORD);
        if confidence > 70.0 {
            last_was_newline = false;
            text.push_str(
                &iter.text(PageIteratorLevel::RIL_WORD).context("failed to get the word")?,
            );

            if !iter
                .is_at_final_element(PageIteratorLevel::RIL_TEXTLINE, PageIteratorLevel::RIL_WORD)
            {
                text.push(' ');
            }
        }

        if iter.is_at_final_element(PageIteratorLevel::RIL_TEXTLINE, PageIteratorLevel::RIL_WORD)
            && !last_was_newline
        {
            text.push('\n');
            last_was_newline = true;
        }

        if !iter.next(PageIteratorLevel::RIL_WORD) {
            break;
        }
    }

    Ok((format, text))
}
