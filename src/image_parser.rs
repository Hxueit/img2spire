use image::{GrayImage, Luma, RgbaImage};
use imageproc::edges::canny;

/// 少于该点数的笔画视为噪点丢弃
const MIN_STROKE_POINTS: usize = 3;
/// 解析前把图片最长边缩放到不超过该值
const MAX_IMAGE_DIM: u32 = 1024;

type Point2D = (f32, f32);
type Trajectory = Vec<Point2D>;

/// 读取图片：透明像素先合成到白底，再转灰度并按需缩小。
pub fn load_gray(path: &str) -> Result<GrayImage, String> {
    let img = image::open(path).map_err(|e| format!("无法加载图片: {e}"))?;
    let gray = flatten_to_gray(&img.into_rgba8());
    Ok(downscale(gray, MAX_IMAGE_DIM))
}

/// 将 RGBA 合成到白色背景后转为灰度，避免透明区域里的“隐藏”颜色产生伪边缘
pub fn flatten_to_gray(rgba: &RgbaImage) -> GrayImage {
    GrayImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let [r, g, b, a] = rgba.get_pixel(x, y).0;
        let alpha = a as f32 / 255.0;
        let blend = |c: u8| c as f32 * alpha + 255.0 * (1.0 - alpha);
        let luma = 0.2126 * blend(r) + 0.7152 * blend(g) + 0.0722 * blend(b);
        Luma([luma.round().clamp(0.0, 255.0) as u8])
    })
}

fn downscale(gray: GrayImage, max_dim: u32) -> GrayImage {
    if gray.width() <= max_dim && gray.height() <= max_dim {
        return gray;
    }
    let scale = (max_dim as f32 / gray.width() as f32).min(max_dim as f32 / gray.height() as f32);
    image::imageops::resize(
        &gray,
        ((gray.width() as f32 * scale) as u32).max(1),
        ((gray.height() as f32 * scale) as u32).max(1),
        image::imageops::FilterType::Triangle,
    )
}

/// 边缘检测后把边缘像素追踪成单向折线（每段边缘只经过一次）。
pub fn parse_image_to_lines(
    image: &GrayImage,
    low_thresh: f32,
    high_thresh: f32,
) -> Vec<Trajectory> {
    let lower = low_thresh.min(high_thresh);
    let upper = low_thresh.max(high_thresh);
    let mut edges = canny(image, lower, upper);
    thin_edges(&mut edges);
    trace_edges(&edges)
}

/// Zhang-Suen 细化：把边缘压成单像素宽。
///
/// 锐利的阶跃边缘两侧梯度相同，Canny 的非极大值抑制会保留两侧像素，
/// 得到 2 像素宽的边缘；不细化的话同一条边会被来回画两遍。
pub fn thin_edges(edges: &mut GrayImage) {
    let (w, h) = (edges.width() as i32, edges.height() as i32);
    let mut on: Vec<bool> = edges.pixels().map(|p| p.0[0] > 0).collect();
    let at = |on: &[bool], x: i32, y: i32| {
        x >= 0 && y >= 0 && x < w && y < h && on[(y * w + x) as usize]
    };
    // P2..P9：从正上方开始顺时针
    const RING: [(i32, i32); 8] = [
        (0, -1),
        (1, -1),
        (1, 0),
        (1, 1),
        (0, 1),
        (-1, 1),
        (-1, 0),
        (-1, -1),
    ];

    let mut to_remove = Vec::new();
    loop {
        let mut changed = false;
        for step in 0..2 {
            to_remove.clear();
            for y in 0..h {
                for x in 0..w {
                    if !on[(y * w + x) as usize] {
                        continue;
                    }
                    let p: [bool; 8] = RING.map(|(dx, dy)| at(&on, x + dx, y + dy));
                    let b = p.iter().filter(|&&v| v).count();
                    let a = (0..8).filter(|&i| !p[i] && p[(i + 1) % 8]).count();
                    let [p2, _, p4, _, p6, _, p8, _] = p;
                    let cond = if step == 0 {
                        !(p4 && p6 && (p2 || p8))
                    } else {
                        !(p2 && p8 && (p4 || p6))
                    };
                    if (2..=6).contains(&b) && a == 1 && cond {
                        to_remove.push((y * w + x) as usize);
                    }
                }
            }
            for &i in &to_remove {
                on[i] = false;
            }
            changed |= !to_remove.is_empty();
        }
        if !changed {
            break;
        }
    }

    for (pixel, &v) in edges.pixels_mut().zip(&on) {
        pixel.0[0] = if v { 255 } else { 0 };
    }
}

