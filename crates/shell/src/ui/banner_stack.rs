use std::time::Duration;

use bkss_integration::Notification;

use super::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BannerPhase {
    Entering,
    Visible,
    Fading,
    Collapsing,
}

impl BannerPhase {
    pub fn duration(self) -> Duration {
        Duration::from_millis(match self {
            Self::Entering => 240,
            Self::Visible => 0,
            Self::Fading => 480,
            Self::Collapsing => 260,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BannerFrame {
    pub x: f32,
    pub opacity: f32,
    pub height: Option<f32>,
}

#[derive(Clone)]
pub struct Banner {
    pub notification: Notification,
    pub token: u64,
    pub phase: BannerPhase,
    pub natural_height: f32,
    from: BannerFrame,
    frame: BannerFrame,
}

impl Banner {
    pub fn live(&self) -> bool {
        matches!(self.phase, BannerPhase::Entering | BannerPhase::Visible)
    }

    // GPUI supplies eased progress and owns frame scheduling.
    pub fn sample(&self, progress: f32) -> BannerFrame {
        let progress = progress.clamp(0., 1.);
        match self.phase {
            BannerPhase::Entering => BannerFrame {
                x: self.from.x * (1. - progress),
                opacity: self.from.opacity + (1. - self.from.opacity) * progress,
                height: self
                    .from
                    .height
                    .map(|height| height + (self.natural_height - height) * progress),
            },
            BannerPhase::Visible => BannerFrame {
                x: 0.,
                opacity: 1.,
                height: None,
            },
            BannerPhase::Fading => BannerFrame {
                x: self.from.x,
                opacity: self.from.opacity * (1. - progress),
                height: self
                    .from
                    .height
                    .or((self.natural_height > 0.).then_some(self.natural_height)),
            },
            BannerPhase::Collapsing => BannerFrame {
                x: self.from.x,
                opacity: 0.,
                height: Some(self.from.height.unwrap_or(self.natural_height) * (1. - progress)),
            },
        }
    }
}

#[derive(Default)]
pub struct BannerStack {
    pub items: Vec<Banner>,
    generation: u64,
}

impl BannerStack {
    pub fn is_live(&self, id: u32) -> bool {
        self.items
            .iter()
            .any(|banner| banner.notification.id == id && banner.live())
    }

    pub fn upsert(&mut self, notification: &Notification) {
        if let Some(index) = self
            .items
            .iter()
            .position(|banner| banner.notification.id == notification.id)
        {
            self.items[index].notification = notification.clone();
            if !self.items[index].live() {
                self.generation = self.generation.wrapping_add(1);
                let banner = &mut self.items[index];
                banner.token = self.generation;
                banner.from = banner.frame;
                banner.phase = BannerPhase::Entering;
            }
        } else {
            self.generation = self.generation.wrapping_add(1);
            let from = BannerFrame {
                x: theme::PANEL_WIDTH,
                opacity: 1.,
                height: None,
            };
            self.items.push(Banner {
                notification: notification.clone(),
                token: self.generation,
                phase: BannerPhase::Entering,
                natural_height: 0.,
                from,
                frame: from,
            });
        }
    }

    pub fn hide(&mut self, id: u32) {
        if let Some(banner) = self
            .items
            .iter_mut()
            .find(|banner| banner.notification.id == id && banner.live())
        {
            banner.from = banner.frame;
            banner.phase = BannerPhase::Fading;
        }
    }

    pub fn hide_all(&mut self) {
        let ids: Vec<_> = self
            .items
            .iter()
            .map(|banner| banner.notification.id)
            .collect();
        for id in ids {
            self.hide(id);
        }
    }

    pub fn frame(&mut self, token: u64, phase: BannerPhase, progress: f32, height: f32) -> bool {
        let Some(index) = self
            .items
            .iter()
            .position(|banner| banner.token == token && banner.phase == phase)
        else {
            return false;
        };
        let banner = &mut self.items[index];
        if height.is_finite() && height > 0. {
            banner.natural_height = height;
        }
        banner.frame = banner.sample(progress);
        if progress < 1. || phase == BannerPhase::Visible {
            return false;
        }
        match phase {
            BannerPhase::Entering => {
                banner.phase = BannerPhase::Visible;
            }
            BannerPhase::Fading => {
                banner.from = banner.frame;
                banner.phase = BannerPhase::Collapsing;
            }
            BannerPhase::Collapsing => {
                self.items.remove(index);
            }
            BannerPhase::Visible => {}
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notification(id: u32, summary: &str) -> Notification {
        Notification {
            id,
            app_name: "App".into(),
            app_icon: String::new(),
            summary: summary.into(),
            body: String::new(),
            actions: Vec::new(),
            urgency: None,
            resident: false,
        }
    }

    fn frame(stack: &mut BannerStack, id: u32, progress: f32, height: f32) {
        let banner = stack
            .items
            .iter()
            .find(|banner| banner.notification.id == id)
            .unwrap();
        stack.frame(banner.token, banner.phase, progress, height);
    }

    #[test]
    fn arrivals_append_below_stable_cards_and_replacements_keep_their_slot() {
        let mut stack = BannerStack::default();
        stack.upsert(&notification(8, "first"));
        frame(&mut stack, 8, 1., 120.);
        stack.upsert(&notification(2, "second"));
        stack.upsert(&notification(9, "third"));
        stack.upsert(&notification(8, "replacement"));
        assert_eq!(
            stack
                .items
                .iter()
                .map(|banner| banner.notification.id)
                .collect::<Vec<_>>(),
            [8, 2, 9]
        );
        assert_eq!(stack.items[0].notification.summary, "replacement");
        assert_eq!(stack.items[0].phase, BannerPhase::Visible);
        assert_eq!(stack.items[1].sample(0.).x, theme::PANEL_WIDTH);
        assert_eq!(stack.items[1].sample(1.).x, 0.);
    }

    #[test]
    fn departing_snapshot_fades_in_place_then_collapses_without_moving_its_neighbours_early() {
        let mut stack = BannerStack::default();
        stack.upsert(&notification(1, "oldest"));
        frame(&mut stack, 1, 1., 140.);
        stack.upsert(&notification(2, "lower"));
        frame(&mut stack, 2, 1., 100.);
        stack.hide(1);
        stack.hide(1);
        assert!(!stack.is_live(1));
        let fading = &stack.items[0];
        assert_eq!(fading.notification.summary, "oldest");
        assert_eq!(
            fading.sample(0.5),
            BannerFrame {
                x: 0.,
                opacity: 0.5,
                height: Some(140.)
            }
        );
        assert_eq!(fading.sample(1.).height, Some(140.));
        frame(&mut stack, 1, 1., 140.);
        assert_eq!(stack.items[0].phase, BannerPhase::Collapsing);
        assert_eq!(stack.items[0].sample(0.5).height, Some(70.));
        assert_eq!(stack.items[0].sample(0.5).opacity, 0.);
        frame(&mut stack, 1, 1., 140.);
        assert_eq!(stack.items.len(), 1);
        assert_eq!(stack.items[0].notification.id, 2);
    }

    #[test]
    fn overlap_and_replacement_cancel_stale_completion_without_duplicates() {
        let mut stack = BannerStack::default();
        stack.upsert(&notification(1, "first"));
        frame(&mut stack, 1, 0.5, 120.);
        stack.hide(1);
        assert_eq!(stack.items[0].sample(0.5).x, theme::PANEL_WIDTH * 0.5);
        frame(&mut stack, 1, 0.5, 120.);
        let old_token = stack.items[0].token;
        let before = stack.items[0].frame;
        stack.upsert(&notification(2, "incoming while fading"));
        stack.upsert(&notification(1, "updated while fading"));
        assert_eq!(stack.items[0].sample(0.), before);
        assert!(stack.is_live(1));
        assert!(!stack.frame(old_token, BannerPhase::Fading, 1., 120.));
        assert_eq!(stack.items.len(), 2);
        frame(&mut stack, 1, 1., 120.);
        stack.hide_all();
        frame(&mut stack, 1, 1., 120.);
        frame(&mut stack, 1, 0.5, 120.);
        let before = stack.items[0].frame;
        let old_token = stack.items[0].token;
        stack.upsert(&notification(1, "updated while collapsing"));
        assert_eq!(stack.items[0].sample(0.), before);
        assert!(!stack.frame(old_token, BannerPhase::Collapsing, 1., 120.));
        frame(&mut stack, 1, 1., 180.);
        assert_eq!(stack.items[0].phase, BannerPhase::Visible);
    }

    #[test]
    fn overlapping_removals_reflow_lower_cards_by_the_remaining_slot_heights() {
        let mut stack = BannerStack::default();
        for (id, height) in [(1, 100.), (2, 140.), (3, 80.)] {
            stack.upsert(&notification(id, "card"));
            frame(&mut stack, id, 1., height);
        }
        stack.hide(1);
        stack.hide(2);
        stack.upsert(&notification(4, "new arrival below all existing cards"));
        let offset = |stack: &BannerStack, first: f32, second: f32| {
            stack.items[0]
                .sample(first)
                .height
                .unwrap_or(stack.items[0].natural_height)
                + stack.items[1]
                    .sample(second)
                    .height
                    .unwrap_or(stack.items[1].natural_height)
        };
        assert_eq!(offset(&stack, 0.5, 0.5), 240.);
        frame(&mut stack, 1, 1., 100.);
        assert_eq!(offset(&stack, 0.5, 0.5), 190.);
        frame(&mut stack, 2, 1., 140.);
        assert_eq!(offset(&stack, 0.5, 0.5), 120.);
        frame(&mut stack, 1, 1., 100.);
        frame(&mut stack, 2, 1., 140.);
        assert_eq!(
            stack
                .items
                .iter()
                .map(|banner| banner.notification.id)
                .collect::<Vec<_>>(),
            [3, 4]
        );
    }
}
