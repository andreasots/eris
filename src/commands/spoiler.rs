use crate::apng::{BlendOp, DisposeOp, Encoder, FrameControl};
use crate::config::Config;
use byteorder::{ByteOrder, NativeEndian};
use serenity::framework::standard::{Args, Command, CommandError};
use serenity::model::prelude::*;
use serenity::prelude::*;
use std::io::Cursor;
use std::sync::Arc;

use cairo::{Format, ImageSurface};
use pango::prelude::*;
use pango::{FontDescription, Layout};
use pangocairo::FontMap;

// This needs to be larger than the Discord's thumbnailer threshold which is currently 400x300 px.
// However if it's too small it will show the message on iOS. 3200px seems to work for now.
const WIDTH: i32 = 3200;
const MARGIN: i32 = 80;

enum Input<'a> {
    Markup(&'a str),
    Text(&'a str),
}

pub struct Spoiler {
    config: Arc<Config>,
}

impl Spoiler {
    pub fn new(config: Arc<Config>) -> Spoiler {
        Spoiler { config }
    }

    fn layout(ctx: &pango::Context, text: Input, font: &FontDescription) -> Layout {
        let layout = Layout::new(ctx);

        layout.set_font_description(font);
        layout.set_width((WIDTH - 2 * MARGIN) * pango::SCALE);
        layout.set_justify(true);

        match text {
            Input::Markup(text) => layout.set_markup(text),
            Input::Text(text) => layout.set_text(text),
        }

        layout
    }

    fn draw(height: i32, layout: &Layout) -> Result<ImageSurface, CommandError> {
        let surface = ImageSurface::create(Format::Rgb24, WIDTH, height + 2 * MARGIN)
            .map_err(|e| format!("`ImageSurface::create()` returned an error: `{:?}`", e))?;

        let cairo_ctx = cairo::Context::new(&surface);
        cairo_ctx.set_source_rgb(52.0 / 255.0, 54.0 / 255.0, 60.0 / 255.0);
        cairo_ctx.rectangle(
            0.0,
            0.0,
            surface.get_width() as f64,
            surface.get_height() as f64,
        );
        cairo_ctx.fill();
        cairo_ctx.set_source_rgb(1.0, 1.0, 1.0);
        cairo_ctx.move_to(MARGIN as f64, MARGIN as f64);

        pangocairo::functions::update_layout(&cairo_ctx, &layout);
        pangocairo::functions::show_layout(&cairo_ctx, &layout);

        Ok(surface)
    }

    fn surface_to_png_frame(surface: &mut ImageSurface) -> Result<Vec<u8>, CommandError> {
        let width = surface.get_width();
        let height = surface.get_height();
        let stride = surface.get_stride() as usize;

        let mut image_data = Vec::with_capacity(width as usize * height as usize * 3);
        {
            let raw_data = surface
                .get_data()
                .map_err(|e| format!("`ImageSurface::get_data()` returned an error: `{:?}`", e))?;

            for row in 0..height as usize {
                for col in 0..width as usize {
                    let pixel = NativeEndian::read_u32(
                        &raw_data[row * stride + col * 4..row * stride + (col + 1) * 4],
                    );
                    let r = ((pixel & 0x00_FF_00_00) >> 16) as u8;
                    let g = ((pixel & 0x00_00_FF_00) >> 8) as u8;
                    let b = ((pixel & 0x00_00_00_FF) >> 0) as u8;

                    image_data.push(r);
                    image_data.push(g);
                    image_data.push(b);
                }
            }
        }

        Ok(image_data)
    }

    fn render(&self, topic: &str, message: &str) -> Result<Vec<u8>, CommandError> {
        let spoiler = format!("[{}] {}", topic, message);

        let font_map = FontMap::get_default().ok_or("`FontMap::get_default()` returned `None`")?;
        let pango_ctx = font_map
            .create_context()
            .ok_or("`FontMapExt::create_context()` returned `None`")?;

        let mut font = FontDescription::new();
        font.set_family("sans");
        font.set_size((6.0 * 11.25 * pango::SCALE as f64) as i32);

        let usage_layout = Self::layout(
            &pango_ctx,
            Input::Markup(&format!(
                concat!(
                    "Click and then open in a browser to reveal the spoiler.\n",
                    "See ",
                    "<span font_family=\"monospace\" background=\"#292b30\">{}help spoiler</span> ",
                    "for more.",
                ),
                self.config.command_prefix
            )),
            &font,
        );
        let spoiler_layout = Self::layout(&pango_ctx, Input::Text(&spoiler), &font);

        let height = std::cmp::max(
            usage_layout.get_pixel_extents().1.height,
            spoiler_layout.get_pixel_extents().1.height,
        );

        let mut usage_surface = Self::draw(height, &usage_layout)?;
        let mut spoiler_surface = Self::draw(height, &spoiler_layout)?;

        let usage_frame = Self::surface_to_png_frame(&mut usage_surface)?;
        let spoiler_frame = Self::surface_to_png_frame(&mut spoiler_surface)?;

        let mut apng = vec![];
        let mut encoder = Encoder::new(
            Cursor::new(&mut apng),
            usage_surface.get_width() as u32,
            usage_surface.get_height() as u32,
        )?;
        encoder.enable_animation(1, 1)?;
        encoder.write_image(&usage_frame, None)?;
        encoder.write_frame(
            &spoiler_frame,
            FrameControl {
                width: spoiler_surface.get_width() as u32,
                height: spoiler_surface.get_height() as u32,
                x_offset: 0,
                y_offset: 0,
                delay_num: 0,
                delay_den: 1,
                dispose_op: DisposeOp::None,
                blend_op: BlendOp::Source,
            },
        )?;
        encoder.finish()?;

        Ok(apng)
    }
}

impl Command for Spoiler {
    fn execute(&self, _: &mut Context, msg: &Message, mut args: Args) -> Result<(), CommandError> {
        msg.delete()?;

        let topic = args.single_quoted::<String>()?;
        let message = args.rest();

        let image = self
            .render(&topic, message)
            .map_err(|e| format!("failed to render the image: {}", e.0))?;

        msg.channel_id
            .send_files(vec![(&image[..], "spoiler.png")], |m| {
                m.content(format!("{}: **{}** spoiler:", msg.author.mention(), topic))
            })?;

        Ok(())
    }
}
