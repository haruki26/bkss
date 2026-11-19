use bkss_integration::Notification;

/// Return the active, unexpired notifications in reverse arrival order.
pub fn active_notifications(notifications: &[Notification]) -> Vec<&Notification> {
    let mut active: Vec<_> = notifications.iter().collect();
    active.sort_by_key(|notification| std::cmp::Reverse(notification.id));
    active
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_the_current_snapshot_and_orders_newest_first() {
        let notifications = vec![
            notification(7, "older"),
            notification(9, "newer"),
            notification(8, "middle"),
        ];
        let active = active_notifications(&notifications);
        assert_eq!(
            active.iter().map(|item| item.id).collect::<Vec<_>>(),
            [9, 8, 7]
        );
        assert_eq!(active[0].summary, "newer");
    }

    fn notification(id: u32, summary: &str) -> Notification {
        Notification {
            id,
            app_name: "app".into(),
            app_icon: String::new(),
            summary: summary.into(),
            body: String::new(),
            actions: Vec::new(),
            urgency: None,
            resident: false,
        }
    }
}
