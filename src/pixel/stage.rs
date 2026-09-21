//! Pixel stage: transcode compressed pixel data to uncompressed, apply the
//! configured region masks, and write the pixel data back uncompressed.

use crate::config::{Config, FillValue, OnMissing, PixelMode, Shape};
use crate::deid::dataset as ds;
use crate::error::{AnonError, Result};
use crate::pixel::geometry::{mask_polygon_in_place, mask_rect_in_place, Geometry};
use crate::pixel::regions::{frames_for, select};
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_dictionary_std::tags;
use dicom_object::{FileDicomObject, InMemDicomObject};
use dicom_pixeldata::Transcode;

/// Build a geometry from optional tag values, defaulting sensibly.
pub fn geometry_from_tags(
    rows: Option<u32>,
    columns: Option<u32>,
    frames: Option<u32>,
    samples_per_pixel: Option<u16>,
    bits_allocated: Option<u16>,
) -> Geometry {
    Geometry {
        rows: rows.unwrap_or(0),
        columns: columns.unwrap_or(0),
        samples_per_pixel: samples_per_pixel.unwrap_or(1),
        bits_allocated: bits_allocated.unwrap_or(8),
        frames: frames.unwrap_or(1),
    }
}

/// Resolve the configured fill value against the sample depth.
pub fn resolve_fill(cfg: &Config, bits: u16) -> u16 {
    match &cfg.pixels.fill {
        FillValue::Num(n) => *n,
        FillValue::Named(_) => {
            if bits >= 16 {
                u16::MAX
            } else {
                255
            }
        }
    }
}

fn get_u32(obj: &InMemDicomObject, t: Tag) -> Option<u32> {
    obj.element(t).ok().and_then(|e| e.to_int::<u32>().ok())
}
fn get_u16(obj: &InMemDicomObject, t: Tag) -> Option<u16> {
    obj.element(t).ok().and_then(|e| e.to_int::<u16>().ok())
}

/// What the pixel stage did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PixelReport {
    /// Number of mask operations applied (configured regions, or one for the
    /// whole-frame `mask_all` fallback).
    pub regions_masked: usize,
    /// Number of individual samples written.
    pub samples_masked: usize,
}

/// JPEG-LS transfer syntaxes need the optional `charls` feature.
fn unsupported_without_feature(uid: &str) -> bool {
    #[cfg(feature = "charls")]
    {
        let _ = uid;
        false
    }
    #[cfg(not(feature = "charls"))]
    {
        matches!(uid, "1.2.840.10008.1.2.4.80" | "1.2.840.10008.1.2.4.81")
    }
}

