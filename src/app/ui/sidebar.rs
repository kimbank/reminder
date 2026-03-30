use eframe::egui::{self, RichText};

use super::super::{notification_state::section_stats, state::AccountState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) struct AccountOverview {
    pub(in crate::app) new_notifications: usize,
    pub(in crate::app) unseen: usize,
    pub(in crate::app) updated: usize,
}

pub(in crate::app) fn tracked_account_heading(
    ui: &egui::Ui,
    account: &AccountState,
    is_selected: bool,
    overview: Option<AccountOverview>,
) -> RichText {
    let has_new = overview
        .map(|overview| overview.new_notifications > 0)
        .unwrap_or(false);
    let mut text = RichText::new(&account.profile.login);
    if is_selected || has_new {
        text = text.strong();
    }
    if has_new && !is_selected {
        text = text.color(ui.visuals().warn_fg_color);
    }
    text
}

pub(in crate::app) fn render_tracked_account_badges(
    ui: &mut egui::Ui,
    overview: Option<AccountOverview>,
    pending: bool,
    has_error: bool,
) {
    if let Some(overview) = overview {
        if overview.new_notifications > 0 {
            ui.small(
                RichText::new(format!("new +{}", overview.new_notifications))
                    .strong()
                    .color(ui.visuals().warn_fg_color),
            );
        }
        ui.small(format!("unseen {}", overview.unseen));
        let updated = if overview.updated > 0 {
            RichText::new(format!("updated {}", overview.updated)).color(ui.visuals().warn_fg_color)
        } else {
            RichText::new(format!("updated {}", overview.updated))
                .color(ui.visuals().weak_text_color())
        };
        ui.small(updated);
    } else if pending {
        ui.small(RichText::new("Loading…").color(ui.visuals().weak_text_color()));
    } else {
        ui.small(RichText::new("No data yet").color(ui.visuals().weak_text_color()));
    }

    if pending {
        ui.small(RichText::new("syncing…").color(ui.visuals().weak_text_color()));
    }
    if has_error {
        ui.small(RichText::new("sync failed").color(ui.visuals().error_fg_color));
    }
}

pub(in crate::app) fn account_overview(account: &AccountState) -> Option<AccountOverview> {
    account.inbox.as_ref().map(|inbox| {
        let stats = section_stats(inbox);
        AccountOverview {
            new_notifications: account.new_notification_ids.len(),
            unseen: stats.inbox.unseen,
            updated: stats.inbox.updated,
        }
    })
}
