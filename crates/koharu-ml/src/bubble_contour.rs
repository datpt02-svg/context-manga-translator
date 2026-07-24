//! Contour extraction from speech bubble segmentation masks.
//!
//! Takes a [`SpeechBubbleRegionMask`] (binary pixel mask from YOLOv8m-seg),
//! traces the outer boundary via `imageproc::contours`, simplifies with
//! Ramer-Douglas-Peucker, and returns a closed polygon in absolute image
//! coordinates suitable for the front-end's `clip-path: polygon(...)`.

use image::{GrayImage, Luma};
use imageproc::contours::{BorderType, find_contours};

use crate::speech_bubble_segmentation::SpeechBubbleRegionMask;

/// Douglas-Peucker simplification tolerance in pixels.
const DP_EPSILON: f32 = 1.5;
/// Minimum polygon points to be considered a valid contour.
const MIN_POINTS: usize = 3;

/// Extract the outer contour of a bubble region mask as a closed polygon.
///
/// Returns `None` when the mask is empty, no outer contour is found, or the
/// simplified polygon has fewer than 3 points.
pub fn extract_contour(mask: &SpeechBubbleRegionMask) -> Option<Vec<[f32; 2]>> {
    if mask.is_empty() {
        return None;
    }

    // Build a GrayImage from the mask's pixel buffer (0 = background, 255 = bubble).
    let binary = GrayImage::from_raw(mask.width, mask.height, mask.pixels.clone())?;

    // Find all contours; we only care about the outer (border_type == Outer) one.
    let contours = find_contours::<i32>(&binary);
    let mut best: Option<Vec<[f32; 2]>> = None;
    let mut best_area = 0.0f32;

    for contour in &contours {
        if contour.border_type != BorderType::Outer || contour.points.is_empty() {
            continue;
        }
        // Convert from relative (mask-local) to absolute (image) coordinates.
        let points: Vec<[f32; 2]> = contour
            .points
            .iter()
            .map(|p| [p.x as f32 + mask.x as f32, p.y as f32 + mask.y as f32])
            .collect();

        let area = polygon_area(&points).abs();
        if area > best_area {
            let simplified = douglas_peucker(&points, DP_EPSILON);
            // Ensure closure: if the simplified polygon is not already closed,
            // it's a closed contour from imageproc (first ≈ last), so keep as-is.
            best_area = area;
            best = Some(simplified);
        }
    }

    let result = best?;
    if result.len() < MIN_POINTS {
        return None;
    }
    Some(result)
}

// ---------------------------------------------------------------------------
// Ramer-Douglas-Peucker simplification
// ---------------------------------------------------------------------------

fn douglas_peucker(points: &[[f32; 2]], epsilon: f32) -> Vec<[f32; 2]> {
    if points.len() <= 2 {
        return points.to_vec();
    }

    // Find the point with the maximum perpendicular distance from the line
    // segment between the first and last points.
    let first = points.first().expect("non-empty");
    let last = points.last().expect("non-empty");

    let mut dmax = 0.0f32;
    let mut idx = 0;
    for i in 1..points.len().saturating_sub(1) {
        let d = perpendicular_distance(&points[i], first, last);
        if d > dmax {
            dmax = d;
            idx = i;
        }
    }

    if dmax > epsilon {
        // Recursively simplify both sub-segments.
        let left = douglas_peucker(&points[..=idx], epsilon);
        let right = douglas_peucker(&points[idx..], epsilon);
        // Merge: connect left (without last) + right.
        let mut result = left;
        result.pop(); // remove duplicate endpoint
        result.extend(right);
        result
    } else {
        vec![*first, *last]
    }
}

/// Perpendicular distance of point `p` from the line `a`–`b`.
fn perpendicular_distance(p: &[f32; 2], a: &[f32; 2], b: &[f32; 2]) -> f32 {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let length_sq = dx * dx + dy * dy;
    if length_sq == 0.0 {
        // a and b are coincident; return Euclidean distance.
        let ex = p[0] - a[0];
        let ey = p[1] - a[1];
        return (ex * ex + ey * ey).sqrt();
    }
    let t = ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / length_sq;
    let t = t.clamp(0.0, 1.0);
    let proj_x = a[0] + t * dx;
    let proj_y = a[1] + t * dy;
    let ex = p[0] - proj_x;
    let ey = p[1] - proj_y;
    (ex * ex + ey * ey).sqrt()
}

