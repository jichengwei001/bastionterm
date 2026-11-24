//! Geometry and visibility of the terminal's vertical scrollbar.
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Metrics {
    pub height: f32,
    pub thumb: f32,
    pub top: f32,
    history: usize,
    view_start: usize,
}

impl Metrics {
    pub fn new(rows: u16, row_height: f32, history: usize, offset: usize) -> Option<Self> {
        if history == 0 {
            return None;
        }
        let height = (f32::from(rows) * row_height).round();
        let thumb = (height * f32::from(rows) / (f32::from(rows) + history as f32))
            .floor()
            .max(20.)
            .min(height);
        let top =
            ((history.saturating_sub(offset) as f32 / history as f32) * (height - thumb)).round();
        Some(Self {
            height,
            thumb,
            top,
            history,
            view_start: history.saturating_sub(offset),
        })
    }

    pub fn view_start(self, thumb_top: f32) -> usize {
        let travel = (self.height - self.thumb).max(1.);
        ((thumb_top / travel).clamp(0., 1.) * self.history as f32).round() as usize
    }

    pub fn grab(self, x: f32, y: f32, track_y: f32) -> Drag {
        let local = y - track_y;
        let on_thumb = (self.top..self.top + self.thumb).contains(&local);
        let top = if on_thumb {
            self.top
        } else {
            (local - self.thumb / 2.).clamp(0., self.height - self.thumb)
        };
        Drag {
            metrics: self,
            x,
            y,
            top,
            start: if on_thumb {
                self.view_start
            } else {
                self.view_start(top)
            },
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Drag {
    metrics: Metrics,
    x: f32,
    y: f32,
    top: f32,
    start: usize,
}

impl Drag {
    pub fn view_start(self, x: f32, y: f32) -> usize {
        // The Windows reference restores the grab position when the pointer strays sideways.
        let delta = if cfg!(windows) && (x - self.x).abs() > 140. {
            0.
        } else {
            y - self.y
        };
        if delta == 0. {
            self.start
        } else {
            self.metrics.view_start(self.top + delta)
        }
    }
}

#[derive(Default)]
pub(crate) struct Scrollbar {
    pub drag: Option<Drag>,
    pub over_terminal: bool,
    pub over_thumb: bool,
    hide_after: Option<Instant>,
    last_position: Option<(usize, usize, u16)>,
}

impl Scrollbar {
    pub fn hover(&mut self, over: bool) {
        self.over_terminal = over;
        if !over {
            self.over_thumb = false;
        }
        self.hide_after = Some(Instant::now());
    }

    pub fn opacity(&mut self, history: usize, offset: usize, rows: u16) -> (f32, bool) {
        let now = Instant::now();
        let position = Some((history, offset, rows));
        if self.last_position != position {
            self.last_position = position;
            self.hide_after = Some(now + Duration::from_millis(500));
        }
        if self.over_terminal || self.drag.is_some() {
            return (1., false);
        }
        let Some(deadline) = self.hide_after else {
            return (0., false);
        };
        let elapsed = now.saturating_duration_since(deadline).as_secs_f32();
        let opacity = (1. - elapsed / 0.8).max(0.);
        (opacity, opacity > 0.)
    }
}

#[cfg(test)]
mod tests {
    use super::Metrics;

    #[test]
    fn xterm_geometry_and_pointer_grab() {
        assert!(Metrics::new(35, 19., 0, 0).is_none());
        let m = Metrics::new(35, 19., 424, 0).unwrap();
        assert_eq!((m.height, m.thumb, m.top), (665., 50., 615.));
        let drag = m.grab(0., 750., 119.);
        assert_eq!(drag.view_start(0., 750.), 424);
        assert_eq!(drag.view_start(0., 550.), 286);
        assert_eq!(m.grab(0., 119., 119.).view_start(0., 119.), 0);
        assert_eq!(m.grab(0., 450., 119.).view_start(0., 450.), 211);
        assert_eq!(Metrics::new(35, 19., 10000, 10000).unwrap().thumb, 20.);
    }
}
