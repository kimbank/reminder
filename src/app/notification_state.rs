use std::collections::HashSet;

use crate::domain::{InboxSnapshot, NotificationItem};

use super::{MENTION_REASONS, REVIEW_REQUEST_REASON};

pub(super) fn is_review_request(item: &NotificationItem) -> bool {
    item.reason == REVIEW_REQUEST_REASON
}

pub(super) fn is_mention(item: &NotificationItem) -> bool {
    MENTION_REASONS.contains(&item.reason.as_str())
}

pub(super) fn is_other_notification(item: &NotificationItem) -> bool {
    !is_review_request(item) && !is_mention(item)
}

pub(super) fn collect_new_notification_ids(
    previous: Option<&InboxSnapshot>,
    next: &InboxSnapshot,
) -> HashSet<String> {
    let Some(previous) = previous else {
        return HashSet::new();
    };

    let known_ids: HashSet<_> = previous
        .notifications
        .iter()
        .map(|item| item.thread_id.as_str())
        .collect();

    next.notifications
        .iter()
        .filter(|item| !known_ids.contains(item.thread_id.as_str()))
        .map(|item| item.thread_id.clone())
        .collect()
}

pub(super) struct SectionCounts {
    pub(super) unseen: usize,
    pub(super) updated: usize,
}

impl SectionCounts {
    pub(super) fn new(unseen: usize, updated: usize) -> Self {
        Self { unseen, updated }
    }

    pub(super) fn bumped_since(&self, previous: &SectionCounts) -> bool {
        self.unseen > previous.unseen || self.updated > previous.updated
    }
}

pub(super) struct SectionStats {
    pub(super) inbox: SectionCounts,
    pub(super) review_requests: SectionCounts,
    pub(super) mentions: SectionCounts,
    pub(super) notifications: SectionCounts,
}

pub(super) fn section_stats(inbox: &InboxSnapshot) -> SectionStats {
    let all_notifications: Vec<_> = inbox.notifications.iter().collect();
    let review_requests: Vec<_> = inbox
        .notifications
        .iter()
        .filter(|item| is_review_request(item))
        .collect();
    let mentions: Vec<_> = inbox
        .notifications
        .iter()
        .filter(|item| is_mention(item))
        .collect();
    let other: Vec<_> = inbox
        .notifications
        .iter()
        .filter(|item| is_other_notification(item))
        .collect();

    let (inbox_unseen, inbox_updated) = summarize_counts(&all_notifications);
    let (rr_unseen, rr_updated) = summarize_counts(&review_requests);
    let (m_unseen, m_updated) = summarize_counts(&mentions);
    let (o_unseen, o_updated) = summarize_counts(&other);

    SectionStats {
        inbox: SectionCounts::new(inbox_unseen, inbox_updated),
        review_requests: SectionCounts::new(rr_unseen, rr_updated),
        mentions: SectionCounts::new(m_unseen, m_updated),
        notifications: SectionCounts::new(o_unseen, o_updated),
    }
}

pub(super) fn pending_review_request_ids(inbox: &InboxSnapshot) -> HashSet<String> {
    let requested_prs: HashSet<_> = inbox
        .review_requests
        .iter()
        .filter_map(|request| request.pull_request_key())
        .collect();
    if requested_prs.is_empty() {
        return HashSet::new();
    }

    let reviewed_prs: HashSet<_> = inbox
        .recent_reviews
        .iter()
        .filter_map(|review| review.pull_request_key())
        .collect();

    inbox
        .notifications
        .iter()
        .filter_map(|item| {
            let pr_key = item.pull_request_key()?;
            if requested_prs.contains(&pr_key) && !reviewed_prs.contains(&pr_key) {
                Some(item.thread_id.clone())
            } else {
                None
            }
        })
        .collect()
}

#[derive(Clone, Copy)]
pub(super) struct NotificationVisualState {
    pub(super) seen: bool,
    pub(super) needs_revisit: bool,
    pub(super) pending_review: bool,
}

pub(super) fn base_notification_state(item: &NotificationItem) -> NotificationVisualState {
    let needs_revisit = item
        .last_read_at
        .map(|last_read| item.updated_at > last_read)
        .unwrap_or(false);

    NotificationVisualState {
        seen: !item.unread && !needs_revisit,
        needs_revisit,
        pending_review: false,
    }
}

pub(super) fn summarize_counts(items: &[&NotificationItem]) -> (usize, usize) {
    let mut unseen = 0;
    let mut updated = 0;
    for item in items {
        let visual = base_notification_state(item);
        if item.unread {
            unseen += 1;
        }
        if visual.needs_revisit {
            updated += 1;
        }
    }
    (unseen, updated)
}