/// Transcode to uncompressed, mask configured regions, write pixels back.
pub fn process(obj: &mut FileDicomObject<InMemDicomObject>, cfg: &Config) -> Result<PixelReport> {
    if cfg.pixels.mode == PixelMode::Keep {
        return Ok(PixelReport::default());
    }
    let modality = ds::get_string(obj, tags::MODALITY);
    let sop = ds::get_string(obj, tags::SOP_CLASS_UID);
    let selected = select(cfg, modality.as_deref(), sop.as_deref());
    if selected.is_empty() {
        return Ok(PixelReport::default());
    }

    // No pixel data (e.g. SR): nothing to mask and nothing to transcode.
    if obj.element(tags::PIXEL_DATA).is_err() {
        return Ok(PixelReport::default());
    }

    let burned = ds::get_string(obj, tags::BURNED_IN_ANNOTATION);
    let mask_all = match (burned.is_none(), &cfg.pixels.on_missing_burnedin) {
        (true, OnMissing::Skip) => return Ok(PixelReport::default()),
        (true, OnMissing::MaskAll) => true,
        (true, OnMissing::Warn) => {
            eprintln!(
                "warning: BurnedInAnnotation (0028,0301) absent; applying configured regions only"
            );
            false
        }
        (false, _) => false,
    };

    // Decompress to Explicit VR LE if the source uses another transfer syntax.
    let ts_uid = obj.meta().transfer_syntax().to_string();
    if ts_uid != "1.2.840.10008.1.2.1" {
        if unsupported_without_feature(&ts_uid) {
            return Err(AnonError::Unsupported(ts_uid));
        }
        let ts = dicom_transfer_syntax_registry::entries::EXPLICIT_VR_LITTLE_ENDIAN.erased();
        obj.transcode_with_options(&ts, dicom_encoding::adapters::EncodeOptions::default())
            .map_err(|e| AnonError::Dicom(format!("transcode failed: {e}")))?;
    }

    let geo = geometry_from_tags(
        get_u32(obj, tags::ROWS),
        get_u32(obj, tags::COLUMNS),
        get_u32(obj, tags::NUMBER_OF_FRAMES),
        get_u16(obj, tags::SAMPLES_PER_PIXEL),
        get_u16(obj, tags::BITS_ALLOCATED),
    );
    if geo.rows == 0 || geo.columns == 0 {
        return Err(AnonError::Dicom("missing Rows/Columns".into()));
    }
    let fill = resolve_fill(cfg, geo.bits_allocated);

    // PlanarConfiguration=1 (colour-by-plane) needs plane-strided offsets; we
    // reject it rather than silently masking the wrong samples.
    let planar = get_u16(obj, tags::PLANAR_CONFIGURATION).unwrap_or(0);
    if planar == 1 && geo.samples_per_pixel > 1 {
        return Err(AnonError::Dicom(
            "PlanarConfiguration=1 is not supported for masking".into(),
        ));
    }

    let mut pixels = obj
        .element(tags::PIXEL_DATA)
        .map_err(|e| AnonError::Dicom(e.to_string()))?
        .to_bytes()
        .map_err(|e| AnonError::Dicom(e.to_string()))?
        .to_vec();

    let expected = geo.frame_len_bytes() * geo.frames as usize;
    if pixels.len() < expected {
        return Err(AnonError::Dicom(format!(
            "pixel buffer {} bytes < expected {}",
            pixels.len(),
            expected
        )));
    }

    let mut samples_masked = 0usize;
    let mut regions_masked = 0usize;
    if mask_all {
        for frame in 0..geo.frames {
            samples_masked += mask_rect_in_place(
                &mut pixels, &geo, frame, 0, 0, geo.columns, geo.rows, fill,
            )?;
        }
        regions_masked = 1;
    } else {
        for region in selected {
            let before = samples_masked;
            for frame in frames_for(region, geo.frames) {
                match region.shape {
                    Shape::Rect => {
                        samples_masked += mask_rect_in_place(
                            &mut pixels,
                            &geo,
                            frame,
                            region.x,
                            region.y,
                            region.width,
                            region.height,
                            fill,
                        )?;
                    }
                    Shape::Polygon => {
                        samples_masked += mask_polygon_in_place(
                            &mut pixels, &geo, frame, &region.points, fill,
                        )?;
                    }
                }
            }
            if samples_masked > before {
                regions_masked += 1;
            }
        }
    }

    let elem = if geo.bits_allocated > 8 {
        let words: Vec<u16> = pixels
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        DataElement::new(tags::PIXEL_DATA, VR::OW, PrimitiveValue::U16(words.into()))
    } else {
        DataElement::new(tags::PIXEL_DATA, VR::OB, PrimitiveValue::U8(pixels.into()))
    };
    obj.put_element(elem);
    Ok(PixelReport {
        regions_masked,
        samples_masked,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn geometry_defaults_absent_frames_to_one() {
        let g = geometry_from_tags(Some(2), Some(2), None, None, None);
        assert_eq!(g.frames, 1);
        assert_eq!(g.rows, 2);
        assert_eq!(g.samples_per_pixel, 1);
        assert_eq!(g.bits_allocated, 8);
    }

    #[test]
    fn missing_rows_or_columns_is_zero() {
        assert_eq!(geometry_from_tags(None, Some(2), None, None, None).rows, 0);
    }

    #[test]
    fn fill_resolution_max_uses_bits_allocated() {
        let cfg = Config::from_toml_str("[pixels]\nfill = \"max\"\n").unwrap();
        assert_eq!(resolve_fill(&cfg, 8), 255);
        assert_eq!(resolve_fill(&cfg, 16), 65535);
        let cfg0 = Config::from_toml_str("[pixels]\nfill = 7\n").unwrap();
        assert_eq!(resolve_fill(&cfg0, 16), 7);
    }

    fn build_pixel_obj(
        spp: u16,
        planar: Option<u16>,
    ) -> dicom_object::FileDicomObject<dicom_object::InMemDicomObject> {
        use dicom_core::{DataElement, PrimitiveValue};
        use dicom_object::{FileMetaTableBuilder, InMemDicomObject};
        let n_bytes = 2 * 2 * spp as usize;
        let mut elems = vec![
            DataElement::new(tags::SOP_CLASS_UID, VR::UI, "1.2.840.10008.5.1.4.1.1.7"),
            DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, "1.2.3.4"),
            DataElement::new(tags::ROWS, VR::US, PrimitiveValue::U16(vec![2u16].into())),
            DataElement::new(tags::COLUMNS, VR::US, PrimitiveValue::U16(vec![2u16].into())),
            DataElement::new(
                tags::BITS_ALLOCATED,
                VR::US,
                PrimitiveValue::U16(vec![8u16].into()),
            ),
            DataElement::new(
                tags::SAMPLES_PER_PIXEL,
                VR::US,
                PrimitiveValue::U16(vec![spp].into()),
            ),
            DataElement::new(
                tags::PIXEL_DATA,
                VR::OB,
                PrimitiveValue::U8(vec![0u8; n_bytes].into()),
            ),
        ];
        if let Some(p) = planar {
            elems.push(DataElement::new(
                tags::PLANAR_CONFIGURATION,
                VR::US,
                PrimitiveValue::U16(vec![p].into()),
            ));
        }
        InMemDicomObject::from_element_iter(elems)
            .with_meta(FileMetaTableBuilder::new().transfer_syntax("1.2.840.10008.1.2.1"))
            .unwrap()
    }

    #[test]
    fn mask_all_fills_whole_frame_when_burnedin_absent() {
        let cfg = Config::from_toml_str(
            "[pixels]\nmode=\"mask\"\nfill=9\non_missing_burnedin=\"mask_all\"\n\
             [[pixels.regions]]\nshape=\"rect\"\nx=0\ny=0\nwidth=1\nheight=1\n",
        )
        .unwrap();
        let mut file = build_pixel_obj(1, None);
        let n = process(&mut file, &cfg).unwrap();
        assert_eq!(n.samples_masked, 4);
        assert_eq!(n.regions_masked, 1);
        let bytes = file
            .element(tags::PIXEL_DATA)
            .unwrap()
            .to_bytes()
            .unwrap()
            .to_vec();
        assert!(bytes.iter().all(|b| *b == 9));
    }

    #[test]
    fn planar_configuration_one_is_error_when_masking() {
        let cfg = Config::from_toml_str(
            "[pixels]\nmode=\"mask\"\n[[pixels.regions]]\nshape=\"rect\"\nwidth=1\nheight=1\n",
        )
        .unwrap();
        let mut file = build_pixel_obj(3, Some(1));
        assert!(process(&mut file, &cfg).is_err());
    }

    #[test]
    #[cfg(not(feature = "charls"))]
    fn jpegls_needs_charls_feature() {
        assert!(unsupported_without_feature("1.2.840.10008.1.2.4.80"));
        assert!(unsupported_without_feature("1.2.840.10008.1.2.4.81"));
        assert!(!unsupported_without_feature("1.2.840.10008.1.2.1"));
    }

    /// A fuller fixture: sized, multi-frame, with the attributes an encoder needs.
    fn build_obj(
        rows: u16,
        cols: u16,
        spp: u16,
        frames: u16,
        ts: &str,
    ) -> dicom_object::FileDicomObject<dicom_object::InMemDicomObject> {
        use dicom_core::{DataElement, PrimitiveValue};
        use dicom_object::{FileMetaTableBuilder, InMemDicomObject};
        let u16v = |n: u16| PrimitiveValue::U16(vec![n].into());
        let n = rows as usize * cols as usize * spp as usize * frames as usize;
        let mut elems = vec![
            DataElement::new(tags::SOP_CLASS_UID, VR::UI, "1.2.840.10008.5.1.4.1.1.7"),
            DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, "1.2.3.4"),
            DataElement::new(tags::ROWS, VR::US, u16v(rows)),
            DataElement::new(tags::COLUMNS, VR::US, u16v(cols)),
            DataElement::new(tags::BITS_ALLOCATED, VR::US, u16v(8)),
            DataElement::new(tags::BITS_STORED, VR::US, u16v(8)),
            DataElement::new(tags::HIGH_BIT, VR::US, u16v(7)),
            DataElement::new(tags::PIXEL_REPRESENTATION, VR::US, u16v(0)),
            DataElement::new(tags::SAMPLES_PER_PIXEL, VR::US, u16v(spp)),
            DataElement::new(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, "MONOCHROME2"),
            DataElement::new(tags::BURNED_IN_ANNOTATION, VR::CS, "NO"),
        ];
        if frames > 1 {
            elems.push(DataElement::new(
                tags::NUMBER_OF_FRAMES,
                VR::IS,
                frames.to_string(),
            ));
        }
        elems.push(DataElement::new(
            tags::PIXEL_DATA,
            VR::OB,
            PrimitiveValue::U8(vec![0u8; n].into()),
        ));
        InMemDicomObject::from_element_iter(elems)
            .with_meta(FileMetaTableBuilder::new().transfer_syntax(ts))
            .unwrap()
    }

    #[test]
    fn multi_frame_masks_only_selected_frame() {
        let cfg = Config::from_toml_str(
            "[pixels]\nmode=\"mask\"\nfill=7\n[[pixels.regions]]\nshape=\"rect\"\nx=0\ny=0\nwidth=1\nheight=1\nframes=[1]\n",
        )
        .unwrap();
        let mut file = build_obj(2, 2, 1, 3, "1.2.840.10008.1.2.1");
        let r = process(&mut file, &cfg).unwrap();
        assert_eq!(r.regions_masked, 1);
        assert_eq!(r.samples_masked, 1);
        let px = file
            .element(tags::PIXEL_DATA)
            .unwrap()
            .to_bytes()
            .unwrap()
            .to_vec();
        assert_eq!(&px[0..4], &[0, 0, 0, 0]); // frame 0 untouched
        assert_eq!(px[4], 7); // frame 1, first sample
        assert_eq!(px[5], 0);
        assert_eq!(&px[8..12], &[0, 0, 0, 0]); // frame 2 untouched
    }

    #[test]
    fn compressed_source_is_transcoded_then_masked() {
        use dicom_pixeldata::Transcode;
        let mut file = build_obj(8, 8, 1, 1, "1.2.840.10008.1.2.1");
        let jpeg = dicom_transfer_syntax_registry::entries::JPEG_BASELINE.erased();
        file.transcode_with_options(&jpeg, dicom_encoding::adapters::EncodeOptions::default())
            .unwrap();
        assert_eq!(file.meta().transfer_syntax(), "1.2.840.10008.1.2.4.50");

        let cfg = Config::from_toml_str(
            "[pixels]\nmode=\"mask\"\nfill=9\n[[pixels.regions]]\nshape=\"rect\"\nx=0\ny=0\nwidth=4\nheight=4\n",
        )
        .unwrap();
        let r = process(&mut file, &cfg).unwrap();
        assert_eq!(r.regions_masked, 1);
        // output is uncompressed again
        assert_eq!(file.meta().transfer_syntax(), "1.2.840.10008.1.2.1");
        let px = file
            .element(tags::PIXEL_DATA)
            .unwrap()
            .to_bytes()
            .unwrap()
            .to_vec();
        assert_eq!(px.len(), 64);
        assert_eq!(px[0], 9);
        assert_eq!(px[1], 9);
    }
}
