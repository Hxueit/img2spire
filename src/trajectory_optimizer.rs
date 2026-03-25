type Point2D = (f32, f32);
type Trajectory = Vec<Point2D>;

pub fn optimize_trajectories(trajectories: &[Trajectory], tolerance: f32) -> Vec<Trajectory> {
    let tol_sq = tolerance * tolerance;
    trajectories
        .iter()
        .map(|t| simplify_dp_iterative(t, tol_sq))
        .filter(|t| t.len() >= 2)
        .collect()
}

#[inline(always)]
fn point_line_distance_sq(p: Point2D, a: Point2D, b: Point2D) -> f32 {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    if dx == 0.0 && dy == 0.0 {
        return (p.0 - a.0).powi(2) + (p.1 - a.1).powi(2);
    }
    let num = dy * p.0 - dx * p.1 + b.0 * a.1 - b.1 * a.0;
    (num * num) / (dx * dx + dy * dy)
}

fn simplify_dp_iterative(points: &[Point2D], tol_sq: f32) -> Trajectory {
    let len = points.len();
    if len < 3 {
        return points.to_vec();
    }

    let mut keep = vec![false; len];
    keep[0] = true;
    keep[len - 1] = true;

    let mut stack = Vec::with_capacity(32);
    stack.push((0, len - 1));

    while let Some((start, end)) = stack.pop() {
        if end <= start + 1 {
            continue;
        }

        let mut max_dist_sq = 0.0;
        let mut index = start;
        let p_start = points[start];
        let p_end = points[end];

        for i in (start + 1)..end {
            let dist_sq = point_line_distance_sq(points[i], p_start, p_end);
            if dist_sq > max_dist_sq {
                max_dist_sq = dist_sq;
                index = i;
            }
        }

        if max_dist_sq > tol_sq {
            keep[index] = true;
            stack.push((start, index));
            stack.push((index, end));
        }
    }

    points
        .iter()
        .enumerate()
        .filter_map(|(i, &p)| if keep[i] { Some(p) } else { None })
        .collect()
}
