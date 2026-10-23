//! What the agent's pictures need (`c TARGET X Y W H` on the agent's
//! socket): boxes in look coordinates, and what two of them have in common.

/// A box's corners, far as they may be.
fn corners(a: [i32; 4]) -> [i64; 4] {
    let a = a.map(i64::from);
    [a[0], a[1], a[0] + a[2], a[1] + a[3]]
}

/// The box between those corners, cut to what an `i32` holds.
fn from_corners(c: [i64; 4]) -> [i32; 4] {
    let n = |v: i64| v.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
    [n(c[0]), n(c[1]), n(c[2] - c[0]), n(c[3] - c[1])]
}

/// What two boxes have in common, if anything.
pub fn meet(a: [i32; 4], b: [i32; 4]) -> Option<[i32; 4]> {
    let (a, b) = (corners(a), corners(b));
    let (x0, y0, x1, y1) = (a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3]));
    (x1 > x0 && y1 > y0).then(|| from_corners([x0, y0, x1, y1]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boxes_met() {
        assert_eq!(meet([-5, -5, 20, 20], [0, 0, 100, 100]), Some([0, 0, 15, 15]));
        assert_eq!(meet([200, 0, 20, 20], [0, 0, 100, 100]), None);
        assert_eq!(meet([i32::MAX, i32::MAX, i32::MAX, i32::MAX], [0, 0, 100, 100]), None);
    }
}
