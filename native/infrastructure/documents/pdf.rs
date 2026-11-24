//! Render PDF pages as JPEGs for models with image input.
use super::{Failure, Outcome, Part, context_limit, image};
use hayro::{
    PixmapSettings, RenderCache, RenderSettings,
    hayro_interpret::InterpreterSettings,
    hayro_syntax::{Pdf, page::Page},
    vello_cpu::color::palette::css::WHITE,
};
use image::{ExtendedColorType, codecs::jpeg::JpegEncoder};
pub(super) fn render(bytes: Vec<u8>, image_budget: usize) -> Outcome<Vec<Part>> {
    let pdf = Pdf::new(bytes).map_err(|_| Failure::document())?;
    let pages = pdf.pages();
    if pages.len() > 20 {
        return Err(context_limit());
    }
    // Pages render independently. A few threads keep the document worker within its memory limit.
    let workers = std::thread::available_parallelism()
        .map_or(1, |count| count.get())
        .clamp(1, 4)
        .min(pages.len().max(1));
    let encoded: Vec<Outcome<Vec<u8>>> = std::thread::scope(|scope| {
        let threads: Vec<_> = (0..workers)
            .map(|worker| {
                scope.spawn(move || {
                    let cache = RenderCache::new();
                    let interpreter = InterpreterSettings::default();
                    let settings = RenderSettings::default();
                    pages
                        .iter()
                        .skip(worker)
                        .step_by(workers)
                        .map(|page| encode(page, &cache, &interpreter, &settings))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let mut rendered: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap_or_default().into_iter())
            .collect();
        (0..pages.len())
            .map(|index| {
                rendered[index % workers]
                    .next()
                    .unwrap_or_else(|| Err(Failure::document()))
            })
            .collect()
    });
    let mut parts = Vec::new();
    let mut image_bytes = 0;
    for jpeg in encoded {
        let jpeg = jpeg?;
        image_bytes += jpeg.len().div_ceil(3) * 4 + "data:image/jpeg;base64,".len();
        if image_bytes > image_budget {
            return Err(context_limit());
        }
        parts.push(image("image/jpeg", &jpeg));
    }
    if parts.is_empty() {
        return Err(Failure::document());
    }
    Ok(parts)
}
fn encode<'a>(
    page: &'a Page<'a>,
    cache: &RenderCache<'a>,
    interpreter: &InterpreterSettings,
    settings: &RenderSettings,
) -> Outcome<Vec<u8>> {
    let (width, height) = page.render_dimensions();
    let scale = (1600.0 / width.max(height)).min(2.0);
    if !width.is_finite() || !height.is_finite() || width * scale < 1.0 || height * scale < 1.0 {
        return Err(Failure::document());
    }
    let bitmap = hayro::render(
        page,
        cache,
        interpreter,
        settings,
        &PixmapSettings {
            x_scale: scale,
            y_scale: scale,
            bg_color: WHITE,
        },
    );
    let rgb: Vec<u8> = bitmap
        .data_as_u8_slice()
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|pixel| [pixel[0], pixel[1], pixel[2]])
        .collect();
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, 82)
        .encode(
            &rgb,
            u32::from(bitmap.width()),
            u32::from(bitmap.height()),
            ExtendedColorType::Rgb8,
        )
        .map_err(|_| Failure::document())?;
    Ok(jpeg)
}
