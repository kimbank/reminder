use super::super::{
    ACCOUNTS_PANEL_MAX_WIDTH, ACCOUNTS_PANEL_MIN_WIDTH, ACCOUNTS_PANEL_WIDTH_RATIO,
    COMPACT_ACCOUNT_ROW_WIDTH, COMPACT_NOTIFICATION_WIDTH, STACKED_ACCOUNT_HEADER_WIDTH,
};

pub(in crate::app) fn responsive_accounts_panel_width(window_width: f32) -> f32 {
    (window_width * ACCOUNTS_PANEL_WIDTH_RATIO)
        .clamp(ACCOUNTS_PANEL_MIN_WIDTH, ACCOUNTS_PANEL_MAX_WIDTH)
}

pub(in crate::app) fn uses_compact_account_rows(available_width: f32) -> bool {
    available_width < COMPACT_ACCOUNT_ROW_WIDTH
}

pub(in crate::app) fn uses_stacked_account_header(available_width: f32) -> bool {
    available_width < STACKED_ACCOUNT_HEADER_WIDTH
}

pub(in crate::app) fn uses_compact_notifications(available_width: f32) -> bool {
    available_width < COMPACT_NOTIFICATION_WIDTH
}