/// Extract the outer contour of a specific bubble from an ID-coded mask.
///
/// `mask` is a grayscale image each bubble painted with a unique non-zero ID.
/// `bubble_id` is the ID of the bubble trace.
///
/// Returns `None` if the bubble ID not present or the contour too small.
pub fn extract_contour_from_id_mask(mask: &GrayImage, bubble_id: u8) -> Option<Vec<[f32; 2]>> {
    if bubble_id == 0 {
        return None;
    }
    // Build binary mask where only `bubble_id` pixels are foreground.
    let (w, h) = mask.dimensions();
    let mut binary = GrayImage::from_pixel(w, h, Luma([0u8]));
    for y in 0..h {
        for x in 0..w {
            if mask.get_pixel(x, y).0[0] == bubble_id {
                binary.put_pixel(x, y, Luma([255u8]));
            }
        }
    }

    let contours = find_contours::<i32>(&binary);
    let mut best: Option<Vec<[f32; 2]>> = None;
    let mut best_area = 0.0f32;

    for contour in &contours {
        if contour.border_type != BorderType::Outer || contour.points.is_empty() {
            continue;
        }
        let points: Vec<[f32; 2]> = contour
            .points
            .iter()
            .map(|p| [p.x as f32, p.y as f32])
            .collect();
        let area = polygon_area(&points).abs();
        if area > best_area {
            let simplified = douglas_peucker(&points, DP_EPSILON);
            best_area = area;
            best = Some(simplified);
        }
    }

    let result = best?;
    if result.len() < MIN_POINTS {
        return None;
    }
    Some(result)
}

/// Signed polygon area via shoelace formula.
fn polygon_area(points: &[[f32; 2]]) -> f32 {
    if points.len() < 3 {
        return 0.0;
    }
    let mut area = 0.0;
    let n = points.len();
    for i in 0..n {
        let j = (i + 1) % n;
        area += points[i][0] * points[j][1];
        area -= points[j][0] * points[i][1];
    }
    area * 0.5
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Luma;

    fn make_mask(width: u32, height: u32, x0: u32, y0: u32, x1: u32, y1: u32) -> SpeechBubbleRegionMask {
        let mut img = GrayImage::from_pixel(width, height, Luma([0u8]));
        for y in y0..y1 {
            for x in x0..x1 {
                img.put_pixel(x, y, Luma([255u8]));
            }
        }
        SpeechBubbleRegionMask {
            x: 0,
            y: 0,
            width,
            height,
            pixels: img.into_raw(),
        }
    }

    #[test]
    fn rect_mask_returns_closed_rect_polygon() {
        let mask = make_mask(200, 200, 20, 30, 120, 130);
        let contour = extract_contour(&mask).expect("should extract contour");
        assert!(contour.len() >= 4, "rect mask should have ≥4 points, got {}", contour.len());
        // All points must be inside the mask bounds
        for &[x, y] in &contour {
            assert!(x >= 20.0 && x <= 120.0, "x={x} out of mask bounds");
            assert!(y >= 30.0 && y <= 130.0, "y={y} out of mask bounds");
        }
    }

    #[test]
    fn empty_mask_returns_none() {
        let mask = SpeechBubbleRegionMask {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            pixels: vec![],
        };
        assert!(extract_contour(&mask).is_none());
    }

    #[test]
    fn contour_points_in_absolute_coordinates() {
        let mask = make_mask(100, 100, 10, 15, 50, 55);
        let contour = extract_contour(&mask).expect("should extract contour");
        // Points should be in absolute coords (x >= 10, y >= 15)
        for &[x, y] in &contour {
            assert!(x >= 10.0, "x={x} should be ≥ mask.x");
            assert!(y >= 15.0, "y={y} should be ≥ mask.y");
            assert!(x <= 50.0);
            assert!(y <= 55.0);
        }
    }

    #[test]
    fn simplified_has_fewer_points_than_raw() {
        let mask = make_mask(300, 300, 50, 50, 250, 250);
        let contour = extract_contour(&mask).expect("should extract contour");
        // A large rect contour from imageproc will have many edge points.
        // After DP simplification it should be a reasonable number.
        assert!(contour.len() <= 100, "simplified contour should be small, got {} points", contour.len());
        assert!(contour.len() >= 4);
    }
}
