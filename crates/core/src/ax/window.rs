//! Where an app drawn in a window of its own lies on the screen.
//!
//! A foldable's inner panel shows an app made for a smaller iPhone screen in a scaled window
//! (compatibility mode). The app's accessibility frames are in its own screen's points, and the
//! dump says nothing about the window; a screenshot does: the window is lit, the margins around it
//! black (PITFALLS #70).

use super::tree::Rect;

/// A pixel counts as lit when a channel is above this. The margins are pure black.
const DARK: u8 = 12;
/// Share of the sampled lines that must be lit inside an edge (and dark outside it) for the edge
/// to count as found. Rounded corners and dark content lower it.
const EDGE_SHARE: f64 = 0.5;
/// Slack below this, in pixels, is no slack: the window fills that axis.
const NO_SLACK: f64 = 2.0;

/// An 8-bit RGB image: the screen as a screenshot shows it, in the interface's orientation.
pub struct Image<'a> {
    pub width: usize,
    pub height: usize,
    /// `width * height * 3` bytes, rows top to bottom.
    pub rgb: &'a [u8],
}

impl Image<'_> {
    fn lit(&self, x: usize, y: usize) -> bool {
        let i = (y * self.width + x) * 3;
        self.rgb[i..i + 3].iter().any(|&c| c > DARK)
    }
}

/// The window an app of `app` points is drawn in on a `screen`-point screen: as large as the
/// screen allows at the app's aspect ratio. When that leaves slack along an axis, the window's
/// place along it is found in `image`: there were 46 points to its left and 88 to its right, where
/// the system's own controls are (PITFALLS #70). `None` when the screenshot doesn't show the
/// window's edges (an app dark at its edges, a screenshot of another size).
pub fn find(image: &Image, screen: (f64, f64), app: (f64, f64)) -> Option<Rect> {
    let ((sw, sh), (aw, ah)) = (screen, app);
    if [sw, sh, aw, ah].iter().any(|v| *v <= 0.0) || image.width == 0 || image.height == 0 {
        return None;
    }
    let ppt = image.width as f64 / sw;
    // A screenshot of the panel turned from the screen as the user sees it, or of another screen.
    if (image.height as f64 / sh - ppt).abs() > ppt * 0.02 {
        return None;
    }
    let k = (sw / aw).min(sh / ah);
    let (ww, wh) = (aw * k, ah * k);
    let place = |slack_pt: f64, along_x: bool| -> Option<f64> {
        let slack = slack_pt * ppt;
        if slack < NO_SLACK {
            return Some(slack_pt / 2.0);
        }
        let len = ((if along_x { ww } else { wh }) * ppt).round() as usize;
        let (span, across) = if along_x { (image.width, image.height) } else { (image.height, image.width) };
        // Lines across the middle 80 % of the other axis, clear of rounded corners.
        let samples = || (across / 10..across * 9 / 10).step_by(4);
        let sampled = samples().count().max(1);
        // How many sampled lines are lit at each position along the axis.
        let profile: Vec<usize> = (0..span)
            .map(|at| samples().filter(|&o| if along_x { image.lit(at, o) } else { image.lit(o, at) }).count())
            .collect();
        let at = |i: isize| -> usize { if i < 0 || i as usize >= span { 0 } else { profile[i as usize] } };
        let mut best: Option<(usize, isize)> = None;
        for start in 0..=(slack.round() as usize).min(span.saturating_sub(len)) {
            let (s, e) = (start as isize, (start + len) as isize);
            let score = at(s) as isize - at(s - 1) as isize + at(e - 1) as isize - at(e) as isize;
            if best.is_none_or(|(_, b)| score > b) {
                best = Some((start, score));
            }
        }
        let (start, score) = best?;
        (score as f64 >= EDGE_SHARE * 2.0 * sampled as f64).then_some(start as f64 / ppt)
    };
    let x = place(sw - ww, true)?;
    let y = place(sh - wh, false)?;
    let r = |v: f64| (v * 100.0).round() / 100.0;
    Some(Rect { x: r(x), y: r(y), w: r(ww), h: r(wh) })
}

/// Where a frame in the app's own points lies on the screen, for an app of `app` points drawn in
/// `window`.
pub fn place(frame: &Rect, app: (f64, f64), window: &Rect) -> Rect {
    let k = if app.0 > 0.0 { window.w / app.0 } else { 1.0 };
    let r = |v: f64| (v * 10.0).round() / 10.0;
    Rect { x: r(window.x + frame.x * k), y: r(window.y + frame.y * k), w: r(frame.w * k), h: r(frame.h * k) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A black screen with lit rectangles, one pixel per point.
    fn screen(w: usize, h: usize, lit: &[(usize, usize, usize, usize)]) -> Vec<u8> {
        let mut rgb = vec![0u8; w * h * 3];
        for &(x0, y0, x1, y1) in lit {
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = (y * w + x) * 3;
                    rgb[i..i + 3].copy_from_slice(&[240, 240, 240]);
                }
            }
        }
        rgb
    }

    #[test]
    fn finds_the_compat_window_beside_the_system_controls() {
        // The layout seen on a foldable's inner panel (669x951 points, iOS 27.3): an app of
        // 375x667 points drawn 534.7 points wide from x 46.3, the status column and a button at
        // x 600 to 646 (PITFALLS #70). Here at 3 pixels a point, the window from pixel 139.
        let (w, h) = (2007, 2853);
        let rgb = screen(w, h, &[(139, 0, 1743, 2853), (1801, 68, 1939, 312), (1801, 2649, 1939, 2792)]);
        let found = find(&Image { width: w, height: h, rgb: &rgb }, (669.0, 951.0), (375.0, 667.0)).unwrap();
        assert_eq!(found, Rect { x: 46.33, y: 0.0, w: 534.67, h: 951.0 });
        // A button of the app at {{20, 522.3}, {45, 24}} showed at about (75, 745) to (139, 779).
        let b = place(&Rect { x: 20.0, y: 522.3, w: 45.0, h: 24.0 }, (375.0, 667.0), &found);
        assert_eq!(b, Rect { x: 74.8, y: 744.7, w: 64.2, h: 34.2 });
    }

    #[test]
    fn a_window_dark_at_its_edges_is_not_found() {
        let (w, h) = (669, 951);
        // Content in the middle only, nothing at the window's edges.
        let rgb = screen(w, h, &[(200, 300, 400, 500)]);
        assert_eq!(find(&Image { width: w, height: h, rgb: &rgb }, (669.0, 951.0), (375.0, 667.0)), None);
    }

    #[test]
    fn a_window_that_fills_the_screen_needs_no_search() {
        let (w, h) = (100, 200);
        let rgb = screen(w, h, &[]);
        let found = find(&Image { width: w, height: h, rgb: &rgb }, (100.0, 200.0), (50.0, 100.0)).unwrap();
        assert_eq!(found, Rect { x: 0.0, y: 0.0, w: 100.0, h: 200.0 });
    }

    #[test]
    fn a_screenshot_turned_from_the_screen_is_refused() {
        let rgb = screen(951, 669, &[]);
        assert_eq!(find(&Image { width: 951, height: 669, rgb: &rgb }, (669.0, 951.0), (375.0, 667.0)), None);
    }
}
