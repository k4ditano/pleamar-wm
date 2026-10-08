//! Physical-pixel layouts; platform backends supply a monitor's work area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect { pub x: i32, pub y: i32, pub width: i32, pub height: i32 }

#[derive(Clone, Copy, Debug)]
pub enum Layout { Left, Right, Columns, Rows, Grid }

impl std::str::FromStr for Layout {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "left" => Ok(Self::Left), "right" => Ok(Self::Right),
            "columns" => Ok(Self::Columns), "rows" => Ok(Self::Rows), "grid" => Ok(Self::Grid),
            _ => Err("layout must be left, right, columns, rows or grid".into()),
        }
    }
}

fn split(start: i32, length: i32, count: usize, gap: i32) -> Result<Vec<(i32, i32)>, String> {
    let count = count as i64;
    let available = i64::from(length) - i64::from(gap) * (count - 1);
    if available < count { return Err("the work area is too small for this layout and gap".into()); }
    Ok((0..count).map(|i| {
        let begin = available * i / count;
        let end = available * (i + 1) / count;
        ((i64::from(start) + begin + i * i64::from(gap)) as i32, (end - begin) as i32)
    }).collect())
}

pub fn arrange(area: Rect, count: usize, layout: Layout, gap: i32) -> Result<Vec<Rect>, String> {
    if !(1..=64).contains(&count) || gap < 0 || gap > 256 || area.width <= 0 || area.height <= 0
        || area.x.checked_add(area.width).is_none() || area.y.checked_add(area.height).is_none() {
        return Err("invalid work area, window count or gap".into());
    }
    if area.width <= 2 * gap || area.height <= 2 * gap { return Err("the gap consumes the work area".into()); }
    let area = Rect { x: area.x + gap, y: area.y + gap,
        width: area.width - 2 * gap, height: area.height - 2 * gap };
    if count == 1 { return Ok(vec![area]); }
    match layout {
        Layout::Columns => split(area.x, area.width, count, gap).map(|parts|
            parts.into_iter().map(|(x, width)| Rect { x, width, ..area }).collect()),
        Layout::Rows => split(area.y, area.height, count, gap).map(|parts|
            parts.into_iter().map(|(y, height)| Rect { y, height, ..area }).collect()),
        Layout::Left | Layout::Right => {
            let parts = split(area.x, area.width, 2, gap)?;
            let lead = if matches!(layout, Layout::Left) { 0 } else { 1 };
            let mut result = vec![Rect { x: parts[lead].0, width: parts[lead].1, ..area }];
            for (y, height) in split(area.y, area.height, count - 1, gap)? {
                result.push(Rect { x: parts[1 - lead].0, width: parts[1 - lead].1, y, height });
            }
            Ok(result)
        }
        Layout::Grid => {
            let columns = (count as f64).sqrt().ceil() as usize;
            let rows = count.div_ceil(columns);
            let mut result = Vec::with_capacity(count);
            for (row, (y, height)) in split(area.y, area.height, rows, gap)?.into_iter().enumerate() {
                let cells = (count - row * columns).min(columns);
                for (x, width) in split(area.x, area.width, cells, gap)? {
                    result.push(Rect { x, y, width, height });
                }
            }
            Ok(result)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn layouts_stay_in_each_monitors_work_area_and_do_not_overlap() {
        for area in [Rect { x: -1920, y: -200, width: 1920, height: 1040 },
            Rect { x: 0, y: 0, width: 2560, height: 1392 },
            Rect { x: 2560, y: -1080, width: 1080, height: 1920 }] {
            for count in 1..=16 {
                for gap in [0, 8, 12, 16] {
                    for layout in [Layout::Left, Layout::Right, Layout::Columns, Layout::Rows, Layout::Grid] {
                        let boxes = arrange(area, count, layout, gap).unwrap();
                        assert_eq!(boxes.len(), count);
                        for (i, a) in boxes.iter().enumerate() {
                            assert!(a.x >= area.x + gap && a.y >= area.y + gap);
                            assert!(a.x + a.width <= area.x + area.width - gap);
                            assert!(a.y + a.height <= area.y + area.height - gap);
                            assert!(a.width > 0 && a.height > 0);
                            for b in &boxes[..i] {
                                assert!(a.x + a.width <= b.x || b.x + b.width <= a.x
                                    || a.y + a.height <= b.y || b.y + b.height <= a.y);
                            }
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn distributes_rounding_without_losing_the_last_pixel() {
        let boxes = arrange(Rect { x: -1919, y: 0, width: 1919, height: 1080 }, 3, Layout::Columns, 8).unwrap();
        assert_eq!(boxes[2].x + boxes[2].width, -8);
        assert_eq!(boxes.iter().map(|r| r.width).sum::<i32>() + 32, 1919);
    }
    #[test]
    fn rejects_degenerate_inputs() {
        let area = Rect { x: 0, y: 0, width: 10, height: 10 };
        for count in [0, 65, usize::MAX] { assert!(arrange(area, count, Layout::Grid, 0).is_err()); }
        for gap in [-1, 5, 257, i32::MAX] { assert!(arrange(area, 2, Layout::Grid, gap).is_err()); }
        assert!(arrange(Rect { x: i32::MAX, ..area }, 1, Layout::Grid, 0).is_err());
    }
}
