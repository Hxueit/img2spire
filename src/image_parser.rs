use image::GrayImage;
use imageproc::contours::find_contours;
use imageproc::edges::canny;

const MIN_CONTOUR_POINTS: usize = 3;

type Point2D = (f32, f32);
type Trajectory = Vec<Point2D>;

/// Detects edges and extracts continuous contours as coordinate trajectories.
pub fn parse_image_to_lines(
    image: &GrayImage,
    low_thresh: f32,
    high_thresh: f32,
) -> Vec<Trajectory> {
    let lower = low_thresh.min(high_thresh);
    let upper = low_thresh.max(high_thresh);
    let edges = canny(image, lower, upper);

    find_contours::<u32>(&edges)
        .into_iter()
        .filter_map(|contour| {
            if contour.points.len() < MIN_CONTOUR_POINTS {
                None
            } else {
                Some(
                    contour
                        .points
                        .into_iter()
                        .map(|p| (p.x as f32, p.y as f32))
                        .collect(),
                )
            }
        })
        .collect()
}
