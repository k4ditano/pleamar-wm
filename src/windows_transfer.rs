//! Preserve logical size and relative placement when a scene sends a window.
use super::*;

pub(super) fn geometry(current:&Bounds,from:&Monitor,to:&Monitor,budget:u64) -> Result<Bounds> {
    for monitor in [from,to] {
        if !monitor.scale.is_finite() || monitor.scale<=0.0 || !monitor.bounds.contains(&monitor.work) {
            return Err("invalid monitor geometry or scale".into());
        }
    }
    if !from.bounds.contains(current) { return Err("bring the window wholly onto its source monitor before sending it".into()); }
    let scale=to.scale/from.scale;
    let dimension=|old:i32,available:i32| -> Result<i32> {
        let wanted=(f64::from(old)*scale).round();
        if !wanted.is_finite() || wanted<1.0 { return Err("invalid scaled window dimension".into()); }
        // A lower resolution destination must leave the whole window reachable.
        let pixels=wanted.min(f64::from(available));
        if pixels>8192.0 { return Err("sent window exceeds capture limits".into()); }
        Ok(pixels as i32)
    };
    let width=dimension(current.width,to.work.width)?;
    let height=dimension(current.height,to.work.height)?;
    if width as u64*height as u64>budget { return Err("sent window exceeds the shared capture budget".into()); }
    let position=|at:i32,old:i32,origin:i32,room:i32,dest:i32,space:i32,size:i32| -> Result<i32> {
        let fraction=if room>old { ((i64::from(at)-i64::from(origin)) as f64/f64::from(room-old)).clamp(0.0,1.0) } else { 0.5 };
        let offset=(fraction*f64::from(space-size)).round() as i64;
        Ok(i32::try_from(i64::from(dest)+offset)?)
    };
    let x=position(current.x,current.width,from.work.x,from.work.width,to.work.x,to.work.width,width)?;
    let y=position(current.y,current.height,from.work.y,from.work.height,to.work.y,to.work.height,height)?;
    let bounds=Bounds {x,y,width,height};
    if i64::from(x)+i64::from(width)>i64::from(i32::MAX) || i64::from(y)+i64::from(height)>i64::from(i32::MAX)
        || !to.work.contains(&bounds) { return Err("sent window bounds overflow the destination".into()); }
    Ok(bounds)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn monitor(name:&str,x:i32,y:i32,w:i32,h:i32,scale:f64) -> Monitor {
        Monitor {name:name.into(),bounds:Bounds{x,y,width:w,height:h},work:Bounds{x,y,width:w,height:h-40},scale,primary:false,refresh_hz:60}
    }
    #[test]
    fn movement_preserves_logical_size_and_relative_position_across_dpi() {
        let from=monitor("2k",0,0,2560,1440,1.5);
        let to=monitor("1080p",-1920,200,1920,1080,1.0);
        let current=Bounds{x:1760,y:800,width:800,height:600};
        assert_eq!(geometry(&current,&from,&to,16_777_216).unwrap(),Bounds{x:-533,y:840,width:533,height:400});
        let centered=Bounds{x:800,y:300,width:960,height:800};
        assert_eq!(geometry(&centered,&from,&to,16_777_216).unwrap(),Bounds{x:-1280,y:454,width:640,height:533});
        let moved=geometry(&centered,&from,&to,16_777_216).unwrap();
        let back=geometry(&moved,&to,&from,16_777_216).unwrap();
        assert!((back.width-centered.width).abs()<=1 && (back.height-centered.height).abs()<=1);
        assert!((back.x-centered.x).abs()<=1 && (back.y-centered.y).abs()<=1);
    }
    #[test]
    fn lower_resolution_never_places_a_window_outside_the_work_area() {
        let from=monitor("large",-3000,-1400,2560,1440,1.0);
        let to=monitor("small",0,0,1280,720,2.0);
        let current=Bounds{x:-3000,y:-1400,width:2500,height:1400};
        assert_eq!(geometry(&current,&from,&to,16_777_216).unwrap(),to.work);
        assert!(geometry(&current,&from,&to,1280*680-1).is_err());
    }
    #[test]
    fn stale_and_invalid_geometry_is_refused_before_native_placement() {
        let from=monitor("source",0,0,2560,1440,1.0);
        let to=monitor("destination",-1920,0,1920,1080,1.25);
        for current in [Bounds{x:-1,y:0,width:800,height:600},Bounds{x:0,y:0,width:0,height:600},
            Bounds{x:i32::MAX,y:0,width:800,height:600}] {
            assert!(geometry(&current,&from,&to,16_777_216).is_err());
        }
        let current=Bounds{x:0,y:0,width:800,height:600};
        for scale in [0.0,-1.0,f64::NAN,f64::INFINITY] {
            let mut invalid=to.clone();invalid.scale=scale;
            assert!(geometry(&current,&from,&invalid,16_777_216).is_err());
        }
        let mut invalid=to.clone();invalid.work.width=0;
        assert!(geometry(&current,&from,&invalid,16_777_216).is_err());
        let overflow=monitor("overflow",i32::MAX-10,0,1920,1080,1.0);
        assert!(geometry(&current,&from,&overflow,16_777_216).is_err());
    }
}
