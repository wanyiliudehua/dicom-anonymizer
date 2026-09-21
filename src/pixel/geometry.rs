//! Pure pixel geometry and byte-level region masking.
//!
//! Operates on raw pixel bytes (uncompressed, little-endian), so it is fully
//! testable without any DICOM object.

use crate::error::{AnonError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub rows: u32,
    pub columns: u32,
    pub samples_per_pixel: u16,
    pub bits_allocated: u16,
    pub frames: u32,
}

impl Geometry {
    pub fn bytes_per_sample(&self) -> usize {
        (self.bits_allocated as usize).div_ceil(8)
    }
    pub fn frame_len_bytes(&self) -> usize {
        self.rows as usize
            * self.columns as usize
            * self.samples_per_pixel as usize
            * self.bytes_per_sample()
    }
}

fn write_sample(px: &mut [u8], offset: usize, bytes: usize, fill: u16) {
    match bytes {
        1 => px[offset] = fill.min(255) as u8,
        2 => {
            let b = fill.to_le_bytes();
            px[offset] = b[0];
            px[offset + 1] = b[1];
        }
        _ => {}
    }
}

fn bounds_ok(geo: &Geometry, x: u32, y: u32, w: u32, h: u32) -> bool {
    w > 0
        && h > 0
        && x.checked_add(w).map_or(false, |r| r <= geo.columns)
        && y.checked_add(h).map_or(false, |r| r <= geo.rows)
}

/// Fill an axis-aligned rectangle; returns the number of samples written.
pub fn mask_rect_in_place(
    pixels: &mut [u8],
    geo: &Geometry,
    frame: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    fill: u16,
) -> Result<usize> {
    if !bounds_ok(geo, x, y, w, h) {
        return Err(AnonError::Dicom(format!(
            "region ({x},{y},{w},{h}) exceeds {columns}x{rows}",
            columns = geo.columns,
            rows = geo.rows
        )));
    }
    if frame >= geo.frames {
        return Err(AnonError::Dicom(format!("frame {frame} out of range")));
    }
    let bps = geo.bytes_per_sample();
    let frame_off = frame as usize * geo.frame_len_bytes();
    let mut written = 0usize;
    for row in y..y + h {
        for col in x..x + w {
            let sample0 = (row as usize * geo.columns as usize + col as usize)
                * geo.samples_per_pixel as usize;
            for s in 0..geo.samples_per_pixel as usize {
                let off = frame_off + (sample0 + s) * bps;
                write_sample(pixels, off, bps, fill);
                written += 1;
            }
        }
    }
    Ok(written)
}

fn point_in_polygon(x: f64, y: f64, pts: &[[i64; 2]]) -> bool {
    let mut inside = false;
    let n = pts.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (pts[i][0] as f64, pts[i][1] as f64);
        let (xj, yj) = (pts[j][0] as f64, pts[j][1] as f64);
        if ((yi > y) != (yj > y)) && (x < (xj - xi) * (y - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Fill a polygon (even-odd rule); returns the number of samples written.
pub fn mask_polygon_in_place(
    pixels: &mut [u8],
    geo: &Geometry,
    frame: u32,
    points: &[[i64; 2]],
    fill: u16,
) -> Result<usize> {
    if points.len() < 3 {
        return Err(AnonError::Dicom("polygon needs >= 3 points".into()));
    }
    if frame >= geo.frames {
        return Err(AnonError::Dicom(format!("frame {frame} out of range")));
    }
    let bps = geo.bytes_per_sample();
    let frame_off = frame as usize * geo.frame_len_bytes();
    let mut written = 0usize;
    for row in 0..geo.rows {
        for col in 0..geo.columns {
            if !point_in_polygon(col as f64 + 0.5, row as f64 + 0.5, points) {
                continue;
            }
            let sample0 = (row as usize * geo.columns as usize + col as usize)
                * geo.samples_per_pixel as usize;
            for s in 0..geo.samples_per_pixel as usize {
                let off = frame_off + (sample0 + s) * bps;
                write_sample(pixels, off, bps, fill);
                written += 1;
            }
        }
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geo8() -> Geometry {
        Geometry { rows: 2, columns: 2, samples_per_pixel: 1, bits_allocated: 8, frames: 1 }
    }

    #[test]
    fn mask_rect_sets_exact_bytes_8bit() {
        let mut px = vec![0u8; 4];
        let n = mask_rect_in_place(&mut px, &geo8(), 0, 0, 0, 1, 1, 255).unwrap();
        assert_eq!(n, 1);
        assert_eq!(px, vec![255, 0, 0, 0]);
    }

    #[test]
    fn mask_rect_sets_exact_samples_16bit() {
        let geo = Geometry { rows: 1, columns: 2, samples_per_pixel: 1, bits_allocated: 16, frames: 1 };
        let mut px = vec![0u8; 4];
        let n = mask_rect_in_place(&mut px, &geo, 0, 1, 0, 1, 1, 7).unwrap();
        assert_eq!(n, 1);
        assert_eq!(px, vec![0, 0, 7, 0]); // little-endian u16
    }

    #[test]
    fn mask_is_multichannel_aware() {
        let geo = Geometry { rows: 1, columns: 1, samples_per_pixel: 3, bits_allocated: 8, frames: 1 };
        let mut px = vec![0u8; 3];
        let n = mask_rect_in_place(&mut px, &geo, 0, 0, 0, 1, 1, 9).unwrap();
        assert_eq!(n, 3);
        assert_eq!(px, vec![9, 9, 9]);
    }

    #[test]
    fn out_of_bounds_region_is_error() {
        let mut px = vec![0u8; 4];
        assert!(mask_rect_in_place(&mut px, &geo8(), 0, 1, 0, 2, 1, 1).is_err());
    }

    #[test]
    fn frame_selection_offsets_correctly() {
        let geo = Geometry { rows: 1, columns: 1, samples_per_pixel: 1, bits_allocated: 8, frames: 2 };
        let mut px = vec![0u8; 2];
        mask_rect_in_place(&mut px, &geo, 1, 0, 0, 1, 1, 5).unwrap();
        assert_eq!(px, vec![0, 5]);
    }

    #[test]
    fn polygon_masks_a_triangle() {
        let geo = Geometry { rows: 3, columns: 3, samples_per_pixel: 1, bits_allocated: 8, frames: 1 };
        let mut px = vec![0u8; 9];
        let pts = [[0i64, 0], [3, 0], [0, 3]];
        let n = mask_polygon_in_place(&mut px, &geo, 0, &pts, 1).unwrap();
        assert!(n >= 3);
        assert_eq!(px[0], 1); // inside
    }
}
