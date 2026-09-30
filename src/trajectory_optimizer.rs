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

/// 贪心的最近邻排序：每次选离当前笔尖最近的笔画（必要时反向绘制），
/// 以缩短抬笔空走的距离。
pub fn order_strokes(strokes: Vec<Trajectory>, start: Point2D) -> Vec<Trajectory> {
    let mut remaining: Vec<Option<Trajectory>> = strokes.into_iter().map(Some).collect();
    let mut ordered = Vec::with_capacity(remaining.len());
    let mut pen = start;

    while ordered.len() < remaining.len() {
        let mut best: Option<(usize, bool, f32)> = None;
        for (i, stroke) in remaining.iter().enumerate() {
            let Some(stroke) = stroke else { continue };
            let (Some(&first), Some(&last)) = (stroke.first(), stroke.last()) else {
                continue;
            };
            for (reversed, p) in [(false, first), (true, last)] {
                let d = dist_sq(pen, p);
                if best.is_none_or(|(_, _, bd)| d < bd) {
                    best = Some((i, reversed, d));
                }
            }
        }
        let Some((i, reversed, _)) = best else { break };
        let mut stroke = remaining[i].take().unwrap_or_default();
        if reversed {
            stroke.reverse();
        }
        pen = stroke.last().copied().unwrap_or(pen);
        ordered.push(stroke);
    }

    ordered
}

#[inline(always)]
fn dist_sq(a: Point2D, b: Point2D) -> f32 {
    (a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)
}

#[inline(always)]
fn point_line_distance_sq(p: Point2D, a: Point2D, b: Point2D) -> f32 {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    if dx == 0.0 && dy == 0.0 {
        return dist_sq(p, a);
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

        for (i, &p) in points.iter().enumerate().take(end).skip(start + 1) {
            let dist_sq = point_line_distance_sq(p, p_start, p_end);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simplify_collapses_straight_line() {
        let line: Trajectory = (0..10).map(|x| (x as f32, 0.0)).collect();
        assert_eq!(
            optimize_trajectories(&[line], 1.0),
            vec![vec![(0.0, 0.0), (9.0, 0.0)]]
        );
    }

    #[test]
    fn simplify_keeps_corner() {
        let mut line: Trajectory = (0..=5).map(|x| (x as f32, 0.0)).collect();
        line.extend((1..=5).map(|y| (5.0, y as f32)));
        let out = optimize_trajectories(&[line], 0.5);
        assert_eq!(out[0], vec![(0.0, 0.0), (5.0, 0.0), (5.0, 5.0)]);
    }

    #[test]
    fn order_picks_nearest_and_reverses() {
        let far = vec![(100.0, 0.0), (110.0, 0.0)];
        let near_backwards = vec![(20.0, 0.0), (10.0, 0.0)];
        let ordered = order_strokes(vec![far.clone(), near_backwards], (0.0, 0.0));
        assert_eq!(ordered[0], vec![(10.0, 0.0), (20.0, 0.0)]);
        assert_eq!(ordered[1], far);
    }
}