/// 8 邻域偏移：先 4 邻域再对角，保证阶梯状的边缘不会被对角线“跳过”
const NEIGHBORS: [(i32, i32); 8] = [
    (1, 0),
    (0, 1),
    (-1, 0),
    (0, -1),
    (1, 1),
    (-1, 1),
    (-1, -1),
    (1, -1),
];

/// 追踪二值边缘图（非零即边缘）中的像素链。
///
/// `find_contours` 追踪的是区域的边界，对 1 像素宽的线会“去一遍回一遍”，
/// 闭合的线条还会同时产生外轮廓和内轮廓，导致每条线被画两遍。
/// 这里改为直接沿边缘像素行走：先从端点出发追踪开放线段，再处理剩余的闭环；
/// 线段末端若紧挨着已画过的像素（分叉点或闭环起点）则连上它，避免留下缺口。
pub fn trace_edges(edges: &GrayImage) -> Vec<Trajectory> {
    let (w, h) = (edges.width() as i32, edges.height() as i32);
    let idx = |x: i32, y: i32| (y * w + x) as usize;
    let on: Vec<bool> = edges.pixels().map(|p| p.0[0] > 0).collect();
    // 每个像素所属的笔画编号，0 表示尚未访问
    let mut owner = vec![0u32; on.len()];

    let neighbors = |x: i32, y: i32| {
        NEIGHBORS.iter().filter_map(move |&(dx, dy)| {
            let (nx, ny) = (x + dx, y + dy);
            (nx >= 0 && ny >= 0 && nx < w && ny < h).then_some((nx, ny))
        })
    };
    let degree = |x: i32, y: i32| neighbors(x, y).filter(|&(nx, ny)| on[idx(nx, ny)]).count();

    let mut strokes = Vec::new();
    let mut next_id = 1u32;

    // 沿一个方向走到头，返回走过的像素（不含起点）
    let walk = |start: (i32, i32), id: u32, owner: &mut Vec<u32>| {
        let mut path = Vec::new();
        let mut cur = start;
        loop {
            let next = neighbors(cur.0, cur.1)
                .find(|&(nx, ny)| on[idx(nx, ny)] && owner[idx(nx, ny)] == 0);
            match next {
                Some(n) => {
                    owner[idx(n.0, n.1)] = id;
                    path.push(n);
                    cur = n;
                }
                None => break,
            }
        }
        path
    };

    // 末端若挨着别的笔画或本笔画起点，则补上这一点把线连起来
    let cap = |path: &[(i32, i32)], id: u32, owner: &[u32]| -> Option<(i32, i32)> {
        let (&end, rest) = path.split_last()?;
        let prev = rest.last().copied();
        let start = path[0];
        neighbors(end.0, end.1).find(|&n| {
            let o = owner[idx(n.0, n.1)];
            on[idx(n.0, n.1)]
                && Some(n) != prev
                && ((o != 0 && o != id) || (n == start && path.len() >= 3))
        })
    };

    let mut trace_from = |start: (i32, i32), owner: &mut Vec<u32>| {
        let id = next_id;
        next_id += 1;
        owner[idx(start.0, start.1)] = id;
        let forward = walk(start, id, owner);
        let backward = walk(start, id, owner);

        let mut path: Vec<(i32, i32)> = backward.into_iter().rev().collect();
        path.push(start);
        path.extend(forward);

        if let Some(p) = cap(&path, id, owner) {
            path.push(p);
        }
        path.reverse();
        if let Some(p) = cap(&path, id, owner) {
            path.push(p);
        }
        path
    };

    // 第一遍：从端点出发；第二遍：剩下的都是闭环或环上的分支
    for pass in 0..2 {
        for y in 0..h {
            for x in 0..w {
                let i = idx(x, y);
                if !on[i] || owner[i] != 0 || (pass == 0 && degree(x, y) != 1) {
                    continue;
                }
                let path = trace_from((x, y), &mut owner);
                if path.len() >= MIN_STROKE_POINTS {
                    strokes.push(
                        path.into_iter()
                            .map(|(x, y)| (x as f32, y as f32))
                            .collect(),
                    );
                }
            }
        }
    }

    strokes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image_from(pixels: &[(u32, u32)], w: u32, h: u32) -> GrayImage {
        let mut img = GrayImage::new(w, h);
        for &(x, y) in pixels {
            img.put_pixel(x, y, Luma([255]));
        }
        img
    }

    #[test]
    fn open_line_is_traced_once() {
        let pixels: Vec<_> = (5..25).map(|x| (x, 5)).collect();
        let strokes = trace_edges(&image_from(&pixels, 40, 10));
        assert_eq!(strokes.len(), 1);
        assert_eq!(strokes[0].len(), 20);
        let xs: Vec<f32> = strokes[0].iter().map(|p| p.0).collect();
        let monotonic = xs.windows(2).all(|w| w[1] > w[0]) || xs.windows(2).all(|w| w[1] < w[0]);
        assert!(monotonic, "line should not double back: {xs:?}");
    }

    #[test]
    fn ring_is_traced_once_and_closed() {
        let mut pixels = Vec::new();
        for i in 5..15 {
            pixels.extend([(i, 5), (i, 14), (5, i), (14, i)]);
        }
        pixels.sort();
        pixels.dedup();
        let strokes = trace_edges(&image_from(&pixels, 20, 20));
        assert_eq!(strokes.len(), 1);
        // 36 个像素 + 闭合回起点的 1 个点
        assert_eq!(strokes[0].len(), pixels.len() + 1);
        assert_eq!(strokes[0].first(), strokes[0].last());
    }

    #[test]
    fn staircase_diagonal_keeps_every_pixel() {
        let pixels = [(0, 0), (1, 0), (1, 1), (2, 1), (2, 2), (3, 2)];
        let strokes = trace_edges(&image_from(&pixels, 5, 5));
        assert_eq!(strokes.len(), 1);
        assert_eq!(strokes[0].len(), pixels.len());
    }

    #[test]
    fn branch_connects_to_junction() {
        // 横线 + 从中间向下伸出的分支 (T 形)
        let mut pixels: Vec<_> = (0..11).map(|x| (x, 0)).collect();
        pixels.extend((1..6).map(|y| (5, y)));
        let strokes = trace_edges(&image_from(&pixels, 12, 8));
        assert_eq!(strokes.len(), 2);
        let total: usize = strokes.iter().map(|s| s.len()).sum();
        // 分支会多出一个连接到交叉点的点
        assert_eq!(total, pixels.len() + 1);
    }

    #[test]
    fn traces_far_fewer_points_than_find_contours() {
        // 白底上的黑色方块和圆，经过真实的 Canny 边缘检测
        let img = GrayImage::from_fn(200, 200, |x, y| {
            let (dx, dy) = (x as f32 - 140.0, y as f32 - 140.0);
            let in_rect = (20..90).contains(&x) && (20..90).contains(&y);
            let in_circle = dx * dx + dy * dy < 40.0 * 40.0;
            Luma([if in_rect || in_circle { 0 } else { 255 }])
        });
        let edges = canny(&img, 50.0, 150.0);
        let old: usize = imageproc::contours::find_contours::<u32>(&edges)
            .iter()
            .map(|c| c.points.len())
            .sum();

        let mut thinned = edges.clone();
        thin_edges(&mut thinned);
        let skeleton_pixels = thinned.pixels().filter(|p| p.0[0] > 0).count();
        let new: usize = trace_edges(&thinned).iter().map(|s| s.len()).sum();

        // 细化后每个像素只经过一次（外加少量闭合/连接点），而旧方法约为两倍以上
        assert!(
            new >= skeleton_pixels && new <= skeleton_pixels + skeleton_pixels / 20,
            "new={new} skeleton_pixels={skeleton_pixels}"
        );
        // 方块周长 4*70 + 圆周长 2π*40 ≈ 531，新方法应接近这个“每条边只画一遍”的下限
        let ideal = 4.0 * 70.0 + 2.0 * std::f32::consts::PI * 40.0;
        assert!((new as f32) < ideal * 1.1, "new={new} ideal={ideal}");
        assert!(old as f32 > new as f32 * 1.6, "old={old} new={new}");
    }

    #[test]
    fn thinning_keeps_one_pixel_lines() {
        let pixels: Vec<_> = (5..25).map(|x| (x, 5)).collect();
        let mut img = image_from(&pixels, 40, 10);
        thin_edges(&mut img);
        assert_eq!(img.pixels().filter(|p| p.0[0] > 0).count(), pixels.len());
    }

    #[test]
    fn thinning_reduces_two_pixel_band() {
        let pixels: Vec<_> = (5..25).flat_map(|x| [(x, 5), (x, 6)]).collect();
        let mut img = image_from(&pixels, 40, 12);
        thin_edges(&mut img);
        let strokes = trace_edges(&img);
        assert_eq!(strokes.len(), 1);
        assert!(strokes[0].len() <= 20, "len={}", strokes[0].len());
    }

    #[test]
    fn transparent_pixels_become_white() {
        let mut rgba = RgbaImage::new(2, 1);
        rgba.put_pixel(0, 0, image::Rgba([0, 0, 0, 0]));
        rgba.put_pixel(1, 0, image::Rgba([0, 0, 0, 255]));
        let gray = flatten_to_gray(&rgba);
        assert_eq!(gray.get_pixel(0, 0).0[0], 255);
        assert_eq!(gray.get_pixel(1, 0).0[0], 0);
    }
}
